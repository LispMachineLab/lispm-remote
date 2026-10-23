// lispm-remote: the page at lispm.ai/remote.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * What the owner sees on the device: how the connection stands, pairing
 * (the camera, then the fingerprint to compare), and LispM's agents, each
 * a chat (chat.ts).  The page takes from its URL
 *
 *   #TOKEN       an offer, as LispM's QR code opens the page (then removed)
 *   ?seed=URL    the seed node to join Rings through (https://node.rings.rs)
 *   ?ice=LIST    ICE servers, as Rings writes them
 *   ?name=TEXT   what LispM's owner sees this device called
 *   ?debug       the page's state as `window.lispmRemote`, for tests
 */

import { chatting, showAgents } from "./chat.js";
import { button, byId, h, rebuild } from "./dom.js";
import { messageOf, type Options, Remote } from "./remote.js";
import { scan, takeToken } from "./scan.js";

const DEFAULT_SEED = "https://node.rings.rs";
const DEFAULT_ICE = "stun://stun.l.google.com:19302";

/** A short name for this browser and system, as "Safari on iPhone". */
function deviceName(): string {
  const agent = navigator.userAgent;
  const system = /iPhone/.test(agent)
    ? "iPhone"
    : /iPad/.test(agent)
      ? "iPad"
      : /Android/.test(agent)
        ? "Android"
        : /Mac OS X/.test(agent)
          ? "Mac"
          : /Windows/.test(agent)
            ? "Windows"
            : /Linux/.test(agent)
              ? "Linux"
              : "a browser";
  const browser = /EdgA?\//.test(agent)
    ? "Edge"
    : /Firefox\/|FxiOS/.test(agent)
      ? "Firefox"
      : /Chrome\/|CriOS/.test(agent)
        ? "Chrome"
        : /Safari\//.test(agent)
          ? "Safari"
          : "";
  return browser ? `${browser} on ${system}` : system;
}

function options(): Options {
  const query = new URLSearchParams(location.search);
  return {
    seed: query.get("seed") || DEFAULT_SEED,
    ice: query.get("ice") || DEFAULT_ICE,
    // A device name is at most 64 bytes.
    name: (query.get("name") || deviceName()).slice(0, 48),
  };
}

// ----------------------------------------------------------------------
// Elements

const statusLine = byId<HTMLParagraphElement>("status");
const noticeLine = byId<HTMLParagraphElement>("notice");
const pairSection = byId<HTMLElement>("pair");
const footer = byId<HTMLElement>("about");

let remote: Remote | null = null;
let scanning: AbortController | null = null;

function showStatus(tone: string, text: string): void {
  statusLine.className = `status ${tone}`;
  statusLine.textContent = text;
}

// ----------------------------------------------------------------------
// Pairing

function startScan(): void {
  scanning?.abort();
  const controller = new AbortController();
  scanning = controller;
  render();
  const video = pairSection.querySelector("video");
  if (!video) {
    return;
  }
  scan(video, controller.signal).then(
    (token) => {
      scanning = null;
      remote?.pairWith(token);
      render();
    },
    (error: unknown) => {
      if (scanning === controller) {
        scanning = null;
        if (remote && !(error instanceof DOMException && error.name === "AbortError")) {
          remote.notice = { tone: "error", text: `The camera could not be used: ${messageOf(error)}` };
        }
        render();
      }
    },
  );
}

function stopScan(): void {
  scanning?.abort();
  scanning = null;
  render();
}

function renderPair(current: Remote): void {
  pairSection.hidden = false;
  if (scanning) {
    rebuild(pairSection, "scan", () => {
      const video = h("video");
      // Inline and silent, or a phone will not play it by itself.
      video.muted = true;
      video.playsInline = true;
      return [h("h2", {}, "Scan LispM's code"), video, button("Cancel", stopScan, "quiet")];
    });
  } else if (current.phase === "pairing" && current.fingerprint) {
    const fingerprint = current.fingerprint;
    rebuild(pairSection, `fingerprint ${fingerprint}`, () => [
      h("h2", {}, "Compare the fingerprint"),
      h("p", { class: "fingerprint" }, fingerprint),
      h("p", {}, "If Emacs shows the same, press a there to approve this device."),
    ]);
  } else if (current.waitingToPair) {
    rebuild(pairSection, "waiting", () => [
      h("h2", {}, "Pairing"),
      h("p", {}, "This device pairs with LispM once it is on Rings Network."),
    ]);
  } else if (!current.pairing && current.phase !== "pairing") {
    rebuild(pairSection, "unpaired", () => [
      h("h2", {}, "Pair with LispM"),
      h("p", {}, "In Emacs, run M-x lispm-remote-pair, then scan the code it shows."),
      button("Scan the code", startScan, "primary"),
    ]);
  } else {
    pairSection.hidden = true;
    rebuild(pairSection, "", () => []);
  }
}

// ----------------------------------------------------------------------
// The page

function renderAbout(current: Remote): void {
  const joining = current.peers === 0 ? current.joinError : null;
  const key = JSON.stringify([current.state?.emacs, current.did, joining, Boolean(current.pairing)]);
  footer.hidden = chatting();
  rebuild(footer, key, () => [
    current.state?.emacs && h("span", {}, `Emacs ${current.state.emacs}`),
    h("span", { class: "did" }, `This device: ${current.did}`),
    joining && h("span", {}, `Joining: ${joining}`),
    current.pairing && button("Pair with another LispM", startScan, "link"),
    current.pairing &&
      button(
        "Forget this LispM",
        () => {
          if (confirm("Forget this LispM on this device? To pair again, you will scan a new code.")) {
            void current.forget();
          }
        },
        "link",
      ),
  ]);
}

function draw(current: Remote): void {
  const status = current.status();
  showStatus(status.tone, status.text);
  noticeLine.className = `notice ${current.notice?.tone ?? ""}`;
  noticeLine.textContent = current.notice?.text ?? "";
  renderPair(current);
  showAgents(current, render);
  renderAbout(current);
}

let drawing = false;
let stale = false;

/** Draws the page as it stands.  What drawing changes, such as the agent
 * the page follows, draws it once more, after this draw, never inside it. */
function render(): void {
  const current = remote;
  if (!current) {
    return;
  }
  if (drawing) {
    stale = true;
    return;
  }
  drawing = true;
  try {
    for (let pass = 0; pass < 3; pass++) {
      stale = false;
      draw(current);
      if (!stale) {
        break;
      }
    }
  } finally {
    drawing = false;
  }
}

async function main(): Promise<void> {
  // First of all, so the token leaves the URL and the history at once.
  const token = takeToken();
  showStatus("busy", "Starting…");
  try {
    remote = await Remote.start(options(), render);
  } catch (error) {
    showStatus("bad", "LispM Remote could not start");
    noticeLine.className = "notice error";
    noticeLine.textContent = messageOf(error);
    console.error(error);
    return;
  }
  if (new URLSearchParams(location.search).has("debug")) {
    (globalThis as { lispmRemote?: Remote }).lispmRemote = remote;
  }
  if (token) {
    remote.pairWith(token);
  }
  render();
}

void main();
