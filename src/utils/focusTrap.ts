// Focus-trap geometry for modal surfaces.
//
// These functions are pure with respect to a given DOM subtree: they read the
// surface and the current focus and decide where focus should go. The React
// shell (ModalShell) wires them to keydown/preventDefault and to focus() calls,
// so the decision logic stays testable without rendering a component.

import { consumesPrintableKeys } from "./shortcuts";

const FOCUSABLE_SELECTOR = [
  "a[href]",
  "button:not([disabled])",
  "input:not([disabled])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  '[tabindex]:not([tabindex="-1"])',
].join(",");

export function getFocusableElements(container: HTMLElement): HTMLElement[] {
  return Array.from(container.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR));
}

/** A field worth opening ON: one the user can immediately type into.
 *
 * SELECT and the toggle-ish INPUT types are deliberately NOT here. They are
 * still perfectly good focus targets once tabbed to — but landing on one buys
 * no typing head start, and it costs real orientation: browsers scroll a
 * focused control into view, so a checkbox two thirds down a long tab opens
 * that tab already scrolled PAST its own heading. That is what Managed tools
 * did — its first field is a checkbox far down the panel, so opening Settings
 * there hid the top of the surface the user had just asked to see. */
function isTypeableField(el: HTMLElement): boolean {
  return consumesPrintableKeys(el) && !el.hasAttribute("data-modal-close");
}

// Where focus should land when the modal opens.
//
// Order matters for safety, not just convenience:
//  0. The surface's explicit focus contract, when provided.
//  1. The first TYPEABLE field — what the user opened the surface to edit.
//     Without this, Settings opens on the first source directory's "Remove"
//     button, one reflexive Enter away from dropping a scanned directory.
//  2. The dismiss control, when the surface carries a destructive primary
//     ([data-destructive]). A confirmation focused on "Delete permanently" is
//     not a speed bump — it is one more press of the key already being held,
//     which is exactly the rhythm a cull run is in.
//  3. Otherwise the first useful control, skipping the header close button.
// Falls back to the surface itself when there is nothing else to focus.
export function resolveInitialFocus(surface: HTMLElement): HTMLElement {
  if (surface.hasAttribute("data-modal-initial-focus")) return surface;
  const explicit = surface.querySelector<HTMLElement>("[data-modal-initial-focus]:not([disabled])");
  if (explicit !== null) return explicit;
  const focusables = getFocusableElements(surface);
  const field = focusables.find(isTypeableField);
  if (field) return field;
  if (surface.querySelector("[data-destructive]")) {
    // The LAST dismiss control, not the first: the header ✕ comes first in
    // DOM order but is supplementary, while the labelled footer dismiss sits
    // beside the primary action and is the one a user reads as the way out.
    const dismissals = focusables.filter((el) =>
      el.hasAttribute("data-modal-close"),
    );
    const dismiss = dismissals[dismissals.length - 1];
    if (dismiss) return dismiss;
  }
  return focusables.find((el) => !el.hasAttribute("data-modal-close")) ?? surface;
}

/** Confirmation footer arrows are independent-button navigation, not a
 * dialog-wide composite or an activation shortcut. */
export function resolveFooterArrowTarget(
  surface: HTMLElement, active: Element | null, direction: "left" | "right",
): HTMLElement | null {
  const footer = surface.querySelector<HTMLElement>("[data-modal-actions]");
  if (footer === null) return null;
  const actions = getFocusableElements(footer);
  if (actions.length === 0) return null;
  const index = actions.findIndex((action) => action === active);
  if (index < 0) return null;
  return actions[Math.max(0, Math.min(actions.length - 1, index + (direction === "left" ? -1 : 1)))];
}

// Given the current focus and Tab direction, return the element focus must move
// to in order to stay trapped, or null when the browser's default Tab already
// keeps focus inside the modal. Focus on the surface itself, or anywhere
// outside it, is treated as "escaped" and pulled to the appropriate edge.
export function resolveTrapTarget(
  surface: HTMLElement,
  active: Element | null,
  shiftKey: boolean,
): HTMLElement | null {
  const focusables = getFocusableElements(surface);
  if (focusables.length === 0) {
    return surface;
  }

  const first = focusables[0];
  const last = focusables[focusables.length - 1];
  const inside = active !== null && active !== surface && surface.contains(active);

  if (!inside) {
    return shiftKey ? last : first;
  }
  if (shiftKey && active === first) {
    return last;
  }
  if (!shiftKey && active === last) {
    return first;
  }
  return null;
}
