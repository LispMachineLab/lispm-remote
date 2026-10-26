// lispm-remote: the tests of when the page pairs again because its Rings
// address changed.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

// Run after `make page` (or `npm run build`) with `node --test web/test`:
// they test the compiled modules in web/dist.

import assert from "node:assert/strict";
import { test } from "node:test";
import { MOVED_NOTICE, moved } from "../dist/pairing.js";

const LISPM = "0x50e92c90d7a57bd01f4184eda98f87c382da4e61";
const HERE = "0x3bbba89641b3470b86f516a9e91862ce6b23765b";
const BEFORE = "0x1f0f8a4b1c2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f";
const KEY = "x".repeat(43);

test("a pairing made here is kept", () => {
  assert.equal(moved({ responder: LISPM, key: KEY, did: HERE }, HERE), false);
});

test("a pairing made at another address is dropped", () => {
  assert.equal(moved({ responder: LISPM, key: KEY, did: BEFORE }, HERE), true);
});

test("a pairing noted without an address was made at the old one", () => {
  // Pages before Rings 0.37 noted no address with their pairing.
  assert.equal(moved({ responder: LISPM, key: KEY }, HERE), true);
});

test("the owner is told why and what to do", () => {
  assert.match(MOVED_NOTICE, /new address/);
  assert.match(MOVED_NOTICE, /M-x lispm-remote-pair/);
  assert.match(MOVED_NOTICE, /replaces its old pairing/);
});
