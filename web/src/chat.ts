// lispm-remote: talking to LispM's agents.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * The agents, and a chat with one of them: what the owner and the agent
 * said, the agent's calls and thoughts, the calls it waits on with their
 * Allow / Always / Deny, a box to write in and a button to stop its turn.
 * With one agent, its chat opens at once; the agent last opened is
 * remembered on this device.
 */

import { button, byId, type Child, h, rebuild } from "./dom.js";
import type { Agent, Approval, ChatItem, Remote } from "./remote.js";

const REMEMBERED = "lispm-remote.agent";

const list = byId<HTMLElement>("agents");
const view = byId<HTMLElement>("chat");

let remote: Remote | null = null;
let render: () => void = () => {};
let selected: string | null = remembered();
let opened = false;
let sent = false;

function remembered(): string | null {
  try {
    return localStorage.getItem(REMEMBERED);
  } catch {
    return null;
  }
}

function select(name: string | null): void {
  selected = name;
  try {
    if (name) {
      localStorage.setItem(REMEMBERED, name);
    } else {
      localStorage.removeItem(REMEMBERED);
    }
  } catch {
    // Remembering is a convenience.
  }
  render();
  if (name) {
    scrollDown();
  }
}

function scrollDown(): void {
  requestAnimationFrame(() => window.scrollTo(0, document.documentElement.scrollHeight));
}

function nearBottom(): boolean {
  return window.innerHeight + window.scrollY >= document.documentElement.scrollHeight - 120;
}

// ----------------------------------------------------------------------
// What was said

/** `text` with its `code` and **bold** spans. */
function inline(text: string): Child[] {
  const parts: Child[] = [];
  let last = 0;
  for (const match of text.matchAll(/`[^`\n]+`|\*\*[^*\n]+\*\*/g)) {
    const at = match.index ?? 0;
    const token = match[0];
    parts.push(text.slice(last, at));
    parts.push(token.startsWith("`") ? h("code", {}, token.slice(1, -1)) : h("strong", {}, token.slice(2, -2)));
    last = at + token.length;
  }
  parts.push(text.slice(last));
  return parts;
}

/** A reply's Markdown as paragraphs and code blocks, never as markup. */
function rich(text: string): Child[] {
  const out: Child[] = [];
  let paragraph: string[] = [];
  let code: string[] | null = null;
  const flush = () => {
    if (paragraph.length > 0) {
      out.push(h("p", {}, ...inline(paragraph.join("\n"))));
      paragraph = [];
    }
  };
  for (const line of text.split("\n")) {
    if (/^\s*```/.test(line)) {
      if (code) {
        out.push(h("pre", {}, code.join("\n")));
        code = null;
      } else {
        flush();
        code = [];
      }
    } else if (code) {
      code.push(line);
    } else if (line.trim() === "") {
      flush();
    } else {
      paragraph.push(line.replace(/^#{1,6}\s+/, ""));
    }
  }
  if (code) {
    out.push(h("pre", {}, code.join("\n")));
  }
  flush();
  return out;
}

function said(item: ChatItem): Child {
  switch (item.kind) {
    case "user":
      return h(
        "div",
        { class: "message mine" },
        item.from ? h("span", { class: "from" }, item.from) : null,
        h("div", { class: "bubble" }, item.text ?? ""),
      );
    case "text":
      return h("div", { class: "message theirs" }, h("div", { class: "bubble" }, ...rich(item.text ?? "")));
    case "thought":
      return h("details", { class: "thought" }, h("summary", {}, "Thought"), h("p", {}, item.text ?? ""));
    case "tool": {
      const state = (item.state ?? "").toLowerCase();
      return h(
        "details",
        { class: `call ${item.failed ? "failed" : ""}` },
        h(
          "summary",
          {},
          h("span", { class: "tool" }, item.name ?? "call"),
          item.summary ? h("span", { class: "input" }, item.summary) : null,
          state ? h("span", { class: `state ${state}` }, state) : null,
        ),
        item.result ? h("pre", {}, item.result) : null,
      );
    }
    case "event":
      return h("p", { class: "aside" }, `${item.name ?? "Event"}: ${item.text ?? ""}`);
    default:
      return h("p", { class: "aside" }, item.text ?? "");
  }
}

function approval(agent: string, asked: Approval): HTMLElement {
  const decide = (decision: "allow" | "always" | "deny") => () =>
    remote?.decide(agent, asked.call, decision);
  return h(
    "div",
    { class: "approval" },
    h("p", { class: "asks" }, `${agent} wants to run `, h("b", {}, asked.tool)),
    asked.summary ? h("pre", {}, asked.summary) : null,
    h(
      "div",
      { class: "decisions" },
      button("Allow", decide("allow"), "primary"),
      button("Always", decide("always")),
      button("Deny", decide("deny"), "danger"),
    ),
  );
}

// ----------------------------------------------------------------------
// The chat

const back = button("‹", () => select(null), "back");
back.setAttribute("aria-label", "All agents");
const title = h("h2");
const activity = h("p", { class: "activity" });
const badge = h("span", { class: "badge" });
const earlier = h("p", { class: "aside" });
const messages = h("div", { class: "messages", role: "log" });
const waiting = h("div", { class: "approvals" });
const input = h("textarea", { rows: "1", enterkeyhint: "send", autocomplete: "off" });
const stop = button("Stop", () => selected && remote?.interrupt(selected), "danger");
const composer = h("form", { class: "composer" }, input, h("button", { type: "submit", class: "primary" }, "Send"), stop);
view.append(
  h("header", { class: "chat-head" }, back, h("div", { class: "who" }, title, activity), badge),
  earlier,
  messages,
  waiting,
  composer,
);

function fit(): void {
  input.style.height = "auto";
  input.style.height = `${Math.min(input.scrollHeight, 160)}px`;
}

function submit(): void {
  const text = input.value.trim();
  if (text && selected && remote) {
    remote.say(selected, text);
    input.value = "";
    fit();
    sent = true;
  }
}

composer.addEventListener("submit", (event) => {
  event.preventDefault();
  submit();
});
input.addEventListener("keydown", (event) => {
  // Enter sends; Shift-Enter starts a new line; an input method that is
  // composing a word keeps its Enter.
  if (event.key === "Enter" && !event.shiftKey && !event.isComposing) {
    event.preventDefault();
    submit();
  }
});
input.addEventListener("input", fit);

function showChat(current: Remote, name: string, agent: Agent | undefined): void {
  const state = agent?.state || "idle";
  title.textContent = name;
  activity.textContent = agent?.status || "";
  activity.hidden = !agent?.status;
  badge.textContent = state;
  badge.className = `badge ${state}`;
  stop.hidden = state === "idle";
  input.placeholder = `Message ${name}`;
  current.follow(name);

  const chat = current.chat;
  earlier.textContent = chat?.more
    ? `${chat.more} earlier ${chat.more === 1 ? "message is" : "messages are"} in Emacs.`
    : "";
  earlier.hidden = !chat?.more;
  const outgoing = current.outbox.filter((entry) => entry.agent === name);
  const down = nearBottom() || sent;
  rebuild(messages, JSON.stringify([chat?.items ?? null, outgoing.map((entry) => entry.text)]), () => {
    if (!chat) {
      return [h("p", { class: "aside" }, "Loading the conversation…")];
    }
    const items: Child[] = chat.items.map(said);
    for (const entry of outgoing) {
      items.push(h("div", { class: "message mine sending" }, h("div", { class: "bubble" }, entry.text)));
    }
    return items.length > 0 ? items : [h("p", { class: "aside" }, `Say something to ${name}.`)];
  });
  const asked = agent?.approvals ?? [];
  rebuild(waiting, JSON.stringify(asked), () => asked.map((entry) => approval(name, entry)));
  if (down) {
    scrollDown();
  }
  sent = false;
}

// ----------------------------------------------------------------------
// The agents

function row(agent: Agent): HTMLElement {
  const state = agent.state || "idle";
  const asked = agent.approvals?.length ?? 0;
  const element = h(
    "button",
    { type: "button", class: "agent" },
    h("span", { class: "row" }, h("strong", {}, agent.name), h("span", { class: `badge ${state}` }, state)),
    agent.status ? h("span", { class: "activity" }, agent.status) : null,
    agent.project ? h("span", { class: "project" }, agent.project) : null,
    asked > 0 ? h("span", { class: "asked" }, `${asked} to approve`) : null,
  );
  element.addEventListener("click", () => select(agent.name));
  return element;
}

function showList(current: Remote): void {
  const agents = current.state?.agents ?? [];
  current.follow(null);
  rebuild(list, JSON.stringify(agents), () =>
    agents.length > 0
      ? agents.map(row)
      : [h("p", { class: "aside" }, current.state ? "No agents are running." : "")],
  );
}

/** Whether the chat with an agent shows. */
export function chatting(): boolean {
  return !view.hidden;
}

/** Shows the agents, or the chat with the one selected, as `current` has
 * them; `again` draws the page again. */
export function showAgents(current: Remote, again: () => void): void {
  remote = current;
  render = again;
  if (!current.pairing) {
    list.hidden = true;
    view.hidden = true;
    current.follow(null);
    return;
  }
  const agents = current.state?.agents ?? [];
  if (!opened && current.state) {
    opened = true;
    if (!selected && agents.length === 1) {
      selected = agents[0]?.name ?? null;
    }
  }
  const agent = agents.find((entry) => entry.name === selected);
  if (selected && current.state && !agent) {
    selected = null;
  }
  list.hidden = selected !== null;
  view.hidden = selected === null;
  list.classList.toggle("stale", current.phase !== "open");
  view.classList.toggle("stale", current.phase !== "open");
  if (selected) {
    showChat(current, selected, agent);
  } else {
    showList(current);
  }
}
