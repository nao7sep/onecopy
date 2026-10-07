// Vite-only visual fixture; not a product entry point or native application.
// Serve with the ordinary Vite development command and open this sibling HTML.
import { createRoot } from "react-dom/client";
import { mockIPC } from "@tauri-apps/api/mocks";
import { isTauri } from "@tauri-apps/api/core";


import { I18nProvider } from "../../src/i18n/I18nContext";
import { loadCatalogue } from "../../src/i18n/catalogues";


import { managedToolsFixture } from "./managed-tools";
import "../../src/App.css";

// A browser has no native bridge. Native visual acceptance uses a disposable
// ONECOPY_DATA_DIR and replaces every action exposed by this modal below; Tauri's
// actual injected IPC property is readonly and cannot use its browser mock.
if (!isTauri()) {
  mockIPC((command) => {
    if (command === "plugin:event|listen") return 1;
    if (command === "plugin:event|unlisten") return;
    throw new Error(`Native command blocked in visual fixture: ${command}`);
  });
}
const { default: BackgroundWorkModal } = await import("../../src/components/BackgroundWorkModal");
const { useDerivedWorkStore } = await import("../../src/state/derived-work-store");
const { useBinariesStore } = await import("../../src/state/binaries-store");
const { useAppStore } = await import("../../src/state/app-store");
const params = new URLSearchParams(location.search);
const platform = params.get("platform") === "windows" ? "windows" : "macos";
const identity = params.get("identity");
// The palette follows the browser's prefers-color-scheme, as the app follows
// its window theme: preview dark with the OS appearance or DevTools emulation.
document.body.className = "bg-background text-ink";
useBinariesStore.setState({
  entries: managedToolsFixture(platform, identity === "unreadable" || identity === "long" ? identity : "known"),
  loading: false,
  install: async () => {}, installAll: async () => {}, checkAll: async () => {},
  cancel: async () => {}, cancelCheck: async () => {},
});
useAppStore.setState({ saveConfig: async () => {} });
const language = params.get("language") === "ja" ? "ja" : "en";
await loadCatalogue(language);
useDerivedWorkStore.setState({
  setDetailsOpen: () => {}, setPaused: async () => {},
  snapshot: {
    workerRunning: true, pausedClasses: [], activeItem: null,
    classes: [
      { id: "previews", state: "running", queued: 12, failed: 0, done: 4, total: 12, reason: null },
      { id: "video-transcripts", state: "unavailable", queued: 3, failed: 0, done: null, total: null, reason: "waiting-for-transcription-model" },
    ],
  },
});
createRoot(document.getElementById("root")!).render(
  <>
    <p className="fixed inset-x-0 top-2 z-50 text-center text-xs text-ink-muted">
      Synthetic {platform === "windows" ? "Windows" : "macOS"} data · installed tools unchanged
    </p>
    <I18nProvider language={language} locale={language}><BackgroundWorkModal open onClose={() => {}} /></I18nProvider>
  </>,
);
