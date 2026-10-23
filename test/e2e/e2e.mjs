// lispm-remote: the browser test of the page.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

// Run by run.sh inside a network namespace, with LispM's Emacs (lispm.el)
// outside it.  The page is served at $E2E_PAGE; it joins Rings through the
// seed $E2E_SEED, a name Chromium maps to $E2E_SEED_IP.  The camera is
// Chromium's fake one, playing a picture of the QR code Emacs shows.
//
// 1. The page scans the code; both ends show the same fingerprint; Emacs
//    approves it; the page shows LispM's agent.
// 2. The device tells the agent something and sees its answer.
// 3. The network goes down ($E2E_IFACE, cut with `ip link`), LispM's state
//    changes meanwhile, the network comes back: the page joins Rings again
//    and shows the new state, without scanning.
// 4. Reloaded, the page opens a session from what it kept.

import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, renameSync, unlinkSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { chromium } from "playwright";
import { pbmToY4m } from "./camera.mjs";

const dir = process.env.E2E_DIR;
const pageUrl = process.env.E2E_PAGE ?? "http://127.0.0.1:8000/";
const seed = process.env.E2E_SEED;
const seedIp = process.env.E2E_SEED_IP;
const iface = process.env.E2E_IFACE;
const seedHost = new URL(seed).hostname;

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function log(...parts) {
  console.log(`[${new Date().toISOString().slice(11, 19)}]`, ...parts);
}

/** Evaluates FORM in LispM's Emacs and returns its value, printed. */
async function lisp(form) {
  const out = join(dir, "eval.out");
  if (existsSync(out)) {
    unlinkSync(out);
  }
  writeFileSync(join(dir, "eval.tmp.el"), form);
  renameSync(join(dir, "eval.tmp.el"), join(dir, "eval.el"));
  for (let i = 0; i < 300; i++) {
    if (existsSync(out)) {
      const value = readFileSync(out, "utf8");
      unlinkSync(out);
      if (value.startsWith("(error ")) {
        throw new Error(`Emacs: ${value}`);
      }
      return value;
    }
    await sleep(100);
  }
  throw new Error(`Emacs did not answer ${form}`);
}

/** Cuts the namespace's link to LispM, or restores it. */
function link(up) {
  execFileSync("sudo", ["-n", "ip", "link", "set", iface, up ? "up" : "down"]);
  log(`network ${up ? "up" : "down"}`);
}

async function main() {
  // The code Emacs shows, as the camera sees it.
  await lisp(`(progn (lispm-remote-pair) (lispm-remote-write-qr ${JSON.stringify(join(dir, "qr.pbm"))} 6) t)`);
  const camera = join(dir, "qr.y4m");
  writeFileSync(camera, pbmToY4m(readFileSync(join(dir, "qr.pbm"), "ascii")));

  const browser = await chromium.launch({
    args: [
      "--use-fake-ui-for-media-stream",
      "--use-fake-device-for-media-stream",
      `--use-file-for-fake-video-capture=${camera}`,
      `--host-resolver-rules=MAP ${seedHost} ${seedIp}`,
      // Show the page's own address to WebRTC, not an mDNS name LispM's
      // node cannot resolve.
      "--disable-features=WebRtcHideLocalIpsWithMdns",
    ],
  });
  const context = await browser.newContext({ viewport: { width: 390, height: 844 } });
  await context.grantPermissions(["camera"]);
  const page = await context.newPage();
  page.on("console", (message) => {
    if (message.type() === "error") {
      log("page:", message.text());
    }
  });
  const status = page.locator("#status");
  const url = `${pageUrl}?seed=${encodeURIComponent(seed)}&name=E2E%20Device&debug`;

  try {
    // 1. Scan, compare, approve.
    await page.goto(url);
    await page.getByRole("button", { name: "Scan the code" }).click();
    const shown = (await page.locator("#pair .fingerprint").textContent({ timeout: 120_000 }))?.trim();
    log("the page shows", shown);
    let asked = "nil";
    for (let i = 0; i < 100 && asked === "nil"; i++) {
      asked = await lisp("(plist-get lispm-remote--offer :fingerprint)");
      await sleep(200);
    }
    assert.equal(asked, JSON.stringify(shown), "Emacs asks about the fingerprint the page shows");
    await lisp(`(progn (lispm-remote-approve ${JSON.stringify(shown)}) t)`);
    await page.locator(".chat-head h2", { hasText: "coder" }).waitFor({ timeout: 60_000 });
    await status.filter({ hasText: "Connected" }).waitFor({ timeout: 60_000 });
    assert.equal(await lisp("(length (lispm-remote--devices))"), "1");
    log("paired; the chat with coder is open");

    // 2. A message and its answer.
    const box = page.getByPlaceholder("Message coder");
    await box.fill("hello from the e2e test");
    await box.press("Enter");
    await page
      .locator(".message.theirs .bubble", { hasText: "Heard: hello from the e2e test" })
      .waitFor({ timeout: 60_000 });
    log("the agent answered");

    // 3. The network drops; LispM moves on; the network comes back.
    link(false);
    await sleep(8_000);
    await lisp(`(e2e-status "working while the device was away")`);
    await sleep(4_000);
    link(true);
    await page
      .locator(".chat-head .activity", { hasText: "working while the device was away" })
      .waitFor({ timeout: 120_000 });
    await status.filter({ hasText: "Connected" }).waitFor({ timeout: 60_000 });
    log("back online, with the new state");

    // 4. Reloaded, it reconnects from what it kept, without scanning.
    await lisp(`(e2e-status "after the reload")`);
    await page.reload();
    await page
      .locator(".chat-head .activity", { hasText: "after the reload" })
      .waitFor({ timeout: 120_000 });
    assert.equal(await page.locator("#pair").isHidden(), true);
    log("reloaded and reconnected");
  } catch (error) {
    await page.screenshot({ path: join(dir, "failure.png"), fullPage: true });
    log("status:", await status.textContent(), "| footer:", await page.locator("#about").textContent());
    throw error;
  } finally {
    await browser.close();
  }
}

main().then(
  () => log("passed"),
  (error) => {
    console.error(error);
    process.exit(1);
  },
);
