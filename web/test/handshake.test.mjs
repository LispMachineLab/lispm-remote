// lispm-remote: the tests of when the page drops a handshake that keeps it
// off Rings.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

// Run after `make page` (or `npm run build`) with `node --test web/test`:
// they test the compiled modules in web/dist.

import assert from "node:assert/strict";
import { test } from "node:test";
import {
  gatherSettleDelay,
  HOST_SETTLE_MS,
  holding,
  inFlight,
  REFLEXIVE_SETTLE_MS,
  STUCK_MS,
  settleIceGathering,
  stuck,
} from "../dist/handshake.js";

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

test("gatherSettleDelay distinguishes IPv4 reflexive, IPv6 reflexive, host, and empty text", () => {
  assert.equal(
    gatherSettleDelay("candidate:1 1 udp 1677729535 14.155.204.30 51628 typ srflx raddr 0.0.0.0 rport 0"),
    REFLEXIVE_SETTLE_MS,
  );
  assert.equal(
    gatherSettleDelay("candidate:2 1 udp 16777215 203.0.113.5 54000 typ relay raddr 0.0.0.0 rport 0"),
    REFLEXIVE_SETTLE_MS,
  );
  assert.equal(
    gatherSettleDelay("candidate:2 1 udp 1677729535 240e:3b0:1234::1 54000 typ srflx raddr :: rport 0"),
    HOST_SETTLE_MS,
  );
  assert.equal(
    gatherSettleDelay("candidate:3 1 udp 2113937151 test.local 56555 typ host"),
    HOST_SETTLE_MS,
  );
  assert.equal(gatherSettleDelay("v=0\r\na=candidate:3 1 udp 2113937151 10.0.0.2 56555 typ host\r\n"), HOST_SETTLE_MS);
  assert.equal(gatherSettleDelay("v=0\r\ns=-\r\n"), null);
});

test("settleIceGathering completes WebKit-style gathering once srflx candidates settle", async () => {
  class FakePeerConnection extends EventTarget {
    #state = "new";
    localDescription = null;
    onicegatheringstatechange = null;

    get iceGatheringState() {
      return this.#state;
    }

    dispatchEvent(event) {
      if (event.type === "icegatheringstatechange" && typeof this.onicegatheringstatechange === "function") {
        this.onicegatheringstatechange(event);
      }
      return super.dispatchEvent(event);
    }

    async setLocalDescription(description) {
      this.#state = "gathering";
      this.localDescription = description ?? { type: "offer", sdp: "v=0\r\n" };
    }

    close() {
      this.#state = "closed";
    }
  }

  const scope = { RTCPeerConnection: FakePeerConnection };
  settleIceGathering(scope);
  // Idempotent when called a second time.
  const Wrapped = scope.RTCPeerConnection;
  settleIceGathering(scope);
  assert.equal(scope.RTCPeerConnection, Wrapped);

  const pc = new scope.RTCPeerConnection();
  let notified = 0;
  pc.onicegatheringstatechange = () => {
    if (pc.iceGatheringState === "complete") {
      notified++;
    }
  };
  await pc.setLocalDescription({ type: "offer", sdp: "v=0\r\n" });
  assert.equal(pc.iceGatheringState, "gathering");

  const event = new Event("icecandidate");
  event.candidate = {
    candidate: "candidate:1 1 udp 1677729535 14.155.204.30 51628 typ srflx raddr 0.0.0.0 rport 0",
  };
  pc.dispatchEvent(event);

  await new Promise((resolve) => setTimeout(resolve, REFLEXIVE_SETTLE_MS + 80));
  assert.equal(pc.iceGatheringState, "complete");
  assert.equal(notified, 1);
  pc.close();
});
