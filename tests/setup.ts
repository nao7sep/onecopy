// Vitest global setup.
//
// The default `node` environment has no `navigator`; utilities that read the
// platform at module load must never throw under `node`, so a bare stub is
// installed here. Tests that need a specific platform stub it themselves.

import { afterEach, beforeEach, vi } from "vitest";
import { CATALOGUES } from "../src/i18n/catalogues";

if (typeof globalThis.navigator === "undefined") {
  vi.stubGlobal("navigator", { platform: "", userAgent: "" });
}

// happy-dom does not expose the Web Animations document query that dnd-kit's
// geometry snapshot uses before measuring a registered draggable.
if (typeof document !== "undefined" && document.getAnimations === undefined) {
  document.getAnimations = () => [];
}
if (typeof Element !== "undefined" && Element.prototype.getAnimations === undefined) {
  Element.prototype.getAnimations = () => [];
}

// Tauri is faked for the whole suite from here, because this is the only place
// a `vi.mock` reaches every spec file — registering them per-spec would drift.
// The doubles and their controls live in tests/mocks/tauri.ts; each factory
// pulls from that one module so a spec and its mock share the same state.
// Specs opt in by importing the controls, not by re-registering the mock.

vi.mock("@tauri-apps/api/core", async () => {
  const m = await import("./mocks/tauri");
  return { invoke: m.invoke, convertFileSrc: m.convertFileSrc };
});

vi.mock("@tauri-apps/api/event", async () => {
  const m = await import("./mocks/tauri");
  return { listen: m.listen, emit: m.emit };
});

vi.mock("@tauri-apps/api/window", async () => {
  const m = await import("./mocks/tauri");
  return {
    getCurrentWindow: m.getCurrentWindow,
    availableMonitors: m.availableMonitors,
    currentMonitor: m.currentMonitor,
    LogicalSize: m.LogicalSize,
    PhysicalSize: m.PhysicalSize,
    PhysicalPosition: m.PhysicalPosition,
  };
});

vi.mock("@tauri-apps/api/webview", async () => {
  const m = await import("./mocks/tauri");
  return { getCurrentWebview: m.getCurrentWebview };
});

vi.mock("@tauri-apps/api/webviewWindow", async () => {
  const m = await import("./mocks/tauri");
  return { WebviewWindow: m.WebviewWindow };
});

vi.mock("@tauri-apps/plugin-dialog", async () => {
  const m = await import("./mocks/tauri");
  return { open: m.openDialog };
});

vi.mock("@tauri-apps/plugin-opener", async () => {
  const m = await import("./mocks/tauri");
  return { openPath: m.openPath, openUrl: m.openUrl, revealItemInDir: m.revealItemInDir };
});

// Every spec that renders the interface doubles as a check that no catalogue
// key reaches the screen untranslated: a key rendered as text or given to an
// attribute a person reads or hears. The type checker cannot catch this,
// because a key is a string and React renders any string. Specs mount their
// own roots, so an observer watches the document while each one runs.

const CATALOGUE_KEYS = new Set(Object.keys(CATALOGUES.en));
const READ_ATTRIBUTES = ["title", "aria-label", "aria-description", "placeholder", "alt", "label"];
const KEY_LIKE = /[A-Za-z]\w*(?:\.\w+)+/g;

let keyObserver: MutationObserver | null = null;
let renderedKeys = new Set<string>();

function checkText(text: string | null): void {
  for (const token of text?.match(KEY_LIKE) ?? []) {
    if (CATALOGUE_KEYS.has(token)) renderedKeys.add(token);
  }
}

// Text nodes one by one: a container's textContent runs neighbours together.
function scanNode(node: Node): void {
  if (node.nodeType === Node.TEXT_NODE) {
    checkText(node.nodeValue);
    return;
  }
  if (node instanceof Element) {
    for (const name of READ_ATTRIBUTES) checkText(node.getAttribute(name));
  }
  node.childNodes.forEach(scanNode);
}

function recordMutations(mutations: MutationRecord[]): void {
  for (const mutation of mutations) {
    if (mutation.type === "childList") mutation.addedNodes.forEach(scanNode);
    else if (mutation.type === "characterData") checkText(mutation.target.nodeValue);
    else if (mutation.target instanceof Element) {
      checkText(mutation.target.getAttribute(mutation.attributeName ?? ""));
    }
  }
}

beforeEach(() => {
  if (typeof document === "undefined") return;
  renderedKeys = new Set();
  scanNode(document.documentElement);
  keyObserver = new MutationObserver(recordMutations);
  keyObserver.observe(document.documentElement, {
    childList: true,
    subtree: true,
    characterData: true,
    attributes: true,
    attributeFilter: READ_ATTRIBUTES,
  });
});

afterEach(() => {
  if (keyObserver === null) return;
  recordMutations(keyObserver.takeRecords());
  keyObserver.disconnect();
  keyObserver = null;
  if (renderedKeys.size > 0) {
    throw new Error(`Untranslated catalogue keys on screen: ${[...renderedKeys].join(", ")}`);
  }
});
