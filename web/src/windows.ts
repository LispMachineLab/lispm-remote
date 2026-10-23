// lispm-remote: LispM's windows, on the page.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * LispM's windows, redrawn on this device: the display's client
 * (crates/client, in WebAssembly) mounted over the page, its messages
 * carried on the paired channel.  It says Hello when it shows, so LispM
 * starts sending, and Bye when it hides, so LispM stops.
 */

import initClient, { DisplayView } from "lispm-remote-client";
import { button, byId, h } from "./dom.js";
import type { Remote } from "./remote.js";

const section = byId<HTMLElement>("windows");

let starting: Promise<DisplayView> | null = null;
let shown = false;

/** Whether LispM's windows show. */
export function showingWindows(): boolean {
  return shown;
}

async function start(remote: Remote, close: () => void): Promise<DisplayView> {
  await initClient();
  const back = button("‹", close, "back");
  back.setAttribute("aria-label", "Back to the agents");
  const mount = h("div", { class: "windows-view" });
  section.append(h("header", { class: "windows-head" }, back, h("h2", {}, "LispM's windows")), mount);
  const view = new DisplayView(mount, (bytes: Uint8Array) => remote.sendDisplay(bytes));
  remote.onDisplay((payload) => view.receive(payload));
  remote.onChannel((open) => {
    if (!shown) {
      return;
    }
    if (open) {
      view.connected();
    } else {
      view.disconnected();
    }
  });
  return view;
}

/** Shows LispM's windows over `remote`'s channel; `close` hides them. */
export async function openWindows(remote: Remote, close: () => void): Promise<void> {
  starting ??= start(remote, close);
  const view = await starting;
  shown = true;
  section.hidden = false;
  if (remote.phase === "open") {
    view.connected();
  } else {
    view.disconnected();
  }
  view.fit();
}

/** Hides LispM's windows, and tells LispM to send nothing more. */
export function closeWindows(): void {
  if (!shown) {
    return;
  }
  shown = false;
  section.hidden = true;
  void starting?.then((view) => view.close());
}
