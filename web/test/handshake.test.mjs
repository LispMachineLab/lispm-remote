// lispm-remote: the tests of when the page drops a handshake that keeps it
// off Rings.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

// Run after `make page` (or `npm run build`) with `node --test web/test`:
// they test the compiled modules in web/dist.

import assert from "node:assert/strict";
import { test } from "node:test";
import { holding, inFlight, STUCK_MS, stuck } from "../dist/handshake.js";

const SEED = "0x50e92c90d7a57bd01f4184eda98f87c382da4e61";
const OTHER = "0x3bbba89641b3470b86f516a9e91862ce6b23765b";
// As the page's node gives it, through `connect_peer_via_http`.
const refused = (peer) =>
  `Internal rpc services error: Server error: a handshake with ${peer} is already in flight.`;

test("a refusal names the peer whose handshake is in flight", () => {
  assert.equal(inFlight(refused(SEED)), SEED);
});

test("other failures name no handshake", () => {
  for (const message of [
    "Joining took too long",
    "Internal rpc services error: Server error: Found existing transport",
    "error sending request for url (http://seed.lispm.test:50077/)",
    "",
  ]) {
    assert.equal(inFlight(message), null, message);
  }
});

test("a handshake is held from its first refusal", () => {
  assert.deepEqual(holding(null, refused(SEED), 1_000), { peer: SEED, since: 1_000 });
});

test("refusals by the same peer keep the first one's time", () => {
  let held = holding(null, refused(SEED), 1_000);
  for (const now of [2_000, 3_000, 9_000]) {
    held = holding(held, refused(SEED), now);
  }
  assert.deepEqual(held, { peer: SEED, since: 1_000 });
});

test("a refusal by another peer starts over", () => {
  const held = holding(holding(null, refused(SEED), 1_000), refused(OTHER), 5_000);
  assert.deepEqual(held, { peer: OTHER, since: 5_000 });
});

test("a failure of another kind lets go of the handshake", () => {
  const held = holding(null, refused(SEED), 1_000);
  assert.equal(holding(held, "Joining took too long", 2_000), null);
});

test(`a handshake is stuck once it has refused joins for ${STUCK_MS / 1_000} s`, () => {
  const held = holding(null, refused(SEED), 1_000);
  assert.equal(stuck(held, 1_000), false);
  assert.equal(stuck(held, 1_000 + STUCK_MS - 1), false);
  assert.equal(stuck(held, 1_000 + STUCK_MS), true);
  assert.equal(stuck(null, 1_000 + STUCK_MS), false);
});

test("joins every second reach a stuck handshake within the limit", () => {
  let held = null;
  let dropped = null;
  for (let now = 0; now <= 30_000 && dropped === null; now += 1_000) {
    held = holding(held, refused(SEED), now);
    if (stuck(held, now)) {
      dropped = now;
    }
  }
  assert.equal(dropped, STUCK_MS);
});
