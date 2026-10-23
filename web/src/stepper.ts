// lispm-remote: drawing the steps.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * The card that shows pairing or reconnecting as steps (steps.ts): a bar of
 * the whole, then each step with its mark, title, detail and bar.  The scan
 * step holds the camera's viewfinder and the confirm step the fingerprint.
 * The card is built once and updated in place, so the camera's video and a
 * bar's running animation survive every update; each step's element
 * carries `data-step` and `data-state`, and the card `data-flow`.
 */

import { button, h } from "./dom.js";
import { progress, type Step, type StepName } from "./steps.js";

const ORDER: readonly StepName[] = ["join", "scan", "reach", "confirm", "connected"];

const SVG = "http://www.w3.org/2000/svg";

/** The tick drawn in a mark once its step is done. */
function tick(): SVGSVGElement {
  const svg = document.createElementNS(SVG, "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("class", "tick");
  const path = document.createElementNS(SVG, "path");
  path.setAttribute("d", "M5 12.5l4.5 4.5L19 7.5");
  svg.append(path);
  return svg;
}

class Row {
  readonly element: HTMLLIElement;
  private readonly number = h("span", { class: "number" });
  private readonly title = h("p", { class: "title" });
  private readonly detail = h("p", { class: "detail" });
  private readonly bar = h("div", { class: "bar", role: "progressbar" });
  private readonly fill = h("span");

  constructor(name: StepName, readonly extra: HTMLElement) {
    this.bar.append(this.fill);
    this.element = h(
      "li",
      { class: "step", "data-step": name, "data-state": "pending" },
      h("span", { class: "mark", "aria-hidden": "true" }, this.number, tick()),
      h("div", { class: "body" }, this.title, this.detail, this.bar, extra),
    );
  }

  update(step: Step, number: number): void {
    this.element.dataset.state = step.state;
    this.number.textContent = String(number);
    this.title.textContent = step.title;
    this.detail.textContent = step.detail;
    if (step.bar === undefined) {
      this.bar.hidden = true;
      return;
    }
    this.bar.hidden = false;
    if (step.bar === "busy") {
      this.bar.className = "bar busy";
      this.bar.removeAttribute("aria-valuenow");
      this.fill.style.width = "";
    } else {
      const left = step.bar;
      this.bar.className = `bar countdown ${left < 0.2 ? "low" : left < 0.5 ? "half" : ""}`;
      this.bar.setAttribute("aria-valuenow", String(Math.round(left * 100)));
      this.fill.style.width = `${(left * 100).toFixed(1)}%`;
    }
  }
}

/** What the card needs beyond the steps. */
export interface Extras {
  /** The camera is looking for the code. */
  readonly scanning: boolean;
  /** The fingerprint to compare, while it is to be compared. */
  readonly fingerprint: string | null;
}

/** The card of steps. */
export class Stepper {
  readonly element: HTMLElement;
  /** Where the camera shows what it sees. */
  readonly video: HTMLVideoElement;
  private readonly heading = h("h2");
  private readonly count = h("span", { class: "count" });
  private readonly overall = h("span");
  private readonly rows = new Map<StepName, Row>();
  private readonly viewfinder: HTMLElement;
  private readonly scanButton: HTMLButtonElement;
  private readonly cancelButton: HTMLButtonElement;
  private readonly fingerprint = h("p", { class: "fingerprint" });

  constructor(actions: { readonly scan: () => void; readonly cancel: () => void }) {
    this.video = h("video");
    // Inline and silent, or a phone will not play it by itself.
    this.video.muted = true;
    this.video.playsInline = true;
    this.viewfinder = h(
      "div",
      { class: "viewfinder" },
      this.video,
      h("span", { class: "corner tl" }),
      h("span", { class: "corner tr" }),
      h("span", { class: "corner bl" }),
      h("span", { class: "corner br" }),
      h("span", { class: "laser" }),
    );
    this.scanButton = button("Scan the code", actions.scan, "primary");
    this.cancelButton = button("Cancel", actions.cancel, "quiet");
    const extras: Record<StepName, HTMLElement> = {
      join: h("div", { class: "extra" }),
      scan: h("div", { class: "extra" }, this.viewfinder, this.scanButton, this.cancelButton),
      reach: h("div", { class: "extra" }),
      confirm: h("div", { class: "extra" }, this.fingerprint),
      connected: h("div", { class: "extra" }),
    };
    const list = h("ol", { class: "steps" });
    for (const name of ORDER) {
      const row = new Row(name, extras[name]);
      this.rows.set(name, row);
      list.append(row.element);
    }
    this.element = h(
      "div",
      { class: "stepper" },
      h("div", { class: "stepper-head" }, this.heading, this.count),
      h("div", { class: "overall", role: "progressbar" }, this.overall),
      list,
    );
  }

  /** Shows `steps` of `flow`, with `extras`. */
  update(flow: "pair" | "reconnect", steps: readonly Step[], extras: Extras): void {
    const card = this.element.parentElement;
    if (card) {
      card.dataset.flow = flow;
    }
    this.heading.textContent = flow === "pair" ? "Pair with LispM" : "Reconnecting to LispM";
    const shown = new Set(steps.map((step) => step.name));
    steps.forEach((step, index) => this.rows.get(step.name)?.update(step, index + 1));
    for (const [name, row] of this.rows) {
      row.element.hidden = !shown.has(name);
    }
    const over = steps.filter((step) => step.state === "done" || step.state === "skipped").length;
    this.count.textContent = `${over} of ${steps.length}`;
    this.overall.style.width = `${(progress(steps) * 100).toFixed(1)}%`;

    const scan = steps.find((step) => step.name === "scan");
    this.viewfinder.hidden = !extras.scanning;
    this.cancelButton.hidden = !extras.scanning;
    this.scanButton.hidden = extras.scanning || scan?.state !== "active";
    const confirm = steps.find((step) => step.name === "confirm");
    this.fingerprint.hidden = !(confirm?.state === "active" && extras.fingerprint);
    this.fingerprint.textContent = extras.fingerprint ?? "";
  }
}
