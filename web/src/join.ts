// lispm-remote: how often the page asks the seed to join Rings.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * When joining Rings through the seed fails, the page asks again, less
 * and less often:
 *
 *   failures 1–9     every 1 s   a seed holds a vanished connection for
 *                                about 5 s ("Found existing transport")
 *   failures 10–29   every 2 s
 *   failures 30–59   every 5 s
 *   failures 60–     every 15 s
 *
 * A seed that answers but not to this page, as one that rate-limits its
 * address (node.rings.rs sends 429 without CORS headers, so the browser
 * reports only a failed request), is asked every 30 s, then 60 s, then
 * every 2 minutes.  Waits of 2 s and more are a fifth longer or shorter at
 * random, so that pages do not ask in step.
 */

/** Why a join failed: the seed was not reached, or it turned the page away. */
export type JoinFailure = "network" | "refused";

/** The wait before the next join, in milliseconds, after `failures` joins
 * failed in a row, `refusals` of them refused by a seed that answered. */
export function joinDelay(failures: number, refusals: number, random: () => number = Math.random): number {
  const base =
    refusals > 0
      ? Math.min(120_000, 30_000 * 2 ** (refusals - 1))
      : failures < 10
        ? 1_000
        : failures < 30
          ? 2_000
          : failures < 60
            ? 5_000
            : 15_000;
  return base === 1_000 ? base : Math.round(base * (0.8 + 0.4 * random()));
}

/** Whether the failure `message` says the seed limits how often it is asked. */
export function rateLimited(message: string): boolean {
  return /\b429\b|too many requests|rate.?limit/i.test(message);
}

/** Whether the failure `message` is of the request itself, which a browser
 * gives alike for a network that is down and for an answer it may not show
 * the page, as one without CORS headers. */
export function unseen(message: string): boolean {
  return /error sending request|failed to fetch|networkerror|load failed/i.test(message);
}
