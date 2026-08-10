// Focus-trap geometry for modal surfaces.
//
// These functions are pure with respect to a given DOM subtree: they read the
// surface and the current focus and decide where focus should go. The React
// shell (ModalShell) wires them to keydown/preventDefault and to focus() calls,
// so the decision logic stays testable without rendering a component.

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

// Where focus should land when the modal opens: the first useful control,
// skipping the header close button so forms and search boxes get focus first.
// Falls back to the surface itself when there is nothing else to focus.
export function resolveInitialFocus(surface: HTMLElement): HTMLElement {
  const focusables = getFocusableElements(surface);
  return focusables.find((el) => !el.hasAttribute("data-modal-close")) ?? surface;
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
