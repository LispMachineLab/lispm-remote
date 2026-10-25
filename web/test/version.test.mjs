// lispm-remote: the tests of what the page says when the other end runs
// another version.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

// Run after `make page` (or `npm run build`) with `node --test web/test`:
// they test the compiled modules in web/dist.

import assert from "node:assert/strict";
import { test } from "node:test";
import { steps } from "../dist/steps.js";
import {
  leftAlone,
  MISMATCH_MS,
  peerConclusion,
  ringsMismatch,
  seedConclusion,
  versionNumber,
} from "../dist/version.js";

// Why LispM Remote 0.1.0, on Rings 0.34.2, could not join through
// node.rings.rs once it ran Rings 0.36.0, word for word; the module's test
// `a_seed_of_another_rings_version_cannot_read_the_offer` holds Rings to
// the words that tell it.
const MISMATCH =
  "Internal rpc services error: Server error: Connect remote rpc server failed: Server returned rpc error Server error: Answer offer info failed: Codec deserialization error...";

test("a seed of another Rings version is told by its words", () => {
  assert.equal(ringsMismatch(MISMATCH), true);
  assert.equal(ringsMismatch("Internal rpc services error: Server error: Decode error.."), true);
  for (const other of [
    "Internal rpc services error: Server error: a handshake with 0x50e92c90d7a57bd01f4184eda98f87c382da4e61 is already in flight.",
    "Internal rpc services error: Server error: Found existing transport",
    "error sending request for url (https://node.rings.rs/)",
    "Joining took too long",
    "429 Too Many Requests",
    "",
  ]) {
    assert.equal(ringsMismatch(other), false, other);
  }
  assert.equal(ringsMismatch(null), false);
});

test("the node's version is read without its commit", () => {
  assert.equal(versionNumber("0.36.0-99feaaf"), "0.36.0");
  assert.equal(versionNumber("0.36.0"), "0.36.0");
  assert.equal(versionNumber("unknown"), null);
  assert.equal(versionNumber(undefined), null);
});

test("the seed's conclusion names this page's Rings", () => {
  assert.equal(
    seedConclusion("node.rings.rs", "0.36.0"),
    "node.rings.rs runs another Rings version than this page (0.36.0). Reload the page for its update, or try a seed of Rings 0.36.0 (?seed=).",
  );
  // Before the node said which Rings it runs.
  assert.equal(
    seedConclusion("node.rings.rs", null),
    "node.rings.rs runs another Rings version than this page. Reload the page for its update, or try another seed (?seed=).",
  );
});

test("LispM's conclusion says which side is older", () => {
  assert.equal(
    peerConclusion(1, 2),
    "LispM Remote in Emacs is older than this page (protocol 1; this page speaks 2). Update LispM Remote in Emacs, then reload.",
  );
  assert.equal(
    peerConclusion(2, 1),
    "This page is older than LispM Remote in Emacs (protocol 1; Emacs speaks 2). Reload the page.",
  );
});

test(`a seed of another Rings version is left alone for ${MISMATCH_MS / 60_000} minutes`, () => {
  const since = 1_000_000;
  assert.equal(leftAlone(0, since), false);
  assert.equal(leftAlone(since, since), true);
  assert.equal(leftAlone(since, since + MISMATCH_MS - 1), true);
  assert.equal(leftAlone(since, since + MISMATCH_MS), false);
  // Joining every second, as the page does without a peer, asks the seed
  // once in that time instead of 1,800 times.
  let joins = 0;
  let failedAt = 0;
  for (let now = since; now < since + MISMATCH_MS; now += 1_000) {
    if (!leftAlone(failedAt, now)) {
      joins++;
      failedAt = now;
    }
  }
  assert.equal(joins, 1);
});

/** The steps of reconnecting, as the page knows it now. */
function reconnecting(situation) {
  return steps({
    flow: "reconnect",
    online: true,
    peers: 0,
    joinAttempts: 1,
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
    now: 0,
    ...situation,
  });
}

test("the Join step fails with the seed's conclusion", () => {
  const conclusion = seedConclusion("node.rings.rs", "0.36.0");
  const [join, reach] = reconnecting({ seedProblem: conclusion });
  assert.equal(join.state, "failed");
  assert.equal(join.detail, conclusion);
  assert.equal(reach.state, "pending");
  // Without one, the step keeps trying, as before.
  assert.equal(reconnecting({}).at(0).state, "active");
  assert.equal(reconnecting({ seedProblem: null }).at(0).state, "active");
});

test("the Reach step fails with LispM's conclusion", () => {
  const conclusion = peerConclusion(1, 2);
  const [join, reach, connected] = reconnecting({ peers: 3, peerProblem: conclusion });
  assert.equal(join.state, "done");
  assert.equal(reach.state, "failed");
  assert.equal(reach.detail, conclusion);
  assert.equal(connected.state, "pending");
});
