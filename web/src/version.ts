// lispm-remote: telling the owner when the other end runs another version.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * Two ends may run another version than this page:
 *
 * - The seed.  This Rings's handshake carries no version: a seed of another
 *   Rings version only fails to read the page's offer, or the page its
 *   answer.  A seed whose handshake carries a wire version
 *   (RingsNetwork/rings#1106) refuses the page by name instead, and its
 *   words say which side is older (`ringsOlder`).  Joining again fails the
 *   same way, and a seed refuses an address that keeps trying (429), so the
 *   page leaves the seed alone for `MISMATCH_MS` and says what it
 *   concluded.
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
 * Codec deserialization error"), the page cannot read its answer ("Decode
 * error"), or a seed with a wire version refuses one from before wire
 * versioning ("Incompatible wire version"). */
export function ringsMismatch(message: string | null): boolean {
  return (
    message !== null &&
    /Codec deserialization error|\bDecode error\b|Incompatible wire version/.test(message)
  );
}

/** The side whose Rings the join failure `message` says is older.
 *
 * A node with a wire version names two in its refusal, "Incompatible wire
 * version: ours is 1, the peer's is from before wire versioning": its own,
 * and the other end's, a number or, oldest of all, from before wire
 * versioning.  The refusal is the seed's when it comes after "Server
 * returned rpc error", as this Rings words a seed's answer, and the page's
 * own otherwise. */
export function ringsOlder(message: string | null): "ours" | "seed" | null {
  const refusal =
    /Incompatible wire version: ours is (\d+), the peer's is (\d+|from before wire versioning)/.exec(
      message ?? "",
    );
  if (!refusal) {
    return null;
  }
  const refusing = Number(refusal[1]);
  // From before wire versioning counts as 0, below every version.
  const other = /^\d+$/.test(refusal[2] ?? "") ? Number(refusal[2]) : 0;
  if (other === refusing) {
    return null;
  }
  if (/Server returned rpc error[^:]*: Incompatible wire version/.test(message ?? "")) {
    // The other end is the page.
    return other < refusing ? "ours" : "seed";
  }
  // The other end is the seed.
  return other < refusing ? "seed" : "ours";
}

/** What the page concludes from the seed `seed` running another Rings
 * version than the page's own, `ours` when known, and what to do: which
 * side is older, when the failure said (`older`). */
export function seedConclusion(
  seed: string,
  ours: string | null,
  older: "ours" | "seed" | null = null,
): string {
  const page = ours === null ? "this page" : `this page (${ours})`;
  switch (older) {
    case "ours":
      return `${seed} runs a newer Rings than ${page}. Reload the page for its update.`;
    case "seed":
      return `${seed} runs an older Rings than ${page}. Try another seed (?seed=).`;
    default:
      return ours === null
        ? `${seed} runs another Rings version than this page. Reload the page for its update, or try another seed (?seed=).`
        : `${seed} runs another Rings version than this page (${ours}). Reload the page for its update, or try a seed of Rings ${ours} (?seed=).`;
  }
}

/** What the page concludes from LispM speaking `theirs` of LispM Remote's
 * protocol, and this page `ours`: which is older, and what to do. */
export function peerConclusion(theirs: number, ours: number): string {
  return theirs < ours
    ? `LispM Remote in Emacs is older than this page (protocol ${theirs}; this page speaks ${ours}). Update LispM Remote in Emacs, then reload.`
    : `This page is older than LispM Remote in Emacs (protocol ${ours}; Emacs speaks ${theirs}). Reload the page.`;
}
