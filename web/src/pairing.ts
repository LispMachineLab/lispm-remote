// lispm-remote: a pairing made at another Rings address.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * LispM keeps a paired device under its Rings address and binds that
 * address into every session's handshake, so a device whose address
 * changed can no longer open a session.  Rings 0.37 changed the address
 * of every browser account (RingsNetwork/rings#1072): the page's account
 * is a WebCrypto P-256 key, which Rings 0.36 read with its y-coordinate
 * negated.  So the page notes its address with its pairing and, when the
 * two differ, pairs again rather than knock on LispM's door unanswered.
 * A pairing noted without an address was made before the page noted it,
 * under Rings 0.36, at the old address.
 *
 * The device keeps its key, so LispM replaces the old record with the new
 * one instead of keeping both.
 */

/** The LispM this device paired with, as the machine's `paired` effect
 * gave it, and the device's own Rings address when it paired. */
export interface Pairing {
  readonly responder: string;
  readonly key: string;
  readonly did?: string;
}

/** Whether `pairing` was made at another address than `did`. */
export function moved(pairing: Pairing, did: string): boolean {
  return pairing.did !== did;
}

/** What the owner is told when the pairing was made at another address. */
export const MOVED_NOTICE =
  "Rings gave this device a new address, so LispM no longer knows it. " +
  "In Emacs, run M-x lispm-remote-pair and scan the new code: " +
  "this device then replaces its old pairing.";
