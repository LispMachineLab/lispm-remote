// lispm-remote: the tests of when the page renews its session key.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

// Run after `make page` (or `npm run build`) with `node --test web/test`:
// they test the compiled modules in web/dist.

import assert from "node:assert/strict";
import { test } from "node:test";
import {
  DELEGATION_TTL_MS,
  LAST_MOMENT_MS,
  RENEWAL_LEAD_MS,
  SIGNING_MARGIN_MS,
  renewalOf,
  renewing,
} from "../dist/renewal.js";

const madeAt = Date.UTC(2026, 9, 28);
const day = 24 * 60 * 60 * 1000;

test("a delegation of 30 days goes mute 25 s before it expires, and is renewed a day before", () => {
  const renewal = renewalOf(madeAt);
  assert.equal(DELEGATION_TTL_MS, 30 * day);
  assert.equal(renewal.horizon, madeAt + 30 * day - SIGNING_MARGIN_MS);
  assert.equal(renewal.renewAt, renewal.horizon - RENEWAL_LEAD_MS);
  assert.equal(RENEWAL_LEAD_MS, day);
});

test("before its renewal, the page runs, looked at or not", () => {
  const renewal = renewalOf(madeAt);
  for (const now of [madeAt, madeAt + 28 * day, renewal.renewAt - 1]) {
    assert.equal(renewing(renewal, now, true), "run");
    assert.equal(renewing(renewal, now, false), "run");
  }
});

test("from its renewal on, a page nobody looks at reloads", () => {
  const renewal = renewalOf(madeAt);
  for (const now of [renewal.renewAt, renewal.renewAt + day / 2, renewal.horizon + 1]) {
    assert.equal(renewing(renewal, now, false), "reload");
  }
});

test("a page looked at warns, until a minute before it would go mute", () => {
  const renewal = renewalOf(madeAt);
  const last = renewal.horizon - LAST_MOMENT_MS;
  assert.equal(renewing(renewal, renewal.renewAt, true), "warn");
  assert.equal(renewing(renewal, last - 1, true), "warn");
  assert.equal(renewing(renewal, last, true), "reload");
  // A page left open past its horizon, as one frozen in the background,
  // reloads when it runs again.
  assert.equal(renewing(renewal, renewal.horizon + day, true), "reload");
});

test("the states never go back as time passes", () => {
  const renewal = renewalOf(madeAt, 2 * day);
  const rank = { run: 0, warn: 1, reload: 2 };
  for (const looking of [true, false]) {
    let before = 0;
    for (let now = madeAt; now <= renewal.horizon + day; now += 60_000) {
      const state = rank[renewing(renewal, now, looking)];
      assert.ok(state >= before, `at ${now - madeAt} ms`);
      before = state;
    }
  }
});
