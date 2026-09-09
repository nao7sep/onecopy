// Vite-only visual fixture; not a product entry point or native application.
// Serve with the ordinary Vite development command and open this sibling HTML.
import { createRoot } from "react-dom/client";
import { mockIPC } from "@tauri-apps/api/mocks";
import { isTauri } from "@tauri-apps/api/core";
import BinariesModal from "../../src/components/BinariesModal";
import { useBinariesStore } from "../../src/state/binaries-store";
import { useAppStore } from "../../src/state/app-store";
import { managedToolsFixture } from "./managed-tools";
import "../../src/App.css";

// A browser has no native bridge. Native visual acceptance uses a disposable
// ONECOPY_HOME and replaces every action exposed by this modal below; Tauri's
// actual injected IPC property is readonly and cannot use its browser mock.
if (!isTauri()) {
  mockIPC((command) => { throw new Error(`Native command blocked in visual fixture: ${command}`); });
}
const params = new URLSearchParams(location.search);
const platform = params.get("platform") === "windows" ? "windows" : "macos";
const identity = params.get("identity");
document.documentElement.classList.toggle("dark", params.get("theme") === "dark");
document.body.className = "bg-background text-ink";
useBinariesStore.setState({
  entries: managedToolsFixture(platform, identity === "unreadable" || identity === "long" ? identity : "known"),
  loading: false,
  install: async () => {}, installAll: async () => {}, checkAll: async () => {},
  cancel: async () => {}, cancelCheck: async () => {},
});
useAppStore.setState({ patchConfig: async () => {} });
createRoot(document.getElementById("root")!).render(
  <>
    <p className="fixed inset-x-0 top-2 z-50 text-center text-xs text-ink-muted">
      Synthetic {platform === "windows" ? "Windows" : "macOS"} data · installed tools unchanged
    </p>
    <BinariesModal open onClose={() => {}} />
  </>,
);
