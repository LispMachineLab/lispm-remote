// lispm-remote: a handshake of the page's node that keeps it off Rings.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * While the page's node holds a handshake with the seed that is not done,
 * Rings refuses to start another: a join fails with "a handshake with
 * 0x… is already in flight".  Most such handshakes finish in a second or
 * two.  One whose connection finds no path may never finish, and Rings
 * keeps it for three minutes (`PENDING_CONNECTION_TIMEOUT_MS`), all that
 * time refusing every join.  So the page drops a handshake that has
 * refused its joins for `STUCK_MS`, and the next join starts a new one.
 */

/** How long a handshake may refuse joins before the page drops it. */
export const STUCK_MS = 10_000;

/** A handshake refusing the page's joins: with which peer, and since when,
 * in milliseconds since the epoch. */
export interface Held {
  readonly peer: string;
  readonly since: number;
}

/** The peer of the handshake that the join failure `message` says is in
 * flight, or null when it says something else. */
export function inFlight(message: string): string | null {
  return /handshake with (0x[0-9a-f]+) is already in flight/i.exec(message)?.[1] ?? null;
}

/** The handshake refusing joins after one failed with `message` at `now`,
 * given `held`, the one that refused before: a refusal by the same peer
 * keeps the first one's time. */
export function holding(held: Held | null, message: string, now: number): Held | null {
  const peer = inFlight(message);
  if (peer === null) {
    return null;
  }
  return held?.peer === peer ? held : { peer, since: now };
}

/** Whether `held` has refused joins for long enough at `now` to be dropped. */
export function stuck(held: Held | null, now: number): held is Held {
  return held !== null && now - held.since >= STUCK_MS;
}
