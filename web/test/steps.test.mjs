// lispm-remote: the tests of the steps the page shows.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

import assert from "node:assert/strict";
import { test } from "node:test";
import { clearToken, keepToken, takeToken, tokenOf } from "../dist/scan.js";
import { clock, expiry, progress, steps } from "../dist/steps.js";

const now = 1_000_000;
const situation = (overrides = {}) => ({
  flow: "pair",
  online: true,
  peers: 0,
  joinAttempts: 0,
  nextJoinAt: 0,
  refused: false,
  seed: "node.rings.rs",
  scanning: false,
  code: null,
  phase: "idle",
  sends: 0,
  sent: false,
  unanswered: false,
  fingerprint: null,
  since: null,
  expires: null,
  open: false,
  state: false,
  now,
  ...overrides,
});
const step = (s, name) => steps(s).find((one) => one.name === name);

test("the join step counts its tries, and says when the next one is", () => {
  assert.equal(step(situation(), "join").detail, "Through node.rings.rs: joining");
  assert.equal(step(situation({ joinAttempts: 3, nextJoinAt: now + 900 }), "join").detail, "Through node.rings.rs: still trying, 3");
  assert.equal(
    step(situation({ joinAttempts: 12, nextJoinAt: now + 4_200 }), "join").detail,
    "Through node.rings.rs: still trying, 12 · again in 5 s",
  );
});

test("a seed that turns the device away says so", () => {
  const join = step(situation({ joinAttempts: 2, refused: true, nextJoinAt: now + 30_000 }), "join");
  assert.equal(join.state, "active");
  assert.equal(join.detail, "node.rings.rs turns this device away for now · again in 30 s");
});

test("joined, offline, and the code's link", () => {
  assert.equal(step(situation({ peers: 2 }), "join").state, "done");
  assert.equal(step(situation({ online: false }), "join").state, "failed");
  assert.equal(step(situation({ code: "link" }), "scan").state, "done");
  assert.equal(step(situation({ code: "link" }), "scan").detail, "Opened from the QR code");
  assert.equal(step(situation({ code: "camera" }), "scan").state, "done");
});

test("pairing goes reach, confirm with the time left, connected", () => {
  const pairing = situation({
    peers: 1, code: "camera", phase: "pairing", sends: 3, sent: true,
    fingerprint: "4F2A-91C0", since: now - 30_000, expires: now + 90_000,
  });
  assert.equal(step(pairing, "reach").state, "done");
  const confirm = step(pairing, "confirm");
  assert.equal(confirm.state, "active");
  assert.equal(confirm.bar, 0.75);
  assert.match(confirm.detail, /1:30 left/);
  const paired = situation({ ...pairing, phase: "open", open: true, state: true });
  assert.deepEqual(steps(paired).map((one) => one.state), ["done", "done", "done", "done", "done"]);
  assert.equal(progress(steps(paired)), 1);
});

test("reconnecting shows join, reach and connected only", () => {
  const names = steps(situation({ flow: "reconnect" })).map((one) => one.name);
  assert.deepEqual(names, ["join", "reach", "connected"]);
  const opening = situation({ flow: "reconnect", peers: 1, phase: "opening", sends: 2 });
  assert.equal(step(opening, "reach").detail, "Sent twice; LispM has not answered yet");
});

test("the offer's expiry is read from its token", () => {
  const bytes = new Uint8Array(90);
  new DataView(bytes.buffer).setBigUint64(81, 1_761_234_567_890n);
  const token = Buffer.from(bytes).toString("base64url");
  assert.equal(expiry(token), 1_761_234_567_890);
  assert.equal(expiry("short"), null);
  assert.equal(clock(65_000), "1:05");
});

test("the offer's token is read from URLs, hashes and query parameters", () => {
  const token = "AbCdEfGhIjKlMnOpQrStUvWxYz0123456789_-";
  assert.equal(tokenOf(token), token);
  assert.equal(tokenOf(`#${token}`), token);
  assert.equal(tokenOf(`#token=${token}`), token);
  assert.equal(tokenOf(`https://lispm.ai/remote/#${token}`), token);
  assert.equal(tokenOf(`https://lispm.ai/remote/?seed=https%3A%2F%2Fnode.rings.rs&token=${token}`), token);
  assert.equal(tokenOf("?token=short"), null);
  assert.equal(tokenOf("https://lispm.ai/remote/"), null);
});

test("takeToken reads the token from the URL, cleans the URL, and keeps it until cleared", () => {
  const bytes = new Uint8Array(90);
  new DataView(bytes.buffer).setBigUint64(81, BigInt(Date.now() + 60_000));
  const token = Buffer.from(bytes).toString("base64url");
  const storage = new Map();
  let replaced = null;

  globalThis.sessionStorage = {
    getItem: (key) => storage.get(key) ?? null,
    setItem: (key, value) => storage.set(key, String(value)),
    removeItem: (key) => storage.delete(key),
  };
  globalThis.history = {
    replaceState: (_data, _unused, url) => {
      replaced = url;
    },
  };
  globalThis.location = {
    pathname: "/remote/",
    search: "?name=Phone",
    hash: `#${token}`,
  };

  assert.equal(takeToken(), token);
  assert.equal(replaced, "/remote/?name=Phone");

  // A reload before pairing settles recovers the token from sessionStorage.
  globalThis.location.hash = "";
  replaced = null;
  assert.equal(takeToken(), token);
  assert.equal(replaced, null);

  // Once pairing settles, clearToken drops it.
  clearToken();
  assert.equal(takeToken(), null);

  // Query parameter ?token= is also supported and cleaned.
  globalThis.location.search = `?seed=https%3A%2F%2Fnode.rings.rs&token=${token}`;
  assert.equal(takeToken(), token);
  assert.equal(replaced, "/remote/?seed=https%3A%2F%2Fnode.rings.rs");
  clearToken();

  // An expired token in sessionStorage is dropped.
  new DataView(bytes.buffer).setBigUint64(81, BigInt(Date.now() - 1_000));
  const expired = Buffer.from(bytes).toString("base64url");
  keepToken(expired);
  globalThis.location.search = "";
  assert.equal(takeToken(), null);
});
