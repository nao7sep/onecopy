// Explicit modal layering stack.
//
// Each open modal registers an opaque token on mount and removes it on unmount.
// "Topmost" is the newest layer, with nested children above their parent. This lets
// Escape, the Tab focus trap, and backdrop clicks act on the top layer only,
// without coupling to DOM order or CSS class names. Tokens are object
// identities, so callers never need to mint unique ids.
//
// `hasOpenModal` is OneCopy's addition: the main window's command layer
// (delete/Enter/zoom) must go quiet while ANY modal is open — Backspace over
// an open Settings dialog must never trash files behind the backdrop.

const stack: Array<{ token: object; surface?: HTMLElement; opener: HTMLElement | null }> = [];

export function pushModal(token: object, surface?: HTMLElement, opener: HTMLElement | null = null): void {
  // Nested layers may mount in one React commit, whose child layout effects
  // run before the parent's. Containment establishes that child's precedence.
  const descendant = surface === undefined ? -1 : stack.findIndex((entry) =>
    entry.surface !== undefined && surface.contains(entry.surface));
  if (descendant < 0) stack.push({ token, surface, opener });
  else {
    const child = stack[descendant];
    // A simultaneously mounted child acquired focus first. Its original
    // opener belongs to the parent; closing the child returns to the parent.
    stack.splice(descendant, 0, { token, surface, opener: child.opener });
    child.opener = surface ?? null;
  }
}

/** Remove a layer and return its valid focus destination, only if it was topmost. */
export function popModal(token: object): HTMLElement | null {
  const index = stack.findIndex((entry) => entry.token === token);
  if (index < 0) return null;
  const wasTopmost = index === stack.length - 1;
  const [closed] = stack.splice(index, 1);
  for (const entry of stack) {
    if (entry.opener && closed.surface?.contains(entry.opener)) entry.opener = closed.opener;
  }
  if (!wasTopmost) return null;
  const remaining = stack[stack.length - 1]?.surface;
  if (closed.opener?.isConnected && (!remaining || remaining.contains(closed.opener))) return closed.opener;
  return remaining ?? null;
}

export function isTopmostModal(token: object): boolean {
  return stack.length > 0 && stack[stack.length - 1].token === token;
}

export function hasOpenModal(): boolean {
  return stack.length > 0;
}

/** Empties the stack. Test-only: the stack is module-global, so a spec that
 * pushed without popping would otherwise leak into every later spec and make
 * `hasOpenModal()` lie about the next one. */
export function resetModalStack(): void {
  stack.length = 0;
}
