// lispm-remote: the tests of how often the page asks to join Rings.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

// Run after `make page` (or `npm run build`) with `node --test web/test`:
// they test the compiled modules in web/dist.

import assert from "node:assert/strict";
import { test } from "node:test";
import { joinDelay, rateLimited, unseen } from "../dist/join.js";

const middle = () => 0.5;

test("joins go every second while a seed may hold the old connection", () => {
  for (let failures = 0; failures < 10; failures++) {
    assert.equal(joinDelay(failures, 0, Math.random), 1_000);
  }
});

test("joins that keep failing are spaced out, to every 15 s", () => {
  assert.equal(joinDelay(10, 0, middle), 2_000);
  assert.equal(joinDelay(29, 0, middle), 2_000);
  assert.equal(joinDelay(30, 0, middle), 5_000);
  assert.equal(joinDelay(59, 0, middle), 5_000);
  assert.equal(joinDelay(60, 0, middle), 15_000);
  assert.equal(joinDelay(10_000, 0, middle), 15_000);
});

test("a seed that turns the page away is asked every 30 s, then 60 s, then 2 minutes", () => {
  assert.equal(joinDelay(1, 1, middle), 30_000);
  assert.equal(joinDelay(2, 2, middle), 60_000);
  assert.equal(joinDelay(3, 3, middle), 120_000);
  assert.equal(joinDelay(50, 40, middle), 120_000);
});

test("waits of 2 s and more are a fifth longer or shorter at random", () => {
  assert.equal(joinDelay(10, 0, () => 0), 1_600);
  assert.equal(joinDelay(10, 0, () => 1), 2_400);
  assert.equal(joinDelay(1, 1, () => 0), 24_000);
  assert.equal(joinDelay(1, 1, () => 1), 36_000);
});

test("a rate limit is told by its status or its words", () => {
  assert.ok(rateLimited("Server error: 429 Too Many Requests"));
  assert.ok(rateLimited("too many requests"));
  assert.ok(rateLimited("rate limit exceeded"));
  assert.ok(!rateLimited("Found existing transport when answer offer from remote node"));
});

test("a request the browser shows no answer to is told apart from the seed's own refusals", () => {
  const cors = "Internal rpc services error: Server error: Connect remote rpc server failed: Client error: error sending request..";
  assert.ok(unseen(cors));
  assert.ok(unseen("TypeError: Failed to fetch"));
  assert.ok(!unseen("Create offer info failed: Found existing transport when answer offer from remote node.."));
});
