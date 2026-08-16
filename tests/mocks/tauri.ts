// The Tauri IPC double for the frontend suite.
//
// The app reaches the Rust core through exactly seven module surfaces (core,
// event, window, webview, webviewWindow, plugin-dialog, plugin-opener), so
// faking those is enough to drive any store or component from a test without a
// running backend. The registrations themselves live in tests/setup.ts, which
// is the only place `vi.mock` applies to every spec file at once; this module
// owns the state and the helpers a spec talks to.
//
// `invoke` deliberately THROWS on an unregistered command rather than resolving
// undefined. A store calling a command the spec did not stub is either a
// contract drift or a test that does not know what it is exercising, and both
// should fail loudly instead of silently taking an undefined-shaped branch.

import { vi } from "vitest";

export type InvokeHandler = (
  args: Record<string, unknown>,
) => unknown | Promise<unknown>;

type EventCallback = (event: { payload: unknown }) => void;

const handlers = new Map<string, InvokeHandler>();
const listeners = new Map<string, Set<EventCallback>>();

/** Every invoke the code under test made, in order, for contract assertions. */
export const invokeCalls: Array<{
  command: string;
  args: Record<string, unknown>;
}> = [];

/** Every event the code under test emitted, in order. */
export const emitCalls: Array<{ event: string; payload: unknown }> = [];

/** Windows the code under test constructed, by label. */
export const createdWindows: Array<{
  label: string;
  options: Record<string, unknown>;
}> = [];

export const invoke = vi.fn(
  async (command: string, args: Record<string, unknown> = {}) => {
    invokeCalls.push({ command, args });
    const handler = handlers.get(command);
    if (!handler) {
      throw new Error(
        `invoke("${command}") has no mock — register one with mockCommand()`,
      );
    }
    return await handler(args);
  },
);

export const convertFileSrc = vi.fn(
  (path: string, protocol = "asset") => `${protocol}://localhost/${path}`,
);

export const listen = vi.fn(async (event: string, cb: EventCallback) => {
  let set = listeners.get(event);
  if (!set) {
    set = new Set();
    listeners.set(event, set);
  }
  set.add(cb);
  return () => {
    set.delete(cb);
  };
});

export const emit = vi.fn(async (event: string, payload?: unknown) => {
  emitCalls.push({ event, payload });
});

export const setFocus = vi.fn(async () => {});
export const setZoom = vi.fn(async () => {});
// Typed with its argument so a spec can assert the DERIVED minimum actually
// reached the window, not merely that the call happened.
export const setMinSize = vi.fn(async (_size: LogicalSize) => {});

export const getCurrentWindow = vi.fn(() => ({
  label: currentWindowLabel,
  setFocus,
  setMinSize,
}));

export const getCurrentWebview = vi.fn(() => ({ setZoom }));

export const availableMonitors = vi.fn(async () => monitors);

export class LogicalSize {
  constructor(
    public width: number,
    public height: number,
  ) {}
}

export class PhysicalSize {
  constructor(
    public width: number,
    public height: number,
  ) {}
}

export class PhysicalPosition {
  constructor(
    public x: number,
    public y: number,
  ) {}
}

/** Windows the app believes exist, by label — what `getByLabel` answers from.
 * The comparison spread REUSES its windows across sessions (hidden, not
 * closed), so a double that forgets them cannot exercise the reuse path. */
const liveWindows = new Map<string, WebviewWindow>();

/** Runs INSIDE the constructor, before it returns. A real webview starts
 * booting at this instant and may announce itself immediately, so a spec
 * asserting what the app had published *by then* has to observe here — after
 * the call it is too late to tell an early publish from a late one. */
let onWindowCreated: ((label: string) => void) | null = null;
export function setWindowCreatedHook(fn: ((label: string) => void) | null): void {
  onWindowCreated = fn;
}

export class WebviewWindow {
  label: string;
  constructor(label: string, options: Record<string, unknown> = {}) {
    this.label = label;
    createdWindows.push({ label, options });
    liveWindows.set(label, this);
    onWindowCreated?.(label);
  }
  static getByLabel = vi.fn(
    async (label: string): Promise<WebviewWindow | null> => liveWindows.get(label) ?? null,
  );
  once = vi.fn(async () => () => {});
  listen = vi.fn(async () => () => {});
  emit = vi.fn(async () => {});
  close = vi.fn(async () => {
    liveWindows.delete(this.label);
  });
  show = vi.fn(async () => {});
  hide = vi.fn(async () => {});
  setPosition = vi.fn(async (_p: PhysicalPosition) => {});
  setSize = vi.fn(async (_s: PhysicalSize) => {});
  setFocus = vi.fn(async () => {});
}

export const openDialog = vi.fn(async () => null as string | null);
export const openPath = vi.fn(async () => {});
export const openUrl = vi.fn(async () => {});

let currentWindowLabel = "main";
let monitors: Array<Record<string, unknown>> = [];

// --- test-side controls -----------------------------------------------------

/** Stub one Tauri command. Last registration for a name wins. */
export function mockCommand(command: string, handler: InvokeHandler): void {
  handlers.set(command, handler);
}

/** Stub several commands at once. */
export function mockCommands(map: Record<string, InvokeHandler>): void {
  for (const [command, handler] of Object.entries(map)) {
    handlers.set(command, handler);
  }
}

/** Deliver a backend event to everything currently listening for it. */
export function fireEvent(event: string, payload?: unknown): void {
  for (const cb of listeners.get(event) ?? []) {
    cb({ payload });
  }
}

/** How many listeners are registered — for leak and duplicate-listener specs. */
export function listenerCount(event: string): number {
  return listeners.get(event)?.size ?? 0;
}

export function setCurrentWindowLabel(label: string): void {
  currentWindowLabel = label;
}

export function setMonitors(next: Array<Record<string, unknown>>): void {
  monitors = next;
}

/** Full reset. Call from beforeEach so no spec inherits another's stubs.
 *
 * `keepListeners` matters for stores that register their event wiring ONCE at
 * module load: clearing the registry after that leaves them permanently deaf,
 * so a spec driving events must keep it. */
export function resetTauriMocks(
  options: { keepListeners?: boolean } = {},
): void {
  handlers.clear();
  if (!options.keepListeners) listeners.clear();
  invokeCalls.length = 0;
  emitCalls.length = 0;
  createdWindows.length = 0;
  liveWindows.clear();
  onWindowCreated = null;
  currentWindowLabel = "main";
  monitors = [];
  for (const spy of [
    invoke,
    convertFileSrc,
    listen,
    emit,
    setFocus,
    setZoom,
    setMinSize,
    getCurrentWindow,
    getCurrentWebview,
    availableMonitors,
    openDialog,
    openPath,
    openUrl,
  ]) {
    spy.mockClear();
  }
}
