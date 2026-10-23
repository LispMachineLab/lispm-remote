// lispm-remote: the steps of pairing and of reconnecting.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * Pairing goes through five steps, which Emacs shows too, so that the owner
 * can follow one pairing on either screen:
 *
 *   join       join Rings Network through the seed
 *   scan       read LispM's code with the camera (skipped for the code's link)
 *   reach      send the request to LispM, again every 2 s until it answers
 *   confirm    compare the fingerprint, before the code expires
 *   connected  paired, and LispM's state arrived
 *
 * Reconnecting goes through join, reach and connected.  `steps` is a pure
 * function of what the page knows; the page draws what it returns.
 */

import type { Phase } from "./remote.js";

/** The steps, by the name each one's element carries in `data-step`. */
export type StepName = "join" | "scan" | "reach" | "confirm" | "connected";

/** Where a step stands, as its element carries it in `data-state`. */
export type StepState = "pending" | "active" | "done" | "skipped" | "failed";

/** One step, as the page draws it. */
export interface Step {
  readonly name: StepName;
  readonly state: StepState;
  readonly title: string;
  readonly detail: string;
  /** "busy" for work under way, a fraction for time left, or none. */
  readonly bar?: "busy" | number;
}

/** What the page knows that the steps follow. */
export interface Situation {
  /** Pairing, or reconnecting a device already paired. */
  readonly flow: "pair" | "reconnect";
  /** The browser says it has a network. */
  readonly online: boolean;
  /** Connected Rings peers. */
  readonly peers: number;
  /** Joins that failed since the device last had a peer. */
  readonly joinAttempts: number;
  /** When the page joins again, in milliseconds. */
  readonly nextJoinAt: number;
  /** Whether the seed answered but turned the page away. */
  readonly refused: boolean;
  /** The seed's host. */
  readonly seed: string;
  /** The camera is looking for the code. */
  readonly scanning: boolean;
  /** How the code came: through the camera, through its link, or not yet. */
  readonly code: "camera" | "link" | null;
  /** Where the device's machine stands. */
  readonly phase: Phase;
  /** How often the request to LispM was sent in this attempt. */
  readonly sends: number;
  /** Whether Rings took the request. */
  readonly sent: boolean;
  /** Whether LispM did not answer the last attempt to open a session. */
  readonly unanswered: boolean;
  /** The fingerprint to compare, while pairing. */
  readonly fingerprint: string | null;
  /** When the code was read and when it expires, in milliseconds. */
  readonly since: number | null;
  readonly expires: number | null;
  /** Whether a channel is open, and whether LispM's state came on it. */
  readonly open: boolean;
  readonly state: boolean;
  /** The moment, in milliseconds. */
  readonly now: number;
}

const TITLES: Record<StepName, string> = {
  join: "Join",
  scan: "Scan",
  reach: "Reach",
  confirm: "Confirm",
  connected: "Connected",
};

/** `ms` as minutes and seconds, as 1:05. */
export function clock(ms: number): string {
  const seconds = Math.max(0, Math.ceil(ms / 1000));
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}

function step(name: StepName, state: StepState, detail: string, bar?: "busy" | number): Step {
  return bar === undefined
    ? { name, state, title: TITLES[name], detail }
    : { name, state, title: TITLES[name], detail, bar };
}

function join(s: Situation): Step {
  if (s.peers > 0) {
    return step("join", "done", `On Rings Network, through ${s.seed}`);
  }
  if (!s.online) {
    return step("join", "failed", "This device is offline; it joins again once it has a network");
  }
  const wait = s.nextJoinAt - s.now;
  const again = wait > 1_500 ? ` · again in ${Math.ceil(wait / 1000)} s` : "";
  if (s.refused) {
    return step("join", "active", `${s.seed} turns this device away for now${again}`, "busy");
  }
  const trying = s.joinAttempts > 0 ? `still trying, ${s.joinAttempts}` : "joining";
  return step("join", "active", `Through ${s.seed}: ${trying}${again}`, "busy");
}

function scan(s: Situation): Step {
  switch (s.code) {
    case "link":
      return step("scan", "skipped", "Opened from the code's link");
    case "camera":
      return step("scan", "done", "Read the code");
    default:
      return step(
        "scan",
        "active",
        s.scanning ? "Point the camera at the code Emacs shows" : "In Emacs, run M-x lispm-remote-pair",
      );
  }
}

function times(n: number): string {
  return n === 1 ? "once" : n === 2 ? "twice" : `${n} times`;
}

function reach(s: Situation): Step {
  if (s.open || (s.flow === "pair" && s.fingerprint !== null && s.sent)) {
    return step("reach", "done", s.sends > 1 ? `Sent ${times(s.sends)}` : "Sent to LispM");
  }
  const busy = s.flow === "pair" ? s.phase === "pairing" : s.phase === "opening";
  if (!busy || s.peers === 0) {
    if (s.flow === "reconnect" && s.unanswered && s.peers > 0) {
      return step("reach", "active", "LispM does not answer; trying again", "busy");
    }
    return step("reach", "pending", s.flow === "pair" ? "Sends the pairing request" : "Opens a session");
  }
  const first = s.flow === "pair" ? "Sending the pairing request" : "Opening a session";
  const detail = s.sends <= 1 ? first : `Sent ${times(s.sends)}; LispM has not answered yet`;
  return step("reach", "active", detail, "busy");
}

function confirm(s: Situation): Step {
  if (s.open) {
    return step("confirm", "done", "Approved in Emacs");
  }
  if (s.phase !== "pairing" || !s.fingerprint) {
    return step("confirm", "pending", "Compare the fingerprint with Emacs");
  }
  if (s.expires !== null && s.since !== null && s.expires > s.since) {
    const left = s.expires - s.now;
    return step(
      "confirm",
      "active",
      `Check that LispM shows the same · ${clock(left)} left`,
      Math.max(0, Math.min(1, left / (s.expires - s.since))),
    );
  }
  return step("confirm", "active", "Check that LispM shows the same");
}

function connected(s: Situation): Step {
  if (s.open && s.state) {
    return step("connected", "done", "LispM's state is here");
  }
  if (s.open) {
    return step("connected", "active", "Waiting for LispM's state", "busy");
  }
  return step("connected", "pending", s.flow === "pair" ? "Paired with LispM" : "Back with LispM");
}

/** The steps of `s.flow`, as `s` has them. */
export function steps(s: Situation): Step[] {
  return s.flow === "pair"
    ? [join(s), scan(s), reach(s), confirm(s), connected(s)]
    : [join(s), reach(s), connected(s)];
}

/** How far the steps are, from 0 to 1: the share done or skipped. */
export function progress(all: readonly Step[]): number {
  const over = all.filter((one) => one.state === "done" || one.state === "skipped").length;
  return all.length === 0 ? 0 : over / all.length;
}

/** The moment the offer `token` expires, in milliseconds since the epoch,
 * or null for a token too short to hold one.  The token is base64url of
 * version (1) ‖ channel (16) ‖ key (32) ‖ psk (32) ‖ expires (u64, big
 * endian) ‖ …; see crates/protocol/src/offer.rs. */
export function expiry(token: string): number | null {
  try {
    const base64 = token.replace(/-/g, "+").replace(/_/g, "/");
    const binary = atob(base64 + "=".repeat((4 - (base64.length % 4)) % 4));
    if (binary.length < 89) {
      return null;
    }
    let ms = 0;
    for (let i = 81; i < 89; i++) {
      ms = ms * 256 + binary.charCodeAt(i);
    }
    return ms;
  } catch {
    return null;
  }
}
