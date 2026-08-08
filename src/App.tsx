import { useEffect, useRef, useState } from "react";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import "./App.css";
import { saveState } from "./repositories";
import { useAppStore } from "./state/app-store";
import {
  ZOOM_DEFAULT,
  isZoomIn,
  isZoomOut,
  isZoomReset,
  stepZoomIn,
  stepZoomOut,
} from "./utils/zoom";
import { computeMinWindowHeight, computeMinWindowWidth } from "./utils/windowSizing";
import { useSectionsStore } from "./state/sections-store";
import { useItemsStore } from "./state/items-store";
import Sidebar from "./components/Sidebar";
import Grid from "./components/Grid";
import MetadataPane from "./components/MetadataPane";
import DestinationsTab from "./components/DestinationsTab";
import Wizard from "./components/Wizard";
import PresenceGate from "./components/PresenceGate";
import ComparisonView from "./components/ComparisonView";
import IssuesView from "./components/IssuesView";
import BinariesModal from "./components/BinariesModal";
import ShortcutsModal from "./components/ShortcutsModal";
import SettingsModal from "./components/SettingsModal";
import { useSettingsStore } from "./state/settings-store";
import { isHelpShortcut } from "./utils/shortcuts";
import { useWizardStore } from "./state/wizard-store";
import { useComparisonStore } from "./state/comparison-store";
import { useIssuesStore } from "./state/issues-store";
import { useBinariesStore } from "./state/binaries-store";
import { itemKey } from "./state/items-store";

// The main-window shell: the sidebar listbox, the thumbnail grid for the
// selected section, the tabbed right pane, and the scan lifecycle in the
// status bar.

export default function App() {
  const appData = useAppStore((s) => s.appData);
  const loadError = useAppStore((s) => s.loadError);
  const counts = useSectionsStore((s) => s.counts);
  const scanning = useSectionsStore((s) => s.scanning);
  const progress = useSectionsStore((s) => s.progress);
  const loadCounts = useSectionsStore((s) => s.loadCounts);
  const startScan = useSectionsStore((s) => s.startScan);
  const selected = useItemsStore((s) => s.selected);
  const items = useItemsStore((s) => s.items);
  const itemsLoading = useItemsStore((s) => s.loading);
  const detail = useItemsStore((s) => s.detail);
  const selectedItemKey = useItemsStore((s) => s.selectedItem);
  const selectedHash =
    selectedItemKey !== null && !selectedItemKey.startsWith("path-")
      ? selectedItemKey
      : null;
  const wizardOpen = useWizardStore((s) => s.open);
  const missingDirs = useWizardStore((s) => s.missingDirs);
  const [rightTab, setRightTab] = useState<"details" | "destinations">("details");
  const issuesOpen = useIssuesStore((s) => s.open);
  const issuesTotal = useIssuesStore((s) => s.total);
  const setIssuesOpen = useIssuesStore((s) => s.setOpen);
  const ffmpegState = useBinariesStore((s) => s.state);
  const binariesInstalling = useBinariesStore((s) => s.installing);
  const binariesProgress = useBinariesStore((s) => s.progress);
  const setBinariesModalOpen = useBinariesStore((s) => s.setModalOpen);
  const [helpOpen, setHelpOpen] = useState(false);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement) {
        return;
      }
      if (isHelpShortcut(event)) {
        event.preventDefault();
        setHelpOpen((open) => !open);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  // A newly opened preview window asks for the current selection.
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    void import("@tauri-apps/api/event").then(async ({ listen }) => {
      const fn = await listen("preview://ready", () => {
        const { items, selectedItem } = useItemsStore.getState();
        const item = items.find((i) => itemKey(i) === selectedItem);
        if (item) {
          void import("./state/preview-store").then(({ updatePreviewIfOpen }) =>
            updatePreviewIfOpen({
              hash: item.hash,
              pathId: item.hash === null ? item.pathId : null,
            }),
          );
        }
      });
      if (disposed) fn();
      else unlisten = fn;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  // App chrome: the derived window minimum, applied once at startup; zoom is
  // persisted app-level STATE (not a preference) on the discrete ladder, with
  // cross-layout shortcut detection (JIS ";" included). Failures degrade
  // silently in the browser-dev case where no Tauri window exists.
  useEffect(() => {
    void getCurrentWindow()
      .setMinSize(new LogicalSize(computeMinWindowWidth(), computeMinWindowHeight()))
      .catch(() => {});
  }, []);

  const zoomRef = useRef(ZOOM_DEFAULT);

  useEffect(() => {
    if (appData === null) return;
    const stored = appData.state?.zoomLevel;
    const level = typeof stored === "number" ? stored : ZOOM_DEFAULT;
    zoomRef.current = level;
    if (level !== ZOOM_DEFAULT) {
      void getCurrentWebview().setZoom(level).catch(() => {});
    }
  }, [appData]);

  useEffect(() => {
    const onZoomKey = (event: KeyboardEvent) => {
      const zoomIn = isZoomIn(event);
      const zoomOut = isZoomOut(event);
      const zoomReset = isZoomReset(event);
      if (!zoomIn && !zoomOut && !zoomReset) return;
      event.preventDefault();
      const next = zoomReset
        ? ZOOM_DEFAULT
        : zoomIn
          ? stepZoomIn(zoomRef.current)
          : stepZoomOut(zoomRef.current);
      zoomRef.current = next;
      void (async () => {
        await getCurrentWebview().setZoom(next).catch(() => {});
        const state = { ...(appData?.state ?? {}), zoomLevel: next };
        await saveState(state).catch(() => {});
      })();
    };
    window.addEventListener("keydown", onZoomKey);
    return () => window.removeEventListener("keydown", onZoomKey);
  }, [appData]);

  // Grid keys: Delete/Backspace trash-deletes the selected logical item
  // (every copy; Shift = permanent); Enter opens the comparison view when the
  // selection has similar photos. Ignored while typing in a form control or
  // while the comparison view owns the keyboard.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (useComparisonStore.getState().open) return;
      const target = event.target as HTMLElement | null;
      if (
        target instanceof HTMLInputElement ||
        target instanceof HTMLTextAreaElement ||
        target?.isContentEditable
      ) {
        return;
      }
      if (event.key === "Delete" || event.key === "Backspace") {
        event.preventDefault();
        void useItemsStore.getState().deleteSelected(event.shiftKey);
      } else if (event.key === "Enter") {
        const { items, selectedItem } = useItemsStore.getState();
        const item = items.find((i) => itemKey(i) === selectedItem);
        if (!item) return;
        event.preventDefault();
        if (item.hash && item.similarGroupId !== null) {
          // Similar photos exist: Enter means "show them all at once".
          void useComparisonStore.getState().openGroup(item.hash);
        } else {
          // No similars: Enter opens the item in the preview window.
          void import("./state/preview-store").then(({ showPreview }) =>
            showPreview({
              hash: item.hash,
              pathId: item.hash === null ? item.pathId : null,
            }),
          );
        }
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  useEffect(() => {
    void useAppStore.getState().reload();
    void loadCounts();
    void useIssuesStore.getState().load();
    void useBinariesStore.getState().load();
  }, [loadCounts]);

  const allEmpty =
    counts !== null &&
    counts.images.length === 0 &&
    counts.videos.length === 0 &&
    counts.others.length === 0;

  return (
    <div className="flex h-screen flex-col bg-background text-ink">
      {wizardOpen && appData !== null ? (
        <Wizard baseConfig={appData.config} dataRoot={appData.dataRoot} />
      ) : missingDirs.length > 0 ? (
        <PresenceGate missing={missingDirs} />
      ) : null}
      <ComparisonView />
      <BinariesModal />
      <ShortcutsModal open={helpOpen} onClose={() => setHelpOpen(false)} />
      <SettingsModal baseConfig={appData?.config ?? null} />
      <div className="flex min-h-0 flex-1">
        <aside className="w-64 shrink-0 overflow-y-auto border-r border-border bg-surface p-3">
          <Sidebar counts={counts} />
          <button
            className={`mt-2 w-full rounded px-1 py-0.5 text-left text-sm ${
              issuesOpen
                ? "bg-danger-surface text-danger"
                : issuesTotal > 0
                  ? "text-danger hover:bg-danger-surface"
                  : "text-ink-muted hover:bg-surface-muted"
            }`}
            onClick={() => setIssuesOpen(!issuesOpen)}
          >
            Issues ({issuesTotal})
          </button>
        </aside>
        <main className="flex min-w-0 flex-1 flex-col overflow-hidden">
          {loadError !== null ? (
            <p className="m-auto text-danger">{loadError}</p>
          ) : issuesOpen ? (
            <IssuesView />
          ) : selected !== null ? (
            <Grid items={items} loading={itemsLoading} />
          ) : allEmpty ? (
            <p className="m-auto text-ink-muted">Nothing to handle</p>
          ) : (
            <p className="m-auto text-ink-muted">Select a month</p>
          )}
        </main>
        <aside className="flex w-72 shrink-0 flex-col overflow-hidden border-l border-border bg-surface">
          {/* One composite tablist: a single tab stop, arrows move and
              activate (panel swaps are cheap), per the composite-control
              conventions. */}
          <div
            role="tablist"
            aria-label="Right pane"
            className="flex shrink-0 border-b border-border"
          >
            {(["details", "destinations"] as const).map((tab, index, tabs) => (
              <button
                key={tab}
                role="tab"
                aria-selected={rightTab === tab}
                tabIndex={rightTab === tab ? 0 : -1}
                className={`flex-1 px-2 py-1 text-xs ${
                  rightTab === tab
                    ? "border-b-2 border-primary font-semibold text-primary"
                    : "text-ink-muted hover:text-ink"
                }`}
                onClick={() => setRightTab(tab)}
                onKeyDown={(event) => {
                  const delta =
                    event.key === "ArrowRight" || event.key === "End"
                      ? 1
                      : event.key === "ArrowLeft" || event.key === "Home"
                        ? -1
                        : 0;
                  if (delta === 0) return;
                  event.preventDefault();
                  const next = tabs[(index + delta + tabs.length) % tabs.length];
                  setRightTab(next);
                  (event.currentTarget.parentElement?.children[
                    (index + delta + tabs.length) % tabs.length
                  ] as HTMLElement | undefined)?.focus();
                }}
              >
                {tab === "details" ? "Details" : "Destinations"}
              </button>
            ))}
          </div>
          <div className="min-h-0 flex-1 overflow-y-auto">
            {rightTab === "details" ? (
              <MetadataPane detail={detail} hash={selectedHash} />
            ) : (
              <DestinationsTab />
            )}
          </div>
        </aside>
      </div>
      <footer className="flex shrink-0 items-center justify-between border-t border-border bg-surface px-3 py-1 text-xs text-ink-muted">
        <span>OneCopy {__APP_VERSION__}</span>
        <span>{scanning ? progress : ""}</span>
        <span className="flex items-center gap-3">
          <button
            className={
              ffmpegState?.status === "not-installed"
                ? "text-danger hover:underline"
                : "text-ink-muted hover:text-ink"
            }
            title="Managed tools"
            onClick={() => setBinariesModalOpen(true)}
          >
            {binariesInstalling
              ? binariesProgress
              : ffmpegState === null
                ? "ffmpeg: …"
                : ffmpegState.status === "not-installed"
                  ? "ffmpeg: not installed"
                  : `ffmpeg ${ffmpegState.facts.installedVersion ?? ""}`}
          </button>
          <button
            className="text-ink-muted hover:text-ink"
            onClick={() => useSettingsStore.getState().openWith(appData?.config ?? null)}
          >
            Settings
          </button>
          <button
            className="text-primary hover:text-primary-hover disabled:text-ink-muted"
            disabled={scanning}
            onClick={() => void startScan()}
          >
            {scanning ? "Scanning…" : "Scan"}
          </button>
          <span>{appData?.dataRoot ?? ""}</span>
        </span>
      </footer>
    </div>
  );
}
