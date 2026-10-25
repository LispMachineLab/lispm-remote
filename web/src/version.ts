// lispm-remote: telling the owner when the other end runs another version.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * Two ends may run another version than this page:
 *
 * - The seed.  Rings's handshake carries no version: a seed of another
 *   Rings version only fails to read the page's offer, or the page its
 *   answer, in words `ringsMismatch` knows.  Joining again fails the same
 *   way, and a seed refuses an address that keeps trying (429), so the page
 *   leaves the seed alone for `MISMATCH_MS` and says what it concluded.
 *   Which side is older it cannot tell until Rings's handshake carries the
 *   version.
 * - LispM.  Every frame starts with the version of LispM Remote's protocol
 *   it is in; the device's machine fails with `other-version` when LispM
 *   answers in another, and the page stops there.
 *
 * Emacs says the same in its window (lispm-remote.el).  Nothing here
 * touches the network or the page, so it is tested under Node.
 */

/** How long the page leaves a seed alone that runs another Rings version. */
export const MISMATCH_MS = 30 * 60_000;

/** Whether the page still leaves the seed alone at `now`, a join through it
 * having failed for its Rings version at `since` (0 for never), both in
 * milliseconds. */
export function leftAlone(since: number, now: number): boolean {
  return since > 0 && now - since < MISMATCH_MS;
}

/** The version `text` starts with, as "0.36.0", or null.  Rings's node says
 * "0.36.0-99feaaf" when built from a checkout. */
export function versionNumber(text: unknown): string | null {
  return typeof text === "string" ? (/^v?(\d+(?:\.\d+)*)/.exec(text)?.[1] ?? null) : null;
}

/** Whether the join failure `message` says that the seed runs another Rings
 * version: it cannot read the page's offer ("Answer offer info failed:
 * Codec deserialization error"), or the page cannot read its answer
 * ("Decode error"). */
export function ringsMismatch(message: string | null): boolean {
  return message !== null && /Codec deserialization error|\bDecode error\b/.test(message);
}

/** What the page concludes from the seed `seed` running another Rings
 * version than the page's own, `ours` when known, and what to do.  Which
 * side is older the seed does not say. */
export function seedConclusion(seed: string, ours: string | null): string {
  return ours === null
    ? `${seed} runs another Rings version than this page. Reload the page for its update, or try another seed (?seed=).`
    : `${seed} runs another Rings version than this page (${ours}). Reload the page for its update, or try a seed of Rings ${ours} (?seed=).`;
}

/** What the page concludes from LispM speaking `theirs` of LispM Remote's
 * protocol, and this page `ours`: which is older, and what to do. */
export function peerConclusion(theirs: number, ours: number): string {
  return theirs < ours
    ? `LispM Remote in Emacs is older than this page (protocol ${theirs}; this page speaks ${ours}). Update LispM Remote in Emacs, then reload.`
    : `This page is older than LispM Remote in Emacs (protocol ${ours}; Emacs speaks ${theirs}). Reload the page.`;
}
