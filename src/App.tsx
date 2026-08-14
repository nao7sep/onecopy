import { useEffect, useRef, useState } from "react";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import "./App.css";
import { useAppStore } from "./state/app-store";
import {
  ZOOM_DEFAULT,
  isZoomIn,
  isZoomOut,
  isZoomReset,
  stepZoomIn,
  stepZoomOut,
} from "./utils/zoom";
import {
  GRID_MIN_WIDTH,
  RIGHT_PANE_DEFAULT_WIDTH,
  RIGHT_PANE_MIN_WIDTH,
  SIDEBAR_DEFAULT_WIDTH,
  SIDEBAR_MIN_WIDTH,
  SPLITTER_WIDTH,
  clampPaneWidths,
  computeMinWindowHeight,
  computeMinWindowWidth,
} from "./utils/windowSizing";
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
import { isHelpShortcut, isSettingsShortcut, primaryModWord } from "./utils/shortcuts";
import { hasOpenModal } from "./utils/modalStack";
import { Menu, MenuItem, MenuSeparator } from "./components/Menu";
import AboutModal from "./components/AboutModal";
import ScenesModal from "./components/ScenesModal";
import ConfirmDialog from "./components/ConfirmDialog";
import { Menu as MenuIcon } from "lucide-react";
import { openPath } from "@tauri-apps/plugin-opener";
import { useWizardStore } from "./state/wizard-store";
import { useComparisonStore } from "./state/comparison-store";
import { useIssuesStore } from "./state/issues-store";
import { ffmpegRole, useBinariesStore } from "./state/binaries-store";
import { itemKey } from "./state/items-store";
import { usePreviewStore } from "./state/preview-store";
import PreviewSurface from "./components/PreviewSurface";

// The main-window shell: the sidebar listbox, the thumbnail grid for the
// selected section, the tabbed right pane, and the scan lifecycle in the
// status bar.

export default function App() {
  const appData = useAppStore((s) => s.appData);
  const loadError = useAppStore((s) => s.loadError);
  const counts = useSectionsStore((s) => s.counts);
  const scanning = useSectionsStore((s) => s.scanning);
  const progress = useSectionsStore((s) => s.progress);
  const rescanNeeded = useSectionsStore((s) => s.rescanNeeded);
  const itemsMessage = useItemsStore((s) => s.message);
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
  const substitutedDirs = useWizardStore((s) => s.substitutedDirs);
  const [rightTab, setRightTabRaw] = useState<"details" | "destinations">("details");
  const setRightTab = (tab: "details" | "destinations") => {
    setRightTabRaw(tab);
    void useAppStore.getState().patchState({ rightPaneTab: tab });
  };
  const issuesOpen = useIssuesStore((s) => s.open);
  const issuesTotal = useIssuesStore((s) => s.total);
  const setIssuesOpen = useIssuesStore((s) => s.setOpen);
  const ffmpegState = useBinariesStore((s) => s.state);
  const binariesInstalling = useBinariesStore((s) => s.installing);
  const binariesProgress = useBinariesStore((s) => s.progress);
  const setBinariesModalOpen = useBinariesStore((s) => s.setModalOpen);
  const [helpOpen, setHelpOpen] = useState(false);
  const [aboutOpen, setAboutOpen] = useState(false);
  /** Enter on an anchor video opens the scenes modal for this hash. */
  const [scenesFor, setScenesFor] = useState<string | null>(null);
  /** Pending permanent deletion awaiting confirmation (item count shown). */
  const [confirmPermanent, setConfirmPermanent] = useState<number | null>(null);
  const previewFollow = usePreviewStore((s) => s.follow);
  const previewPlacement = usePreviewStore((s) => s.placement);
  const previewCurrent = usePreviewStore((s) => s.current);
  const splitRatio = usePreviewStore((s) => s.splitRatio);
  const splitOpen = previewFollow && previewPlacement === "split";
  const mainColRef = useRef<HTMLElement | null>(null);

  const beginSplitDrag = (event: React.MouseEvent) => {
    event.preventDefault();
    const startY = event.clientY;
    const startRatio = usePreviewStore.getState().splitRatio;
    const height = mainColRef.current?.clientHeight ?? 1;
    document.body.classList.add("row-resizing");
    const onMove = (e: MouseEvent) => {
      usePreviewStore.getState().setSplitRatio(startRatio + (e.clientY - startY) / height);
    };
    const onUp = () => {
      document.body.classList.remove("row-resizing");
      document.removeEventListener("mousemove", onMove);
      document.removeEventListener("mouseup", onUp);
      void useAppStore
        .getState()
        .patchState({ previewSplitRatio: usePreviewStore.getState().splitRatio });
    };
    document.addEventListener("mousemove", onMove);
    document.addEventListener("mouseup", onUp);
  };

  // The one open-Settings path — the menu item and the Cmd+, chord both use
  // it, so the two can never drift.
  const openSettings = () =>
    useSettingsStore.getState().openWith(useAppStore.getState().appData?.config ?? null);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement) {
        return;
      }
      if (isHelpShortcut(event)) {
        event.preventDefault();
        // Over another modal the chord only closes an already-open help —
        // never stacks help on top of Settings or Managed tools.
        setHelpOpen((open) => (open ? false : hasOpenModal() ? open : true));
      } else if (isSettingsShortcut(event)) {
        event.preventDefault();
        if (!hasOpenModal()) openSettings();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  // A newly opened preview window asks for the current selection; answer
  // with the payload AND the already-fetched detail (the window queries
  // nothing itself).
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    void import("@tauri-apps/api/event").then(async ({ listen, emit }) => {
      const fn = await listen("preview://ready", () => {
        const { items, selectedItem, detail } = useItemsStore.getState();
        const item = items.find((i) => itemKey(i) === selectedItem);
        if (item) {
          void emit("preview://show", {
            hash: item.hash,
            pathId: item.hash === null ? item.pathId : null,
            detail,
          });
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

  // Pane widths: persisted INTENT in pixels (written only on drag-end); the
  // displayed width is the intent clamped against the live container width,
  // so a narrow window never rewrites what the user chose.
  const [paneIntents, setPaneIntents] = useState({
    left: SIDEBAR_DEFAULT_WIDTH,
    right: RIGHT_PANE_DEFAULT_WIDTH,
  });
  const paneIntentsRef = useRef(paneIntents);
  paneIntentsRef.current = paneIntents;
  const [containerWidth, setContainerWidth] = useState<number | null>(null);
  const contentRowRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    const row = contentRowRef.current;
    if (!row) return;
    const measure = () => setContainerWidth(row.clientWidth);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(row);
    return () => observer.disconnect();
  }, []);
  const paneWidths = clampPaneWidths(
    paneIntents.left,
    paneIntents.right,
    containerWidth ?? computeMinWindowWidth() * 4,
  );

  const beginPaneDrag = (side: "left" | "right") => (event: React.MouseEvent) => {
    event.preventDefault();
    const startX = event.clientX;
    const start = { ...paneIntentsRef.current };
    // Window-wide resize cursor for the drag's duration — the pointer roams
    // over elements carrying their own cursors otherwise (cursor conventions).
    document.body.classList.add("col-resizing");
    const onMove = (e: MouseEvent) => {
      const delta = e.clientX - startX;
      const next =
        side === "left"
          ? { ...paneIntentsRef.current, left: Math.max(SIDEBAR_MIN_WIDTH, start.left + delta) }
          : {
              ...paneIntentsRef.current,
              right: Math.max(RIGHT_PANE_MIN_WIDTH, start.right - delta),
            };
      setPaneIntents(next);
    };
    const onUp = () => {
      document.body.classList.remove("col-resizing");
      document.removeEventListener("mousemove", onMove);
      document.removeEventListener("mouseup", onUp);
      void useAppStore.getState().patchState({
        sidebarWidth: paneIntentsRef.current.left,
        rightPaneWidth: paneIntentsRef.current.right,
      });
    };
    document.addEventListener("mousemove", onMove);
    document.addEventListener("mouseup", onUp);
  };

  const zoomRef = useRef(ZOOM_DEFAULT);
  // Reactive mirror of zoomRef so the menu's zoom stepper can display it.
  const [zoomLevel, setZoomLevel] = useState(ZOOM_DEFAULT);

  // The one zoom application path: keys and the menu stepper both use it.
  const applyZoom = (next: number) => {
    zoomRef.current = next;
    setZoomLevel(next);
    void getCurrentWebview().setZoom(next).catch(() => {});
    // Through the one state owner (patch of only this key, debounced) —
    // a failed write is logged there, never silently swallowed.
    void useAppStore.getState().patchState({ zoomLevel: next });
  };

  useEffect(() => {
    if (appData === null) return;
    const stored = appData.state?.zoomLevel;
    const level = typeof stored === "number" ? stored : ZOOM_DEFAULT;
    zoomRef.current = level;
    setZoomLevel(level);
    if (level !== ZOOM_DEFAULT) {
      void getCurrentWebview().setZoom(level).catch(() => {});
    }
  }, [appData]);

  useEffect(() => {
    const onZoomKey = (event: KeyboardEvent) => {
      if (hasOpenModal()) return;
      const zoomIn = isZoomIn(event);
      const zoomOut = isZoomOut(event);
      const zoomReset = isZoomReset(event);
      if (!zoomIn && !zoomOut && !zoomReset) return;
      event.preventDefault();
      applyZoom(
        zoomReset
          ? ZOOM_DEFAULT
          : zoomIn
            ? stepZoomIn(zoomRef.current)
            : stepZoomOut(zoomRef.current),
      );
    };
    window.addEventListener("keydown", onZoomKey);
    return () => window.removeEventListener("keydown", onZoomKey);
    // applyZoom is stable in behavior; the ref carries the current level.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Grid keys: Delete/Backspace trash-deletes the selected logical item
  // (every copy; Shift = permanent); Enter opens the comparison view when the
  // selection has similar photos. Ignored while typing in a form control or
  // while the comparison view owns the keyboard.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      // The command layer goes quiet while ANY modal is open: Backspace over
      // an open dialog must never trash files behind the backdrop.
      if (hasOpenModal()) return;
      if (useComparisonStore.getState().open) return;
      // A composite that already acted on this key has claimed it. React 19
      // dispatches from #root, so the native event keeps bubbling to this
      // window listener afterwards — without this, Enter on a destination row
      // moves the file AND opens the comparison view for the item being moved.
      if (event.defaultPrevented) return;
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
        if (event.shiftKey) {
          // Permanent deletion always confirms with the count (the design's
          // rule); trash deletion stays instant — the trash is the net.
          const { selectedKeys, selectedItem } = useItemsStore.getState();
          const count = selectedKeys.size > 0 ? selectedKeys.size : selectedItem !== null ? 1 : 0;
          if (count > 0) setConfirmPermanent(count);
        } else {
          void useItemsStore.getState().deleteSelected(false);
        }
      } else if (event.key.toLowerCase() === "p" && !event.metaKey && !event.ctrlKey && !event.altKey) {
        // Preview follow toggle (FastStone's model): on opens the surface for
        // the current anchor; off closes it by any placement.
        event.preventDefault();
        void import("./state/preview-store").then(({ usePreviewStore, showPreview }) => {
          const preview = usePreviewStore.getState();
          if (preview.follow) {
            preview.close();
            return;
          }
          const { items, selectedItem } = useItemsStore.getState();
          const item = items.find((i) => itemKey(i) === selectedItem);
          if (item) {
            void showPreview({
              hash: item.hash,
              pathId: item.hash === null ? item.pathId : null,
            });
          } else {
            // No anchor yet: arm follow; the first selection opens it.
            usePreviewStore.setState({ follow: true });
          }
        });
      } else if (event.key === "Enter") {
        const { items, selectedItem } = useItemsStore.getState();
        const item = items.find((i) => itemKey(i) === selectedItem);
        if (!item) return;
        event.preventDefault();
        if (item.hash && item.durationMs !== null) {
          // Enter goes deeper on the anchor: a video opens the scenes modal
          // (selection-based culling — videos never group).
          setScenesFor(item.hash);
        } else if (item.hash && item.similarGroupId !== null) {
          // Similar photos exist: Enter means "show them all at once" — and
          // when the group turns out to have no other live members, Enter
          // falls back to the preview instead of doing nothing.
          void useComparisonStore
            .getState()
            .openGroup(item.hash)
            .then((opened) => {
              if (opened) return;
              return import("./state/preview-store").then(({ showPreview }) =>
                showPreview({ hash: item.hash, pathId: null }),
              );
            });
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

  // One-shot state restore once the persisted document is in: sort order,
  // right-pane tab, Issues-open, and the last-open section + anchor (the
  // section restore waits for counts so it never selects a vanished month).
  const restoredRef = useRef(false);
  useEffect(() => {
    if (restoredRef.current || appData === null || counts === null) return;
    restoredRef.current = true;
    const state = appData.state ?? {};
    const sort = state.sortOrder;
    if (sort === "time" || sort === "name" || sort === "size" || sort === "resolution") {
      useItemsStore.setState({ sortOrder: sort });
    }
    if (state.rightPaneTab === "destinations") setRightTabRaw("destinations");
    if (state.issuesOpen === true) useIssuesStore.getState().setOpen(true);
    const left = state.sidebarWidth;
    const right = state.rightPaneWidth;
    setPaneIntents((current) => ({
      left: typeof left === "number" && Number.isFinite(left) ? left : current.left,
      right: typeof right === "number" && Number.isFinite(right) ? right : current.right,
    }));
    void import("./state/preview-store").then(({ usePreviewStore }) =>
      usePreviewStore
        .getState()
        .restoreFollow(
          state.previewFollow === true,
          typeof state.previewSplitRatio === "number" ? state.previewSplitRatio : null,
        ),
    );
    const last = state.lastSection as { kind?: string; month?: string } | undefined;
    if (
      last &&
      (last.kind === "image" || last.kind === "video" || last.kind === "other") &&
      typeof last.month === "string"
    ) {
      const lists =
        last.kind === "image" ? counts.images : last.kind === "video" ? counts.videos : counts.others;
      if (lists.some((s) => s.month === last.month)) {
        void useItemsStore
          .getState()
          .select({ kind: last.kind, month: last.month })
          .then(() => {
            const anchor = state.lastItem;
            if (typeof anchor !== "string") return;
            const { items } = useItemsStore.getState();
            if (items.some((i) => itemKey(i) === anchor)) {
              useItemsStore.getState().selectItem(anchor);
            }
          });
      }
    }
  }, [appData, counts]);

  const allEmpty =
    counts !== null &&
    counts.images.length === 0 &&
    counts.videos.length === 0 &&
    counts.others.length === 0;

  return (
    <div className="flex h-screen flex-col bg-background text-ink">
      {wizardOpen && appData !== null ? (
        <Wizard dataRoot={appData.dataRoot} />
      ) : missingDirs.length > 0 || substitutedDirs.length > 0 ? (
        <PresenceGate missing={missingDirs} substituted={substitutedDirs} />
      ) : null}
      <ComparisonView />
      <BinariesModal />
      <ShortcutsModal open={helpOpen} onClose={() => setHelpOpen(false)} />
      <AboutModal open={aboutOpen} onClose={() => setAboutOpen(false)} />
      {scenesFor !== null ? (
        <ScenesModal hash={scenesFor} onClose={() => setScenesFor(null)} />
      ) : null}
      {confirmPermanent !== null ? (
        <ConfirmDialog
          title="Delete permanently?"
          message={`Permanently delete ${confirmPermanent} item${
            confirmPermanent === 1 ? "" : "s"
          } and every copy? This bypasses the trash and cannot be undone.`}
          confirmLabel="Delete permanently"
          onConfirm={() => {
            setConfirmPermanent(null);
            void useItemsStore.getState().deleteSelected(true);
          }}
          onCancel={() => setConfirmPermanent(null)}
        />
      ) : null}
      <SettingsModal />
      <div ref={contentRowRef} className="flex min-h-0 flex-1">
        <aside
          style={{ width: paneWidths.left }}
          className="shrink-0 overflow-y-auto bg-surface p-3"
        >
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
        <div
          role="separator"
          aria-orientation="vertical"
          aria-label="Resize sidebar"
          style={{ width: SPLITTER_WIDTH }}
          className="shrink-0 cursor-col-resize bg-border hover:bg-border-strong"
          onMouseDown={beginPaneDrag("left")}
        />
        <main
          ref={mainColRef}
          style={{ minWidth: GRID_MIN_WIDTH }}
          className="flex min-w-0 flex-1 flex-col overflow-hidden"
        >
          {splitOpen ? (
            <>
              {/* Single-monitor placement: the preview rides ABOVE the grid —
                  a separate window would cover it and steal the keyboard. */}
              <div
                style={{ height: `${splitRatio * 100}%` }}
                className="relative min-h-24 shrink-0 overflow-hidden"
              >
                <PreviewSurface
                  hash={previewCurrent?.hash ?? null}
                  detail={previewCurrent?.detail ?? null}
                />
                <button
                  aria-label="Close preview"
                  title="Close preview (P)"
                  className="absolute right-2 top-2 rounded bg-surface/80 px-1.5 text-sm text-ink-muted hover:bg-surface hover:text-ink"
                  onClick={() => usePreviewStore.getState().close()}
                >
                  ✕
                </button>
              </div>
              <div
                role="separator"
                aria-orientation="horizontal"
                aria-label="Resize preview"
                className="h-1 shrink-0 cursor-row-resize bg-border hover:bg-border-strong"
                onMouseDown={beginSplitDrag}
              />
            </>
          ) : null}
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
        <div
          role="separator"
          aria-orientation="vertical"
          aria-label="Resize details pane"
          style={{ width: SPLITTER_WIDTH }}
          className="shrink-0 cursor-col-resize bg-border hover:bg-border-strong"
          onMouseDown={beginPaneDrag("right")}
        />
        <aside
          style={{ width: paneWidths.right }}
          className="flex shrink-0 flex-col overflow-hidden bg-surface"
        >
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
                id={`right-tab-${tab}`}
                role="tab"
                aria-selected={rightTab === tab}
                aria-controls="right-tabpanel"
                tabIndex={rightTab === tab ? 0 : -1}
                className={`flex-1 px-2 py-1 text-xs ${
                  rightTab === tab
                    ? "border-b-2 border-primary font-semibold text-primary"
                    : "text-ink-muted hover:text-ink"
                }`}
                onClick={() => setRightTab(tab)}
                onKeyDown={(event) => {
                  // Home/End jump to the ends; arrows STOP at them (the
                  // app-wide end-of-axis choice — the grid and sidebar clamp).
                  const target =
                    event.key === "ArrowRight"
                      ? Math.min(index + 1, tabs.length - 1)
                      : event.key === "ArrowLeft"
                        ? Math.max(index - 1, 0)
                        : event.key === "Home"
                          ? 0
                          : event.key === "End"
                            ? tabs.length - 1
                            : null;
                  if (target === null || target === index) return;
                  event.preventDefault();
                  setRightTab(tabs[target]);
                  (event.currentTarget.parentElement?.children[target] as
                    | HTMLElement
                    | undefined)?.focus();
                }}
              >
                {tab === "details" ? "Details" : "Destinations"}
              </button>
            ))}
          </div>
          {/* Focusable so PageUp/PageDown/arrows scroll a long copy-path list
              — the pane holds nothing else focusable, and the tab buttons are
              siblings whose keydowns never reach this scroller. */}
          <div
            id="right-tabpanel"
            role="tabpanel"
            aria-labelledby={`right-tab-${rightTab}`}
            tabIndex={0}
            className="min-h-0 flex-1 overflow-y-auto outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-primary-ring"
          >
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
        {/* Standing STATE stays glanceable in the status bar; the actions
            live in the menu (app-chrome: a curated summary surface). */}
        <span>
          {/* A failed delete outranks standing state: it is the one thing the
              user just did that did not happen, and silence there reads as
              "Delete does nothing". */}
          {itemsMessage ? (
            <span className="text-danger" title={itemsMessage}>
              {itemsMessage}
            </span>
          ) : scanning ? (
            progress
          ) : rescanNeeded ? (
            <span
              className="text-warning"
              title="The watcher lost events — run Scan all sources to repair the index"
            >
              Rescan needed
            </span>
          ) : (
            ""
          )}
        </span>
        <span className="flex items-center gap-3">
          <button
            className={
              ffmpegRole(ffmpegState?.status ?? null) === "warning"
                ? "text-warning hover:underline"
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
          <Menu
            ariaLabel="Application menu"
            panelClassName="bottom-full right-0 mb-1"
            trigger={(props) => (
              <button
                {...props}
                aria-label="Open menu"
                className="flex h-6 w-6 items-center justify-center rounded text-ink-muted hover:bg-surface-muted hover:text-ink"
              >
                <MenuIcon size={16} />
              </button>
            )}
          >
            <MenuItem disabled={scanning} onSelect={() => void startScan()}>
              {scanning ? "Scanning…" : "Scan all sources"}
            </MenuItem>
            <MenuItem
              onSelect={() =>
                useWizardStore.getState().reopen(useAppStore.getState().appData?.config ?? null)
              }
            >
              Re-run setup wizard…
            </MenuItem>
            <MenuSeparator />
            <MenuItem onSelect={openSettings} shortcut={`${primaryModWord()}+Comma`}>
              Settings…
            </MenuItem>
            <MenuItem onSelect={() => setBinariesModalOpen(true)}>Managed tools…</MenuItem>
            <MenuSeparator />
            {/* A contained widget, not menu items — arrow navigation skips it
                because only [role="menuitem"] participates. */}
            <div className="flex items-center justify-between gap-2 px-3 py-1 text-sm text-ink">
              <span>Zoom</span>
              <span className="flex items-center gap-1">
                <button
                  className="h-5 w-5 rounded border border-border text-xs hover:bg-surface-muted"
                  aria-label="Zoom out"
                  onClick={() => applyZoom(stepZoomOut(zoomRef.current))}
                >
                  −
                </button>
                <span className="w-10 text-center font-mono text-xs text-ink-muted">
                  {Math.round(zoomLevel * 100)}%
                </span>
                <button
                  className="h-5 w-5 rounded border border-border text-xs hover:bg-surface-muted"
                  aria-label="Zoom in"
                  onClick={() => applyZoom(stepZoomIn(zoomRef.current))}
                >
                  +
                </button>
              </span>
            </div>
            <MenuSeparator />
            <MenuItem
              onSelect={() => {
                const root = useAppStore.getState().appData?.dataRoot;
                if (root) void openPath(`${root}/logs`);
              }}
            >
              Reveal logs folder
            </MenuItem>
            <MenuItem
              onSelect={() => {
                const root = useAppStore.getState().appData?.dataRoot;
                if (root) void openPath(root);
              }}
            >
              Reveal app home
            </MenuItem>
            <MenuSeparator />
            <MenuItem
              onSelect={() => setHelpOpen(true)}
              shortcut={`${primaryModWord()}+Slash`}
            >
              Keyboard shortcuts…
            </MenuItem>
            <MenuItem onSelect={() => setAboutOpen(true)}>About OneCopy…</MenuItem>
          </Menu>
        </span>
      </footer>
    </div>
  );
}
