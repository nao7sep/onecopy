// @vitest-environment happy-dom
//
// Where a modal puts focus on open is a destructive-safety property, not a
// convenience one. ModalShell marks the header ✕ and the footer dismiss with
// data-modal-close, so in a ConfirmDialog — whose body is a plain <p> — the
// only remaining focusable was the danger-styled primary. "Delete
// permanently?" opened with "Delete permanently" focused, one press of the
// key already being held.

import { describe, expect, it } from "vitest";
import {
  getFocusableElements,
  resolveInitialFocus,
  resolveTrapTarget,
} from "../../src/utils/focusTrap";

function surfaceFrom(html: string): HTMLElement {
  const host = document.createElement("div");
  host.innerHTML = html;
  document.body.append(host);
  return host;
}

/** A ConfirmDialog as ModalShell renders it. */
const CONFIRM = `
  <button data-modal-close aria-label="Close">✕</button>
  <p>Permanently delete 3 items and every copy?</p>
  <button data-modal-close>Cancel</button>
  <button data-destructive>Delete permanently</button>
`;

/** A Settings-shaped surface: the first focusable is a "Remove" button. */
const SETTINGS = `
  <button data-modal-close aria-label="Close">✕</button>
  <ul><li><span>/Volumes/Photos</span><button>Remove</button></li></ul>
  <input id="timezone" value="Asia/Tokyo" />
  <button data-modal-close>Close</button>
  <button>Save</button>
`;

describe("initial focus", () => {
  it("opens a destructive confirmation on its dismiss control", () => {
    const surface = surfaceFrom(CONFIRM);
    const focused = resolveInitialFocus(surface);
    expect(focused.textContent).toBe("Cancel");
    expect(focused.hasAttribute("data-destructive")).toBe(false);
  });

  it("opens a form-bearing surface on its first field, not a Remove button", () => {
    const surface = surfaceFrom(SETTINGS);
    const focused = resolveInitialFocus(surface);
    expect(focused.tagName).toBe("INPUT");
    expect(focused.id).toBe("timezone");
  });

  it("still skips the header close on an ordinary surface", () => {
    const surface = surfaceFrom(`
      <button data-modal-close aria-label="Close">✕</button>
      <button>Check for updates</button>
      <button data-modal-close>Close</button>
    `);
    expect(resolveInitialFocus(surface).textContent).toBe("Check for updates");
  });

  it("falls back to the surface when nothing is focusable", () => {
    const surface = surfaceFrom(`<p>Nothing here</p>`);
    expect(resolveInitialFocus(surface)).toBe(surface);
  });
});

describe("tab trapping", () => {
  it("pulls focus to the first control when it is outside the surface", () => {
    const surface = surfaceFrom(CONFIRM);
    const target = resolveTrapTarget(surface, document.body, false);
    expect(target).toBe(getFocusableElements(surface)[0]);
  });

  it("wraps from the last control forward to the first", () => {
    const surface = surfaceFrom(CONFIRM);
    const focusables = getFocusableElements(surface);
    const last = focusables[focusables.length - 1]!;
    expect(resolveTrapTarget(surface, last, false)).toBe(focusables[0]);
  });

  it("wraps from the first control backward to the last", () => {
    const surface = surfaceFrom(CONFIRM);
    const focusables = getFocusableElements(surface);
    expect(resolveTrapTarget(surface, focusables[0]!, true)).toBe(
      focusables[focusables.length - 1],
    );
  });

  it("leaves an interior move to the browser", () => {
    const surface = surfaceFrom(CONFIRM);
    const focusables = getFocusableElements(surface);
    expect(resolveTrapTarget(surface, focusables[1]!, false)).toBeNull();
  });
});
