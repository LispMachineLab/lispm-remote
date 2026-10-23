// lispm-remote: the page at lispm.ai/remote.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * What the owner sees on the phone: how the connection stands, pairing
 * (the camera, then the fingerprint to compare), and LispM's agents with
 * their approvals and controls.  The page takes from its URL
 *
 *   #TOKEN       an offer, as LispM's QR code opens the page (then removed)
 *   ?seed=URL    the seed node to join Rings through (https://node.rings.rs)
 *   ?ice=LIST    ICE servers, as Rings writes them
 *   ?name=TEXT   what LispM's owner sees this phone called
 *   ?debug       the page's state as `window.lispmRemote`, for tests
 */

import { type Agent, type Approval, messageOf, type Options, Remote } from "./remote.js";
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

type Child = Node | string | null | undefined | false;

function h<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  props: Partial<Record<string, string>> = {},
  ...children: Child[]
): HTMLElementTagNameMap[K] {
  const element = document.createElement(tag);
  for (const [name, value] of Object.entries(props)) {
    if (value !== undefined) {
      element.setAttribute(name, value);
    }
  }
  for (const child of children) {
    if (child) {
      element.append(child);
    }
  }
  return element;
}

function byId<T extends HTMLElement>(id: string): T {
  const element = document.getElementById(id);
  if (!element) {
    throw new Error(`The page has no #${id}.`);
  }
  return element as T;
}

const statusLine = byId<HTMLParagraphElement>("status");
const noticeLine = byId<HTMLParagraphElement>("notice");
const pairSection = byId<HTMLElement>("pair");
const agentList = byId<HTMLElement>("agents");
const footer = byId<HTMLElement>("about");

let remote: Remote | null = null;
let scanning: AbortController | null = null;

/** Fills `element` with what `build` makes, unless it shows `key` already:
 * a button replaced at every render could lose the tap meant for it. */
function rebuild(element: HTMLElement, key: string, build: () => Child[]): void {
  if (element.dataset.key !== key) {
    element.dataset.key = key;
    element.replaceChildren(...build().filter((child): child is Node | string => Boolean(child)));
  }
}

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

function button(text: string, action: () => void, kind = ""): HTMLButtonElement {
  const element = h("button", { type: "button", class: kind || undefined }, text);
  element.addEventListener("click", action);
  return element;
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
      h("p", {}, "If Emacs shows the same, press a there to approve this phone."),
    ]);
  } else if (current.waitingToPair) {
    rebuild(pairSection, "waiting", () => [
      h("h2", {}, "Pairing"),
      h("p", {}, "The phone pairs with LispM once it is on Rings Network."),
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
// Agents

/** An agent's card, kept across updates so a half-typed message stays. */
class Card {
  readonly element: HTMLElement;
  private readonly title = h("h2");
  private readonly badge = h("span", { class: "badge" });
  private readonly project = h("p", { class: "project" });
  private readonly status = h("p", { class: "activity" });
  private readonly approvals = h("div", { class: "approvals" });
  private readonly input = h("input", {
    type: "text",
    enterkeyhint: "send",
    autocomplete: "off",
  });
  private readonly stop = button("Stop", () => remote?.interrupt(this.name), "danger");

  constructor(private name: string) {
    const form = h("form", { class: "say" }, this.input, h("button", { type: "submit" }, "Send"));
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      const text = this.input.value.trim();
      if (text) {
        remote?.say(this.name, text);
        this.input.value = "";
      }
    });
    this.element = h(
      "article",
      { class: "agent" },
      h("header", {}, this.title, this.badge),
      this.project,
      this.status,
      this.approvals,
      form,
      h("div", { class: "controls" }, this.stop),
    );
  }

  update(agent: Agent): void {
    this.name = agent.name;
    const state = agent.state || "idle";
    this.title.textContent = agent.name;
    this.badge.textContent = state;
    this.badge.className = `badge ${state}`;
    this.project.textContent = agent.project || "";
    this.project.hidden = !agent.project;
    this.status.textContent = agent.status || "";
    this.status.hidden = !agent.status;
    this.input.placeholder = `Tell ${agent.name}…`;
    this.stop.hidden = state === "idle";
    const approvals = agent.approvals ?? [];
    rebuild(this.approvals, JSON.stringify(approvals), () =>
      approvals.map((approval) => this.approval(approval)),
    );
  }

  private approval(approval: Approval): HTMLElement {
    const decide = (decision: "allow" | "always" | "deny") => () =>
      remote?.decide(this.name, approval.call, decision);
    return h(
      "div",
      { class: "approval" },
      h("p", { class: "asks" }, `${this.name} wants to run `, h("b", {}, approval.tool)),
      approval.summary ? h("pre", {}, approval.summary) : null,
      h(
        "div",
        { class: "decisions" },
        button("Allow", decide("allow"), "primary"),
        button("Always", decide("always")),
        button("Deny", decide("deny"), "danger"),
      ),
    );
  }
}

const cards = new Map<string, Card>();

function renderAgents(current: Remote): void {
  const agents = current.state?.agents ?? [];
  const seen = new Set<string>();
  const elements: HTMLElement[] = [];
  for (const agent of agents) {
    seen.add(agent.name);
    let card = cards.get(agent.name);
    if (!card) {
      card = new Card(agent.name);
      cards.set(agent.name, card);
    }
    card.update(agent);
    elements.push(card.element);
  }
  for (const name of [...cards.keys()]) {
    if (!seen.has(name)) {
      cards.delete(name);
    }
  }
  if (current.state && agents.length === 0) {
    elements.push(h("p", { class: "empty" }, "No agents are running."));
  }
  // Keep the elements in place when nothing moved, so focus stays.
  const same =
    elements.length === agentList.children.length &&
    elements.every((element, index) => agentList.children[index] === element);
  if (!same) {
    agentList.replaceChildren(...elements);
  }
  agentList.classList.toggle("stale", current.phase !== "open");
}

// ----------------------------------------------------------------------
// The page

function renderAbout(current: Remote): void {
  const joining = current.peers === 0 ? current.joinError : null;
  const key = JSON.stringify([current.state?.emacs, current.did, joining, Boolean(current.pairing)]);
  rebuild(footer, key, () => [
    current.state?.emacs && h("span", {}, `Emacs ${current.state.emacs}`),
    h("span", { class: "did" }, `This phone: ${current.did}`),
    joining && h("span", {}, `Joining: ${joining}`),
    current.pairing && button("Pair with another LispM", startScan, "link"),
    current.pairing &&
      button(
        "Forget this LispM",
        () => {
          if (confirm("Forget this LispM on this phone? To pair again, you will scan a new code.")) {
            void current.forget();
          }
        },
        "link",
      ),
  ]);
}

function render(): void {
  const current = remote;
  if (!current) {
    return;
  }
  const status = current.status();
  showStatus(status.tone, status.text);
  noticeLine.className = `notice ${current.notice?.tone ?? ""}`;
  noticeLine.textContent = current.notice?.text ?? "";
  renderPair(current);
  renderAgents(current);
  renderAbout(current);
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
