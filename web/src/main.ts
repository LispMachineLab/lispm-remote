// lispm-remote: the page at lispm.ai/remote.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * What the owner sees on the device: how the connection stands, pairing
 * and reconnecting as steps (steps.ts, stepper.ts), and LispM's agents,
 * each a chat (chat.ts).  The page takes from its URL
 *
 *   #TOKEN       an offer, as LispM's QR code opens the page (then removed)
 *   ?seed=URL    the seed node to join Rings through (https://node.rings.rs)
 *   ?ice=LIST    ICE servers, as Rings writes them
 *   ?name=TEXT   what LispM's owner sees this device called
 *   ?debug       the page's state as `window.lispmRemote`, for tests
 */

import { chatting, showAgents } from "./chat.js";
import { closeWindows, openWindows, showingWindows } from "./windows.js";
import { button, byId, h, rebuild } from "./dom.js";
import { messageOf, type Options, Remote } from "./remote.js";
import { scan, takeToken } from "./scan.js";
import { Stepper } from "./stepper.js";
import { steps } from "./steps.js";

const DEFAULT_SEED = "https://node.rings.rs";
const DEFAULT_ICE = "stun://stun.l.google.com:19302";

/** A short name for this browser and system, as "Safari on iPhone". */
function deviceName(): string {
  const agent = navigator.userAgent;
  const system = /iPhone/.test(agent)
    ? "iPhone"
    : /iPad/.test(agent)
      ? "iPad"
      : /Android/.test(agent)
        ? "Android"
        : /Mac OS X/.test(agent)
          ? "Mac"
          : /Windows/.test(agent)
            ? "Windows"
            : /Linux/.test(agent)
              ? "Linux"
              : "a browser";
  const browser = /EdgA?\//.test(agent)
    ? "Edge"
    : /Firefox\/|FxiOS/.test(agent)
      ? "Firefox"
      : /Chrome\/|CriOS/.test(agent)
        ? "Chrome"
        : /Safari\//.test(agent)
          ? "Safari"
          : "";
  return browser ? `${browser} on ${system}` : system;
}

function options(): Options {
  const query = new URLSearchParams(location.search);
  return {
    seed: query.get("seed") || DEFAULT_SEED,
    ice: query.get("ice") || DEFAULT_ICE,
    // A device name is at most 64 bytes.
    name: (query.get("name") || deviceName()).slice(0, 48),
  };
}

// ----------------------------------------------------------------------
// Elements

const statusLine = byId<HTMLParagraphElement>("status");
const noticeLine = byId<HTMLParagraphElement>("notice");
const pairSection = byId<HTMLElement>("pair");
const footer = byId<HTMLElement>("about");
const windowsButton = byId<HTMLButtonElement>("open-windows");

let remote: Remote | null = null;
let scanning: AbortController | null = null;

/** How long the steps stay after they are all done, in milliseconds. */
const LINGER_MS = 2_500;
const stepper = new Stepper({ scan: startScan, cancel: stopScan });
pairSection.replaceChildren(stepper.element);
/** The flow the steps showed last, if they showed. */
let flow: "pair" | "reconnect" | null = null;

function showStatus(tone: string, text: string): void {
  statusLine.className = `status ${tone}`;
  statusLine.textContent = text;
}

// ----------------------------------------------------------------------
// Pairing

function startScan(): void {
  scanning?.abort();
  const controller = new AbortController();
  scanning = controller;
  render();
  scan(stepper.video, controller.signal).then(
    (token) => {
      scanning = null;
      remote?.pairWith(token, "camera");
      render();
    },
    (error: unknown) => {
      if (scanning === controller) {
        scanning = null;
        if (remote && !(error instanceof DOMException && error.name === "AbortError")) {
          remote.notice = { tone: "error", text: `The camera could not be used: ${messageOf(error)}` };
        }
        render();
      }
    },
  );
}

function stopScan(): void {
  scanning?.abort();
  scanning = null;
  render();
}

/** Whether LispM's state came on the session open now. */
function settled(current: Remote): boolean {
  return current.phase === "open" && current.stateAt >= current.openedAt;
}

/** Which steps to show now: pairing's, reconnecting's, or none. */
function flowOf(current: Remote, now: number): "pair" | "reconnect" | null {
  if (scanning || current.waitingToPair || current.phase === "pairing" || !current.pairing) {
    return "pair";
  }
  // A device that lost its peers has a session only in name.
  const done = settled(current) && current.peers > 0;
  if (!done) {
    // Paired a moment ago: the same steps go on to the end.
    return flow === "pair" ? "pair" : "reconnect";
  }
  return flow && now - current.stateAt < LINGER_MS ? flow : null;
}

function renderPair(current: Remote): void {
  const now = Date.now();
  flow = flowOf(current, now);
  pairSection.hidden = flow === null;
  if (flow === null) {
    return;
  }
  // While pairing anew, what the device had before does not count.
  const fresh =
    flow === "reconnect" || (current.codeAt !== null && current.pairedAt >= current.codeAt);
  const open = fresh && current.phase === "open" && current.peers > 0;
  stepper.update(
    flow,
    steps({
      flow,
      online: navigator.onLine !== false,
      peers: current.peers,
      joinAttempts: current.joinAttempts,
      nextJoinAt: current.nextJoinAt,
      refused: current.joinRefusals > 0,
      seed: current.seed,
      scanning: scanning !== null,
      code: current.code,
      phase: current.phase,
      sends: current.sends,
      sent: current.sent,
      unanswered: current.unanswered,
      fingerprint: current.fingerprint,
      since: current.codeAt,
      expires: current.expires,
      open,
      state: open && settled(current),
      now,
    }),
    { scanning: scanning !== null, fingerprint: current.fingerprint },
  );
}

// ----------------------------------------------------------------------
// The page

function renderAbout(current: Remote): void {
  const joining = current.peers === 0 ? current.joinError : null;
  const key = JSON.stringify([current.state?.emacs, current.did, joining, Boolean(current.pairing)]);
  footer.hidden = chatting();
  rebuild(footer, key, () => [
    current.state?.emacs && h("span", {}, `Emacs ${current.state.emacs}`),
    h("span", { class: "did" }, `This device: ${current.did}`),
    joining && h("span", {}, `Joining: ${joining}`),
    current.pairing && button("Pair with another LispM", startScan, "link"),
    current.pairing &&
      button(
        "Forget this LispM",
        () => {
          if (confirm("Forget this LispM on this device? To pair again, you will scan a new code.")) {
            void current.forget();
          }
        },
        "link",
      ),
  ]);
}

function draw(current: Remote): void {
  const status = current.status();
  showStatus(status.tone, status.text);
  noticeLine.className = `notice ${current.notice?.tone ?? ""}`;
  noticeLine.textContent = current.notice?.text ?? "";
  renderPair(current);
  showAgents(current, render);
  renderAbout(current);
  windowsButton.hidden = !current.pairing || showingWindows();
}

windowsButton.addEventListener("click", () => {
  if (remote) {
    void openWindows(remote, () => {
      closeWindows();
      render();
    }).then(render);
  }
});

let drawing = false;
let stale = false;

/** Draws the page as it stands.  What drawing changes, such as the agent
 * the page follows, draws it once more, after this draw, never inside it. */
function render(): void {
  const current = remote;
  if (!current) {
    return;
  }
  if (drawing) {
    stale = true;
    return;
  }
  drawing = true;
  try {
    for (let pass = 0; pass < 3; pass++) {
      stale = false;
      draw(current);
      if (!stale) {
        break;
      }
    }
  } finally {
    drawing = false;
  }
}

async function main(): Promise<void> {
  // First of all, so the token leaves the URL and the history at once.
  const token = takeToken();
  showStatus("busy", "Starting…");
  try {
    remote = await Remote.start(options(), render);
  } catch (error) {
    showStatus("bad", "LispM Remote could not start");
    noticeLine.className = "notice error";
    noticeLine.textContent = messageOf(error);
    console.error(error);
    return;
  }
  if (new URLSearchParams(location.search).has("debug")) {
    (globalThis as { lispmRemote?: Remote }).lispmRemote = remote;
  }
  if (token) {
    remote.pairWith(token, "link");
  }
  render();
  // The countdown and the steps that linger move with the clock.
  setInterval(render, 1_000);
}

void main();
