// lispm-remote: the phone's Rings account.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * The phone's own Rings account: a P-256 key pair made by WebCrypto, its
 * private half not extractable, so it never leaves the browser.  Rings
 * derives the phone's DID from the public half (account type
 * `secp256r1`), and the node runs on a session key this account delegates
 * to at each start, as LispM's node does with its own account.  The phone
 * never borrows LispM's account: a node's DID is its account's.
 */

import { type DelegateeKey, DelegationBuilder } from "rings-node";
import type { Store } from "./store.js";

const P256: EcKeyGenParams = { name: "ECDSA", namedCurve: "P-256" };

/** The order of P-256's base point. */
const ORDER = 0xffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc632551n;

/** The phone's account: its key pair, and the public key as Rings names it. */
export interface Account {
  readonly keys: CryptoKeyPair;
  /** The public key's coordinates x ‖ y, in hexadecimal. */
  readonly entity: string;
}

/** Hexadecimal text of `bytes`. */
export function hex(bytes: Uint8Array): string {
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

function unhex(text: string): Uint8Array {
  return Uint8Array.from(text.match(/../g) ?? [], (pair) => Number.parseInt(pair, 16));
}

/** The account kept in `store`, made and kept there the first time. */
export async function account(store: Store): Promise<Account> {
  let keys = await store.get<CryptoKeyPair>("account");
  if (!keys) {
    keys = await crypto.subtle.generateKey(P256, false, ["sign", "verify"]);
    await store.put("account", keys);
  }
  // The raw point is 04 ‖ x ‖ y.
  const point = new Uint8Array(await crypto.subtle.exportKey("raw", keys.publicKey));
  return { keys, entity: hex(point.subarray(1)) };
}

/** What a `secp256r1` account signs for Rings: a prefix, the text's length
 * in bytes, and the text. */
function prefixed(text: string): Uint8Array<ArrayBuffer> {
  const body = new TextEncoder().encode(text);
  const prefix = new TextEncoder().encode(`\x19Rings Signed Message:\n${body.length}`);
  const message = new Uint8Array(prefix.length + body.length);
  message.set(prefix);
  message.set(body, prefix.length);
  return message;
}

/** The signature r ‖ s with s ≤ n/2, the one of the pair `(r, s)`,
 * `(r, n − s)` that Rings accepts. */
export function lowS(signature: Uint8Array): Uint8Array {
  const s = BigInt(`0x${hex(signature.subarray(32, 64))}`);
  if (s <= ORDER >> 1n) {
    return signature;
  }
  const low = new Uint8Array(signature);
  low.set(unhex((ORDER - s).toString(16).padStart(64, "0")), 32);
  return low;
}

/** A fresh session key that `account` delegates to, for the node to run on. */
export async function delegatee(account: Account): Promise<DelegateeKey> {
  let builder = DelegationBuilder.new(account.entity, "secp256r1");
  const signature = await crypto.subtle.sign(
    { name: "ECDSA", hash: "SHA-256" },
    account.keys.privateKey,
    prefixed(builder.unsigned_proof()),
  );
  builder = builder.set_delegator_signature(lowS(new Uint8Array(signature)));
  return builder.build();
}
