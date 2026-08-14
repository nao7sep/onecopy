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

export const getCurrentWindow = vi.fn(() => ({
  label: currentWindowLabel,
  setFocus,
}));

export const getCurrentWebview = vi.fn(() => ({ setZoom }));

export const availableMonitors = vi.fn(async () => monitors);

export class LogicalSize {
  constructor(
    public width: number,
    public height: number,
  ) {}
}

export class WebviewWindow {
  label: string;
  constructor(label: string, options: Record<string, unknown> = {}) {
    this.label = label;
    createdWindows.push({ label, options });
  }
  once = vi.fn(async () => () => {});
  listen = vi.fn(async () => () => {});
  emit = vi.fn(async () => {});
  close = vi.fn(async () => {});
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

/** Full reset. Call from beforeEach so no spec inherits another's stubs. */
export function resetTauriMocks(): void {
  handlers.clear();
  listeners.clear();
  invokeCalls.length = 0;
  emitCalls.length = 0;
  createdWindows.length = 0;
  currentWindowLabel = "main";
  monitors = [];
  for (const spy of [
    invoke,
    convertFileSrc,
    listen,
    emit,
    setFocus,
    setZoom,
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
