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

const listeners = new Set<() => void>();

export function subscribeModalStack(listener: () => void): () => void {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

const stack: Array<{ token: object; surface?: HTMLElement; opener: HTMLElement | null; recovery: boolean }> = [];

// Inert only siblings of the active surface and its ancestors. Making an
// ancestor inert would disable the modal itself, including nested dialogs.
const inerted = new Map<HTMLElement, string | null>();
let observer: MutationObserver | null = null;

function updateBackground(): void {
  for (const [element, original] of inerted) {
    if (original === null) element.removeAttribute("inert");
    else element.setAttribute("inert", original);
  }
  inerted.clear();
  observer?.disconnect();
  observer = null;
  let surface: HTMLElement | undefined;
  for (let index = stack.length - 1; index >= 0; index--) {
    if (stack[index].surface !== undefined) { surface = stack[index].surface; break; }
  }
  if (!surface?.isConnected) return;
  let branch: HTMLElement = surface;
  while (branch.parentElement) {
    for (const sibling of branch.parentElement.children) {
      if (sibling === branch || !(sibling instanceof HTMLElement)) continue;
      inerted.set(sibling, sibling.getAttribute("inert"));
      sibling.setAttribute("inert", "");
    }
    if (branch.parentElement === document.body) break;
    branch = branch.parentElement;
  }
  // Portals or other siblings can mount while a blocking surface is open.
  // Observe structure only: our own inert attributes cannot retrigger this.
  observer = new MutationObserver(updateBackground);
  observer.observe(document.body, { childList: true, subtree: true });
}

export function pushModal(token: object, surface?: HTMLElement, opener: HTMLElement | null = null, recovery = false): void {
  // Nested layers may mount in one React commit, whose child layout effects
  // run before the parent's. Containment establishes that child's precedence.
  const descendant = surface === undefined ? -1 : stack.findIndex((entry) =>
    entry.surface !== undefined && surface.contains(entry.surface));
  if (descendant < 0) {
    // An escaped-failure root owns the window until reload. Delayed ordinary
    // dialogs may mount underneath it, but cannot make recovery inert.
    const terminal = recovery ? -1 : stack.findIndex((entry) => entry.recovery);
    if (terminal < 0) stack.push({ token, surface, opener, recovery });
    else stack.splice(terminal, 0, { token, surface, opener, recovery });
  }
  else {
    const child = stack[descendant];
    // A simultaneously mounted child acquired focus first. Its original
    // opener belongs to the parent; closing the child returns to the parent.
    stack.splice(descendant, 0, { token, surface, opener: child.opener, recovery });
    child.opener = surface ?? null;
  }
  updateBackground();
  listeners.forEach((listener) => listener());
}

/** Remove a layer and return its valid focus destination, only if it was topmost. */
export function popModal(token: object, destination?: HTMLElement | null): HTMLElement | null {
  const index = stack.findIndex((entry) => entry.token === token);
  if (index < 0) return null;
  const wasTopmost = index === stack.length - 1;
  const [closed] = stack.splice(index, 1);
  for (const entry of stack) {
    if (entry.opener && closed.surface?.contains(entry.opener)) entry.opener = closed.opener;
  }
  updateBackground();
  listeners.forEach((listener) => listener());
  if (!wasTopmost) return null;
  const remaining = stack[stack.length - 1]?.surface;
  if (destination?.isConnected && (!remaining || remaining.contains(destination))) return destination;
  if (closed.opener?.isConnected && (!remaining || remaining.contains(closed.opener))) return closed.opener;
  return remaining ?? null;
}

export function isTopmostModal(token: object): boolean {
  return stack.length > 0 && stack[stack.length - 1].token === token;
}

// A full-window workspace may retain its own app-chrome commands, while
// still blocking commands for covered content. Nested dialogs always block.
export function hasOpenModal(except?: object): boolean {
  return stack.some((entry) => entry.token !== except);
}

/** Empties the stack. Test-only: the stack is module-global, so a spec that
 * pushed without popping would otherwise leak into every later spec and make
 * `hasOpenModal()` lie about the next one. */
export function resetModalStack(): void {
  stack.length = 0;
  updateBackground();
  listeners.forEach((listener) => listener());
}
