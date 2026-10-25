// lispm-remote: the device's side of LispM Remote.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * The page's shell around the device's machine (crates/web): a Rings node
 * of the device's own, the machine's effects carried out, and the loops that
 * keep the device in touch with LispM.
 *
 * - Every 0.5 s the machine ticks, so a frame waiting for its answer goes
 *   again and a phase waiting too long gives up.  After a channel opens,
 *   the page asks for LispM's state, `{"type": "state?"}`, every 2 s until
 *   a state arrives.
 * - Every 1 s, and on the browser's `online` and `offline` events, the
 *   page checks its peers.  With none, it joins Rings again through the
 *   seed, retrying every second: for about 5 s after the network drops, a
 *   seed still holds the device's old connection and refuses a new one
 *   ("Found existing transport").  A handshake of the page's own that never
 *   finishes refuses its joins too, for three minutes; after 10 s of that
 *   the page drops it (handshake.ts).  Once back, it closes the channel it
 *   had and opens a new one, Noise KK, without scanning again.
 * - When LispM has said nothing for 30 s, the page asks for the state; if
 *   nothing comes within 12 s, it opens a new channel, in case Emacs
 *   restarted and forgot the old one.
 * - When the seed or LispM runs another version, the page says so at once
 *   and stops trying (version.ts): it leaves a seed of another Rings
 *   version alone for 30 minutes, and opens no session with LispM of
 *   another protocol version until it reloads or pairs again.
 *
 * Commands to LispM carry an `id` and are sent again every 2 s until LispM
 * answers with an `ack` or `error` of that id; LispM answers a repeated id
 * without acting on it twice.
 *
 * The page follows one agent's conversation at a time: it sends `watch`
 * every 2 s until LispM sends the agent's `chat`, which LispM then sends
 * again whenever the conversation changes.  What the owner sent shows at
 * once, until the conversation holds it.
 */

import initDevice, { generateStatic, Initiator } from "lispm-remote-web";
import initRings, { ProcessorConfig, Provider, type ProviderListener } from "rings-node";
import { account, delegatee } from "./account.js";
import { type Held, holding, stuck } from "./handshake.js";
import { expiry } from "./steps.js";
import { Store } from "./store.js";
import {
  leftAlone,
  peerConclusion,
  ringsMismatch,
  ringsOlder,
  seedConclusion,
  versionNumber,
} from "./version.js";

/** The namespace LispM Remote's frames travel under on Rings. */
export const NAMESPACE = "lispm-remote/1";

/** Rings's network, as LispM's node joins it. */
const NETWORK_ID = 1;
/** The Chord stabilization interval, in seconds, as LispM's node has it. */
const STABILIZE_SECONDS = 3n;

const TICK_MS = 500;
const WATCH_MS = 1_000;
const ASK_MS = 2_000;
const QUIET_MS = 30_000;
const PROBE_MS = 12_000;
const RETRY_MS = 2_000;
const GIVE_UP_MS = 20_000;
const JOIN_TIMEOUT_MS = 20_000;
/** How long a join that succeeded may take to show a connected peer. */
const JOIN_GRACE_MS = 5_000;

/** What the page is configured with: the URL, or defaults. */
export interface Options {
  /** The seed node's HTTP endpoint the page joins Rings through. */
  readonly seed: string;
  /** ICE servers, as Rings writes them: `stun://host:port;...`. */
  readonly ice: string;
  /** What LispM's owner sees this device called. */
  readonly name: string;
}

/** The LispM this device paired with, as the machine's `paired` effect gave it. */
export interface Pairing {
  readonly responder: string;
  readonly key: string;
}

/** A call an agent waits for approval of. */
export interface Approval {
  readonly call: string;
  readonly tool: string;
  readonly summary: string;
}

/** One of LispM's agents, as its state describes it. */
export interface Agent {
  readonly name: string;
  readonly role?: string;
  readonly project?: string;
  readonly state?: string;
  readonly status?: string;
  readonly approvals?: readonly Approval[];
}

/** One item of an agent's conversation, as LispM's chat has it. */
export interface ChatItem {
  readonly kind: "user" | "text" | "thought" | "tool" | "note" | "event" | "summary" | string;
  readonly text?: string;
  readonly from?: string;
  readonly name?: string;
  readonly state?: string;
  readonly summary?: string;
  readonly result?: string;
  readonly failed?: boolean;
}

/** The latest part of an agent's conversation; `more` earlier items stay
 * in Emacs. */
export interface Chat {
  readonly agent: string;
  readonly items: readonly ChatItem[];
  readonly more: number;
}

/** Something the owner said that the conversation does not show yet. */
export interface Outgoing {
  readonly id: string;
  readonly agent: string;
  readonly text: string;
}

/** LispM's state, as `lispm-remote-state-function' returns it. */
export interface LispmState {
  readonly emacs?: string;
  readonly agents?: readonly Agent[];
}

/** Something to tell the owner. */
export interface Notice {
  readonly tone: "info" | "error";
  readonly text: string;
}

/** Where the device stands. */
export type Phase = "idle" | "pairing" | "opening" | "open";

/** An answer to "how is it going", for the page's status line. */
export interface Status {
  readonly tone: "ok" | "busy" | "bad";
  readonly text: string;
}

type Effect =
  | { type: "transmit"; to: string; frame: Uint8Array }
  | { type: "show-fingerprint"; fingerprint: string }
  | { type: "paired"; responder: string; key: string }
  | { type: "unpaired" }
  | { type: "opened"; channel: string }
  | { type: "delivered"; payload: Uint8Array }
  | { type: "closed"; reason: string }
  | { type: "failed"; reason: string; detail?: string; ours?: string }
  | { type: "discarded"; why: string };

/** A command on its way to LispM. */
interface Command {
  readonly message: Record<string, unknown> & { readonly id: string };
  readonly what: string;
  readonly since: number;
  sentAt: number;
}

interface PeerInfo {
  readonly did: string;
  readonly state: string;
}

/** The text of `error`, whatever was thrown. */
export function messageOf(error: unknown): string {
  if (error instanceof Error) {
    return error.message;
  }
  return typeof error === "string" ? error : JSON.stringify(error);
}

function timeout<T>(promise: Promise<T>, ms: number, what: string): Promise<T> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`${what} took too long`)), ms);
    promise.then(
      (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      (error: unknown) => {
        clearTimeout(timer);
        reject(error);
      },
    );
  });
}

function randomId(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(8));
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

/** What a failure to pair means to the owner. */
function pairingFailure(effect: { reason: string; detail?: string }): string {
  switch (effect.detail ?? effect.reason) {
    case "owner-refused":
      return "LispM's owner refused this device.";
    case "offer-unavailable":
      return "This code no longer works. Show a new one in Emacs with M-x lispm-remote-pair.";
    case "contested":
      return "Two devices used this code, so LispM dropped it: someone else may have seen it. Show a new one.";
    case "timed-out":
      return "The code expired before LispM answered. Show a new one.";
    case "abandoned":
      return "Pairing stopped.";
    default:
      return `Pairing failed: ${effect.detail ?? effect.reason}.`;
  }
}

/** The device's side of LispM Remote. */
export class Remote {
  /** Where the device stands with LispM. */
  phase: Phase = "idle";
  /** The fingerprint to compare while pairing. */
  fingerprint: string | null = null;
  /** LispM's latest state. */
  state: LispmState | null = null;
  /** The latest thing to tell the owner. */
  notice: Notice | null = null;
  /** Connected Rings peers, at the last look. */
  peers = 0;
  /** Why the last join failed, if it did. */
  joinError: string | null = null;
  /** Joins that failed since the device last had a peer. */
  joinAttempts = 0;
  /** How the code being paired with came: through the camera or its link. */
  code: "camera" | "link" | null = null;
  /** When that code came, and when it expires, in milliseconds. */
  codeAt: number | null = null;
  expires: number | null = null;
  /** How often this attempt's request to LispM went out, and whether Rings
   * took it. */
  sends = 0;
  sent = false;
  /** Whether LispM did not answer the last attempt to open a session. */
  unanswered = false;
  /** When the device last paired, last opened a session, and last got
   * LispM's state on a session it opened. */
  pairedAt = 0;
  openedAt = 0;
  stateAt = 0;
  /** The conversation of the agent the page follows, once it came. */
  chat: Chat | null = null;
  /** What the owner said that the conversation does not show yet. */
  outbox: Outgoing[] = [];
  /** The Rings version this page's node runs, once known. */
  rings: string | null = null;
  /** When a join last failed because the seed runs another Rings version,
   * or 0, and the side whose Rings that failure said is older. */
  mismatchAt = 0;
  mismatchOlder: "ours" | "seed" | null = null;
  /** The protocol versions of LispM and of this page, once LispM answered
   * in another. */
  otherVersion: { readonly theirs: number; readonly ours: number } | null = null;

  private following: string | null = null;
  private followedAt = 0;
  private asking = false;
  private askedAt = 0;
  private lastHeard = 0;
  private probeSince = 0;
  private attempt = 0;
  private stale = false;
  private joinedAt = 0;
  private held: Held | null = null;
  private watching = false;
  private token: string | null = null;
  private readonly commands = new Map<string, Command>();
  private readonly displayListeners: ((payload: Uint8Array) => void)[] = [];
  private readonly channelListeners: ((open: boolean) => void)[] = [];

  private constructor(
    private readonly store: Store,
    private readonly provider: Provider,
    private readonly listener: ProviderListener,
    private readonly machine: Initiator,
    public pairing: Pairing | null,
    private readonly options: Options,
    private readonly changed: () => void,
  ) {}

  /** Starts the device: its keys, its node, its machine and its loops.
   * `changed` is called whenever what the page shows may have changed. */
  static async start(options: Options, changed: () => void): Promise<Remote> {
    await Promise.all([initDevice(), initRings()]);
    const store = await Store.open();
    let secret = await store.get<string>("static");
    if (!secret) {
      secret = generateStatic();
      await store.put("static", secret);
    }
    const pairing = (await store.get<Pairing>("pairing")) ?? null;
    const config = ProcessorConfig.new(
      NETWORK_ID,
      options.ice,
      await delegatee(await account(store)),
      STABILIZE_SECONDS,
    );
    // The replay records go to IndexedDB under this name: a device that
    // reloads must not start its message sequences over, or Rings refuses
    // it ("Transaction sequence 0 forks").
    const rings = "lispm-remote/rings";
    await store.prepareReplay(rings);
    const provider = (await Provider.new_provider_with_storage(config, rings)) as Provider;
    let remote: Remote | null = null;
    provider.on(
      NAMESPACE,
      null,
      (context: { state: unknown }, event: { from: string; payload: Uint8Array }) => {
        const { from, payload } = event;
        // Off Rings's own call stack: the machine's effects call the node.
        queueMicrotask(() => remote?.receive(from, payload));
        return { state: context.state, effects: [] };
      },
    );
    const listener = provider.listen();
    await listener.started();
    const machine = new Initiator(secret, provider.address, pairing);
    remote = new Remote(store, provider, listener, machine, pairing, options, changed);
    remote.run();
    return remote;
  }

  /** This device's DID on Rings. */
  get did(): string {
    return this.provider.address;
  }

  /** The seed's host, as the owner may know it. */
  get seed(): string {
    try {
      return new URL(this.options.seed).host;
    } catch {
      return this.options.seed;
    }
  }

  /** Whether an offer waits for the device to be on the network. */
  get waitingToPair(): boolean {
    return this.token !== null;
  }

  /** Whether the node still listens. */
  get listening(): boolean {
    return !this.listener.is_stopped();
  }

  private run(): void {
    setInterval(() => this.tick(), TICK_MS);
    setInterval(() => void this.watch(), WATCH_MS);
    addEventListener("online", () => {
      this.lost();
      void this.watch();
    });
    addEventListener("offline", () => this.lost());
    // Which Rings this page runs, to say so when a seed runs another.
    void (this.provider.request("nodeInfo", {}) as Promise<{ version?: string }>).then(
      (info) => {
        this.rings = versionNumber(info.version);
      },
      (error: unknown) => console.debug("lispm-remote: nodeInfo failed:", error),
    );
    void this.watch();
  }

  /** Whether a join failed lately because the seed runs another Rings
   * version, so that the page leaves it alone. */
  mismatched(now = Date.now()): boolean {
    return leftAlone(this.mismatchAt, now);
  }

  /** What the page concluded from the seed running another Rings version,
   * while it leaves the seed alone. */
  seedProblem(): string | null {
    return this.peers === 0 && this.mismatched()
      ? seedConclusion(this.seed, this.rings, this.mismatchOlder)
      : null;
  }

  /** What the page concluded from LispM speaking another protocol version. */
  peerProblem(): string | null {
    return this.otherVersion && peerConclusion(this.otherVersion.theirs, this.otherVersion.ours);
  }

  /** How it is going, in a few words. */
  status(): Status {
    if (navigator.onLine === false) {
      return { tone: "bad", text: "Offline" };
    }
    if (this.seedProblem()) {
      return { tone: "bad", text: `${this.seed} runs another Rings version` };
    }
    if (this.peers === 0) {
      return { tone: "busy", text: "Joining Rings Network…" };
    }
    if (this.otherVersion) {
      return {
        tone: "bad",
        text:
          this.otherVersion.theirs < this.otherVersion.ours
            ? "LispM Remote in Emacs is older than this page"
            : "This page is older than LispM Remote in Emacs",
      };
    }
    switch (this.phase) {
      case "pairing":
        return { tone: "busy", text: "Pairing…" };
      case "opening":
        return {
          tone: "busy",
          text: this.unanswered ? "LispM does not answer; trying again…" : "Connecting to LispM…",
        };
      case "open":
        return this.state
          ? { tone: "ok", text: "Connected" }
          : { tone: "busy", text: "Waiting for LispM…" };
      case "idle":
        return this.pairing
          ? { tone: "busy", text: "Connecting to LispM…" }
          : { tone: "bad", text: "Not paired" };
    }
  }

  // --------------------------------------------------------------------
  // Pairing

  /** Pairs with the offer `token`, from LispM's QR code, read by the
   * camera or opened as its link, as soon as the device is on the network. */
  pairWith(token: string, how: "camera" | "link"): void {
    this.token = token;
    this.code = how;
    this.codeAt = Date.now();
    this.expires = expiry(token);
    this.notice = null;
    // A new code may come from a LispM Remote updated meanwhile.
    this.otherVersion = null;
    this.changed();
    void this.watch();
  }

  /** Forgets the paired LispM on this device. */
  async forget(): Promise<void> {
    if (this.machine.openP()) {
      this.apply(this.machine.close());
    }
    this.pairing = null;
    this.state = null;
    await this.store.delete("pairing");
    // The machine still holds the pairing; a fresh one does not.
    location.reload();
  }

  // --------------------------------------------------------------------
  // Commands

  /** Gives the agent `agent` the input `text`. */
  say(agent: string, text: string): void {
    const id = this.command({ type: "say", agent, text }, `Sent to ${agent}`);
    if (id) {
      this.outbox.push({ id, agent, text });
      this.changed();
    }
  }

  /** Follows the conversation of `agent`, or none. */
  follow(agent: string | null): void {
    if (agent === this.following) {
      return;
    }
    const was = this.following;
    // Before sending: sending draws the page, which follows again.
    this.following = agent;
    this.followedAt = 0;
    if (this.chat?.agent !== agent) {
      this.chat = null;
    }
    if (was && !agent && this.phase === "open") {
      this.send({ type: "watch" });
    }
  }

  /** Answers the call `call` that `agent` waits on. */
  decide(agent: string, call: string, decision: "allow" | "always" | "deny"): void {
    const done = { allow: "Allowed", always: "Allowed from now on", deny: "Denied" }[decision];
    this.command({ type: "decide", agent, call, decision }, done);
  }

  /** Stops the turn of `agent`. */
  interrupt(agent: string): void {
    this.command({ type: "interrupt", agent }, `Stopped ${agent}`);
  }

  /** Sends the command `message` until LispM answers; returns its id, or
   * null when there is no channel to send it on. */
  private command(message: Record<string, unknown>, what: string): string | null {
    if (this.phase !== "open") {
      this.notice = { tone: "error", text: "Not connected to LispM." };
      this.changed();
      return null;
    }
    const now = Date.now();
    const command: Command = { message: { ...message, id: randomId() }, what, since: now, sentAt: now };
    this.commands.set(command.message.id, command);
    this.send(command.message);
    return command.message.id;
  }

  private answered(message: { type: string; id?: string; command?: string; message?: string }): void {
    if (message.command === "watch") {
      if (message.type === "error") {
        this.notice = { tone: "error", text: message.message ?? "LispM has no such agent." };
        this.following = null;
      }
      return;
    }
    if (message.type === "error" && message.id !== undefined) {
      const failed = message.id;
      this.outbox = this.outbox.filter((outgoing) => outgoing.id !== failed);
    }
    const command = message.id === undefined ? undefined : this.commands.get(message.id);
    if (message.id !== undefined && !command) {
      return; // An answer to a repeat.
    }
    if (message.id !== undefined) {
      this.commands.delete(message.id);
    }
    this.notice =
      message.type === "ack"
        ? { tone: "info", text: command?.what ?? "Done" }
        : { tone: "error", text: message.message ?? "LispM could not do that." };
  }

  // --------------------------------------------------------------------
  // Loops

  private tick(): void {
    this.apply(this.machine.tick());
    if (this.phase !== "open") {
      return;
    }
    const now = Date.now();
    if (!this.asking && now - this.lastHeard >= QUIET_MS) {
      this.asking = true;
      this.askedAt = 0;
      this.probeSince = now;
    }
    if (this.probeSince && now - this.probeSince >= PROBE_MS) {
      // LispM said nothing to a question: its side of the channel may be
      // gone with an Emacs that restarted.
      this.probeSince = 0;
      this.apply(this.machine.close());
      return;
    }
    if (this.asking && now - this.askedAt >= ASK_MS) {
      this.askedAt = now;
      this.send({ type: "state?" });
    }
    if (this.following && this.chat?.agent !== this.following && now - this.followedAt >= ASK_MS) {
      this.followedAt = now;
      this.send({ type: "watch", agent: this.following });
    }
    for (const [id, command] of this.commands) {
      if (now - command.since >= GIVE_UP_MS) {
        this.commands.delete(id);
        this.outbox = this.outbox.filter((outgoing) => outgoing.id !== id);
        this.notice = { tone: "error", text: `LispM did not answer: ${command.what.toLowerCase()}?` };
        this.changed();
      } else if (now - command.sentAt >= RETRY_MS) {
        command.sentAt = now;
        this.send(command.message);
      }
    }
  }

  private async peerCount(): Promise<number> {
    const result = (await this.provider.request("listPeers", {})) as { peers?: PeerInfo[] };
    return (result.peers ?? []).filter((peer) => peer.state === "Connected").length;
  }

  private async watch(): Promise<void> {
    if (this.watching) {
      return;
    }
    this.watching = true;
    try {
      const peers = await this.peerCount();
      if (peers === 0 && this.peers > 0) {
        this.lost();
      }
      this.peers = peers;
      if (peers > 0) {
        this.joinError = null;
        this.joinAttempts = 0;
        this.held = null;
        this.mismatchAt = 0;
        this.proceed();
      } else if (
        navigator.onLine !== false &&
        Date.now() - this.joinedAt >= JOIN_GRACE_MS &&
        !this.mismatched()
      ) {
        await this.join();
      }
    } catch (error) {
      this.joinError = messageOf(error);
    } finally {
      this.watching = false;
      this.changed();
    }
  }

  private async join(): Promise<void> {
    try {
      await timeout(this.provider.connect_peer_via_http(this.options.seed), JOIN_TIMEOUT_MS, "Joining");
      this.joinedAt = Date.now();
      this.joinError = null;
      this.held = null;
    } catch (error) {
      this.joinError = messageOf(error);
      this.joinAttempts++;
      if (ringsMismatch(this.joinError)) {
        this.mismatchAt = Date.now();
        this.mismatchOlder = ringsOlder(this.joinError);
        console.warn(`lispm-remote: ${seedConclusion(this.seed, this.rings, this.mismatchOlder)}`);
      }
      await this.release(this.joinError);
    }
  }

  /** After a join failed with `message`: drops the handshake refusing the
   * page's joins, once it has for long enough. */
  private async release(message: string): Promise<void> {
    const now = Date.now();
    this.held = holding(this.held, message, now);
    if (!stuck(this.held, now)) {
      return;
    }
    const { peer } = this.held;
    this.held = null;
    try {
      await this.provider.disconnect(peer);
    } catch (error) {
      this.joinError = messageOf(error);
    }
  }

  /** The network went away: what the channel had may be lost. */
  private lost(): void {
    if (this.phase === "open") {
      this.stale = true;
    }
    this.changed();
  }

  /** On the network: pair, or reopen the channel, as needed. */
  private proceed(): void {
    if (this.stale) {
      this.stale = false;
      if (this.machine.openP()) {
        this.apply(this.machine.close());
      }
    }
    const token = this.token;
    if (token && (this.phase === "idle" || this.phase === "open")) {
      this.token = null;
      if (this.machine.openP()) {
        this.apply(this.machine.close());
      }
      this.state = null;
      this.step("pairing", () => this.machine.pair(token, this.options.name));
    } else if (this.pairing && this.phase === "idle" && !this.otherVersion) {
      // LispM of another version would answer every try the same.
      this.step("opening", () => this.machine.connect());
    }
  }

  // --------------------------------------------------------------------
  // The machine

  private step(phase: Phase, command: () => Effect[]): void {
    const before = this.phase;
    this.phase = phase;
    this.attempt++;
    this.sends = 0;
    this.sent = false;
    try {
      this.apply(command());
    } catch (error) {
      this.phase = before;
      this.notice = { tone: "error", text: messageOf(error) };
      this.changed();
    }
  }

  private send(message: object): void {
    if (!this.machine.openP()) {
      return;
    }
    try {
      this.apply(this.machine.send(new TextEncoder().encode(JSON.stringify(message))));
    } catch (error) {
      this.notice = { tone: "error", text: messageOf(error) };
    }
  }

  private receive(from: string, frame: Uint8Array): void {
    this.apply(this.machine.receive(from, frame));
  }

  private apply(effects: Effect[]): void {
    if (effects.length === 0) {
      return;
    }
    for (const effect of effects) {
      switch (effect.type) {
        case "transmit": {
          const handshake = this.phase === "pairing" || this.phase === "opening";
          const attempt = this.attempt;
          if (handshake) {
            this.sends++;
          }
          this.provider.send_message(effect.to, NAMESPACE, effect.frame).then(
            () => {
              if (handshake && attempt === this.attempt && !this.sent) {
                this.sent = true;
                this.changed();
              }
            },
            (error: unknown) => console.debug("lispm-remote: sending failed:", error),
          );
          break;
        }
        case "show-fingerprint":
          this.fingerprint = effect.fingerprint;
          break;
        case "paired":
          this.pairing = { responder: effect.responder, key: effect.key };
          this.pairedAt = Date.now();
          void this.store.put("pairing", this.pairing);
          break;
        case "unpaired":
          this.pairing = null;
          this.state = null;
          this.code = null;
          void this.store.delete("pairing");
          this.notice = {
            tone: "error",
            text: "LispM no longer knows this device. Scan a new code to pair again.",
          };
          break;
        case "opened":
          this.phase = "open";
          this.openedAt = Date.now();
          // A new channel follows nothing until asked.
          this.chat = null;
          this.followedAt = 0;
          this.unanswered = false;
          this.asking = true;
          this.askedAt = 0;
          this.probeSince = 0;
          this.lastHeard = Date.now();
          this.tellChannel(true);
          break;
        case "delivered":
          this.heard(effect.payload);
          break;
        case "closed":
          this.phase = "idle";
          this.asking = false;
          this.tellChannel(false);
          break;
        case "failed":
          if (effect.reason === "other-version") {
            this.otherVersion = { theirs: Number(effect.detail), ours: Number(effect.ours) };
            const conclusion = peerConclusion(this.otherVersion.theirs, this.otherVersion.ours);
            this.notice = { tone: "error", text: conclusion };
            console.warn(`lispm-remote: ${conclusion}`);
          }
          if (this.phase === "pairing") {
            this.fingerprint = null;
            this.code = null;
            this.expires = null;
            if (!this.otherVersion) {
              this.notice = { tone: "error", text: pairingFailure(effect) };
            }
          } else if (effect.reason === "timed-out") {
            this.unanswered = true;
          }
          this.phase = "idle";
          break;
        case "discarded":
          console.debug("lispm-remote: discarded a frame:", effect.why);
          break;
      }
    }
    this.changed();
  }

  /** Sends `payload`, a message of LispM's display protocol (its first
   * byte is 1), on the channel, if one is open. */
  sendDisplay(payload: Uint8Array): void {
    if (!this.machine.openP()) {
      return;
    }
    try {
      this.apply(this.machine.send(payload));
    } catch (error) {
      this.notice = { tone: "error", text: messageOf(error) };
    }
  }

  /** Calls `listener` with each display message LispM sends. */
  onDisplay(listener: (payload: Uint8Array) => void): void {
    this.displayListeners.push(listener);
  }

  /** Calls `listener` with `true` when a channel to LispM opens, and
   * `false` when it closes. */
  onChannel(listener: (open: boolean) => void): void {
    this.channelListeners.push(listener);
  }

  /** Tells the channel's listeners, after the effects at hand are done. */
  private tellChannel(open: boolean): void {
    queueMicrotask(() => {
      for (const listener of this.channelListeners) {
        listener(open);
      }
    });
  }

  private heard(payload: Uint8Array): void {
    // A display message (first byte 1) goes to the display; `{` is JSON.
    if (payload[0] === 1) {
      this.lastHeard = Date.now();
      this.probeSince = 0;
      for (const listener of this.displayListeners) {
        listener(payload);
      }
      return;
    }
    let message: {
      type?: unknown;
      state?: LispmState;
      id?: string;
      command?: string;
      message?: string;
      agent?: string;
      items?: ChatItem[];
      more?: number;
    };
    try {
      message = JSON.parse(new TextDecoder().decode(payload));
    } catch {
      return;
    }
    this.lastHeard = Date.now();
    this.probeSince = 0;
    switch (message.type) {
      case "state":
        if (this.openedAt > this.stateAt) {
          this.stateAt = Date.now();
        }
        this.state = message.state ?? null;
        this.asking = false;
        break;
      case "chat":
        if (message.agent === this.following && message.agent !== undefined) {
          const chat: Chat = { agent: message.agent, items: message.items ?? [], more: message.more ?? 0 };
          this.chat = chat;
          this.outbox = this.outbox.filter(
            (outgoing) =>
              outgoing.agent !== chat.agent ||
              !chat.items.some((item) => item.kind === "user" && item.text === outgoing.text),
          );
        }
        break;
      case "ack":
      case "error":
        this.answered(message as { type: string; id?: string; command?: string; message?: string });
        break;
    }
  }
}
