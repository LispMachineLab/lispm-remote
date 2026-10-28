// lispm-remote: when the page renews its node's session key.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * The page's Rings node signs every frame with a session key the device's
 * account delegates to, and Rings keeps that delegation for the node's
 * life: from its signing horizon, 25 s before it expires, the node sends
 * nothing, and stays mute (RingsNetwork/rings#1063).  The browser node
 * cannot be closed and started again in place, so a page open that long
 * reloads: the reload delegates to a fresh key, and keeps the account, the
 * pairing and the replay records, and opens a session again, as any
 * reload does.  It reloads when nobody is looking, if it can:
 *
 *   now < renewAt                          run
 *   renewAt ≤ now, nobody looking          reload
 *   renewAt ≤ now < horizon − 1 min        warn, and reload once nobody looks
 *   horizon − 1 min ≤ now                  reload, about to go mute
 *
 * where horizon = expiry − 25 s and renewAt = horizon − 1 day, the day
 * Rings starts warning that a node nears its horizon.  Nobody looks at a
 * page that is hidden, or that was just shown again: whatever it showed is
 * stale, and it opens its session again anyway.
 */

/** How long the page's delegations last: Rings's default, 30 days. */
export const DELEGATION_TTL_MS = 30 * 24 * 60 * 60 * 1000;
/** How long before a delegation expires Rings stops signing with it: its
 * delivery bound. */
export const SIGNING_MARGIN_MS = 25_000;
/** How long before the horizon the page starts to renew. */
export const RENEWAL_LEAD_MS = 24 * 60 * 60 * 1000;
/** How long before the horizon the page reloads whoever is looking. */
export const LAST_MOMENT_MS = 60_000;

/** When a delegation is renewed, and when it would go mute, in
 * milliseconds since the epoch. */
export interface Renewal {
  readonly renewAt: number;
  readonly horizon: number;
}

/** What the page does about its delegation now. */
export type Renewing = "run" | "warn" | "reload";

/** The renewal of a delegation of `ttl` made no earlier than `madeAt`:
 * Rings stamps a delegation when its builder is made, after `madeAt`, so
 * this horizon is never later than the delegation's. */
export function renewalOf(madeAt: number, ttl = DELEGATION_TTL_MS): Renewal {
  const horizon = madeAt + ttl - SIGNING_MARGIN_MS;
  return { renewAt: horizon - RENEWAL_LEAD_MS, horizon };
}

/** What the page does at `now` about the delegation renewed as `renewal`,
 * `looking` whether its owner may be looking at it. */
export function renewing(renewal: Renewal, now: number, looking: boolean): Renewing {
  if (now < renewal.renewAt) {
    return "run";
  }
  if (!looking || now >= renewal.horizon - LAST_MOMENT_MS) {
    return "reload";
  }
  return "warn";
}

/** What the page tells its owner while it waits to reload. */
export const RENEWAL_NOTICE =
  "This page reloads soon to renew its key on Rings Network: once you switch away from it, or within a day.";
