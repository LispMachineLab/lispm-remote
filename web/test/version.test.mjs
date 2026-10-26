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
  ringsOlder,
  seedConclusion,
  versionNumber,
} from "../dist/version.js";

// Why LispM Remote 0.1.0, on Rings 0.34.2, could not join through
// node.rings.rs once it ran Rings 0.36.0, word for word; the module's test
// `a_seed_of_another_rings_version_cannot_read_the_offer` holds Rings to
// the words that tell it.
const MISMATCH =
  "Internal rpc services error: Server error: Connect remote rpc server failed: Server returned rpc error Server error: Answer offer info failed: Codec deserialization error...";

// Why a page of LispM Remote 0.2, on Rings 0.36, cannot join through a seed
// whose handshake carries a wire version (RingsNetwork/rings#1106), word for
// word.  Rings 0.36 relays the seed's words, so their "ours" is the seed's;
// LispM Remote 0.2's module tests held Rings 0.36 to them.
const REFUSED =
  "Internal rpc services error: Server error: Connect remote rpc server failed: Server returned rpc error Server error: Incompatible wire version: ours is 1, the peer's is from before wire versioning...";

// Why this page, on Rings 0.37, refuses a seed of a later wire version and
// one from before wire versioning, word for word, from the page's side.
// The module's tests `a_seed_of_a_later_wire_version_is_refused_before_the_offer`
// and `a_seed_from_before_wire_versioning_is_refused_before_the_offer`
// hold Rings to them, from stand-in seeds; the browser's node passes on the
// same words, without the data.
const REFUSES_A_LATER_SEED =
  "Internal rpc services error: Server error: Incompatible wire version: ours is 1, the peer's is 2..";
const REFUSES_A_SEED_BEFORE_VERSIONING =
  "Internal rpc services error: Server error: Incompatible wire version: ours is 1, the peer's is from before wire versioning..";

test("a seed of another Rings version is told by its words", () => {
  assert.equal(ringsMismatch(MISMATCH), true);
  assert.equal(ringsMismatch(REFUSED), true);
  assert.equal(ringsMismatch(REFUSES_A_LATER_SEED), true);
  assert.equal(ringsMismatch(REFUSES_A_SEED_BEFORE_VERSIONING), true);
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
  // When the failure said which side is older.
  assert.equal(
    seedConclusion("node.rings.rs", "0.36.0", "ours"),
    "node.rings.rs runs a newer Rings than this page (0.36.0). Reload the page for its update.",
  );
  assert.equal(
    seedConclusion("node.rings.rs", "0.36.0", "seed"),
    "node.rings.rs runs an older Rings than this page (0.36.0). Try another seed (?seed=).",
  );
});

test("a refusal says which Rings is older", () => {
  // Rings 0.37 refusing a seed by its nodeDid, from the page's side.
  assert.equal(ringsOlder(REFUSES_A_LATER_SEED), "ours");
  assert.equal(ringsOlder(REFUSES_A_SEED_BEFORE_VERSIONING), "seed");
  // Rings 0.36 relaying a seed's refusal of the page, from before wire
  // versioning: the page's Rings is older.
  assert.equal(ringsOlder(REFUSED), "ours");
  const bySeed = (ours, theirs) =>
    `Internal rpc services error: Server error: Connect remote rpc server failed: Server returned rpc error Server error: Incompatible wire version: ours is ${ours}, the peer's is ${theirs}..`;
  const byPage = (ours, theirs) =>
    `Internal rpc services error: Server error: Incompatible wire version: ours is ${ours}, the peer's is ${theirs}..`;
  assert.equal(ringsOlder(bySeed(2, 1)), "ours");
  assert.equal(ringsOlder(bySeed(1, 2)), "seed");
  assert.equal(ringsOlder(byPage(2, 1)), "seed");
  assert.equal(ringsOlder(byPage(1, 2)), "ours");
  assert.equal(ringsOlder(byPage(2, 2)), null);
  assert.equal(ringsOlder(byPage(1, "from before wire versioning")), "seed");
  // A codec error says nothing of which is older.
  assert.equal(ringsOlder(MISMATCH), null);
  assert.equal(ringsOlder(null), null);
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
