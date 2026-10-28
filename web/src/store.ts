// lispm-remote: what the page keeps.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * The page keeps three things in the IndexedDB database "lispm-remote":
 * its Rings account ("account", a WebCrypto key pair whose private half
 * cannot be read out), its static secret ("static", hexadecimal) and its
 * pairing ("pairing", the `{responder, key}` of the machine's `paired`
 * effect, and the page's own address then).  The Rings node keeps its
 * stores beside them, under "lispm-remote/rings", written by the Rings
 * "replayFormat" names.
 */

const DATABASE = "lispm-remote";
const STORE = "keys";

/** The Rings whose stores this page writes.  A store an earlier Rings
 * wrote may not decode: Rings 0.35 changed the replay record format
 * (RingsNetwork/rings#936), Rings 0.37 asks every store to be removed
 * (RingsNetwork/rings#1072), and Rings 0.38 changes the replay record
 * format again (RingsNetwork/rings#1059). */
const REPLAY_FORMAT = "rings-0.38";

/** The databases Rings's browser node keeps under its storage name. */
const RINGS_DATABASES = [
  "",
  "/relay-inbox",
  "/dht-removals",
  "/transaction-replay",
  "/onion-entry-guards",
  "/measure",
  "/provisional-evidence",
];

/** What the store holds. */
export type Key = "account" | "static" | "pairing" | "replayFormat";

function result<T>(request: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

function committed(transaction: IDBTransaction): Promise<void> {
  return new Promise((resolve, reject) => {
    transaction.oncomplete = () => resolve();
    transaction.onerror = () => reject(transaction.error);
    transaction.onabort = () => reject(transaction.error);
  });
}

/** The page's keys and pairing. */
export class Store {
  private constructor(private readonly db: IDBDatabase) {}

  /** Opens the store, asking the browser to keep it when space runs low. */
  static async open(): Promise<Store> {
    const opening = indexedDB.open(DATABASE, 1);
    opening.onupgradeneeded = () => opening.result.createObjectStore(STORE);
    const db = await result(opening);
    // Losing the replay records would get the device refused by Rings, and
    // losing the keys would unpair it.
    void navigator.storage?.persist?.();
    return new Store(db);
  }

  /** The value kept under `key`, if any. */
  async get<T>(key: Key): Promise<T | undefined> {
    const store = this.db.transaction(STORE, "readonly").objectStore(STORE);
    return (await result(store.get(key))) as T | undefined;
  }

  /** Keeps `value` under `key`, once it is written. */
  async put(key: Key, value: unknown): Promise<void> {
    const transaction = this.db.transaction(STORE, "readwrite");
    transaction.objectStore(STORE).put(value, key);
    await committed(transaction);
  }

  /** Forgets what is kept under `key`. */
  async delete(key: Key): Promise<void> {
    const transaction = this.db.transaction(STORE, "readwrite");
    transaction.objectStore(STORE).delete(key);
    await committed(transaction);
  }

  /** Removes the stores of the Rings node kept under `rings` when another
   * Rings than `REPLAY_FORMAT` wrote them, before the node opens them.
   * Rings resets each stream's replay window then, as clearing a stream
   * does.  They are removed before the format is noted, so a page closed in
   * between removes them again. */
  async prepareRings(rings: string): Promise<void> {
    if ((await this.get<string>("replayFormat")) === REPLAY_FORMAT) return;
    for (const suffix of RINGS_DATABASES) {
      await result(indexedDB.deleteDatabase(`${rings}${suffix}`));
    }
    await this.put("replayFormat", REPLAY_FORMAT);
  }
}
