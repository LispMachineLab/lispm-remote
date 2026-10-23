// lispm-remote: building the page's elements.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

/** What an element may hold: nodes and text; falsy children are left out. */
export type Child = Node | string | null | undefined | false;

/** A `tag` element with the attributes `props` and the `children`.  Text
 * becomes text nodes, never markup. */
export function h<K extends keyof HTMLElementTagNameMap>(
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

/** The page's element `id`. */
export function byId<T extends HTMLElement>(id: string): T {
  const element = document.getElementById(id);
  if (!element) {
    throw new Error(`The page has no #${id}.`);
  }
  return element as T;
}

/** Fills `element` with what `build` makes, unless it shows `key` already:
 * a button replaced at every render could lose the tap meant for it. */
export function rebuild(element: HTMLElement, key: string, build: () => Child[]): void {
  if (element.dataset.key !== key) {
    element.dataset.key = key;
    element.replaceChildren(...build().filter((child): child is Node | string => Boolean(child)));
  }
}

/** A button saying `text` that does `action`, of the style `kind`. */
export function button(text: string, action: () => void, kind = ""): HTMLButtonElement {
  const element = h("button", { type: "button", class: kind || undefined }, text);
  element.addEventListener("click", action);
  return element;
}
