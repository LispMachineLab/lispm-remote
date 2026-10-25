// lispm-remote: what the page keeps.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * The page keeps three things in the IndexedDB database "lispm-remote":
 * its Rings account ("account", a WebCrypto key pair whose private half
 * cannot be read out), its static secret ("static", hexadecimal) and its
 * pairing ("pairing", the `{responder, key}` of the machine's `paired`
 * effect).  The Rings node keeps its replay records beside them, under
 * "lispm-remote/rings", in the format "replayFormat" names.
 */

const DATABASE = "lispm-remote";
const STORE = "keys";

/** The replay record format this page's Rings writes.  Rings 0.35 changed
 * it (RingsNetwork/rings#936): a store of an earlier format decodes no
 * record, so every stream would fail closed. */
const REPLAY_FORMAT = "rings-0.35";

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

  /** Removes the replay store of the Rings node kept under `rings` when it
   * was written in another format than `REPLAY_FORMAT`, before the node
   * opens it.  Rings resets each stream's replay window then, as clearing
   * a stream does.  It is removed before the format is noted, so a page
   * closed in between removes it again. */
  async prepareReplay(rings: string): Promise<void> {
    if ((await this.get<string>("replayFormat")) === REPLAY_FORMAT) return;
    await result(indexedDB.deleteDatabase(`${rings}/transaction-replay`));
    await this.put("replayFormat", REPLAY_FORMAT);
  }
}
