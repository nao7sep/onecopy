// Explicit modal layering stack.
//
// Each open modal registers an opaque token on mount and removes it on unmount.
// "Topmost" is simply the most recently pushed token still present. This lets
// Escape, the Tab focus trap, and backdrop clicks act on the top layer only,
// without coupling to DOM order or CSS class names. Tokens are object
// identities, so callers never need to mint unique ids.
//
// `hasOpenModal` is OneCopy's addition: the main window's command layer
// (delete/Enter/zoom) must go quiet while ANY modal is open — Backspace over
// an open Settings dialog must never trash files behind the backdrop.

const stack: object[] = [];

export function pushModal(token: object): void {
  stack.push(token);
}

export function popModal(token: object): void {
  const index = stack.lastIndexOf(token);
  if (index !== -1) {
    stack.splice(index, 1);
  }
}

export function isTopmostModal(token: object): boolean {
  return stack.length > 0 && stack[stack.length - 1] === token;
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
