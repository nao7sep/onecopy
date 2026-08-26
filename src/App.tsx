import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";
import { useAppStore } from "./state/app-store";
import {
  GRID_MIN_WIDTH,
  HEADER_HEIGHT,
  SPLITTER_WIDTH,
} from "./utils/windowSizing";
import { useSectionsStore } from "./state/sections-store";
import { statusLine } from "./models/status";
import { useItemsStore } from "./state/items-store";
import Sidebar from "./components/Sidebar";
import Grid from "./components/Grid";
import MetadataPane from "./components/MetadataPane";
import DestinationsTab from "./components/DestinationsTab";
import Wizard from "./components/Wizard";
import PresenceGate from "./components/PresenceGate";
import ComparisonView from "./components/ComparisonView";
import IssuesModal from "./components/IssuesModal";
import QuarantineNotice from "./components/QuarantineNotice";
import BinariesModal from "./components/BinariesModal";
import ShortcutsModal from "./components/ShortcutsModal";
import SettingsModal from "./components/SettingsModal";
import { Menu, MenuItem, MenuSeparator } from "./components/Menu";
import AboutModal from "./components/AboutModal";
import QuickView from "./components/QuickView";
import TrashModal from "./components/TrashModal";
import ConfirmDialog from "./components/ConfirmDialog";
import { Menu as MenuIcon, Minus, Plus, X } from "lucide-react";
import { useWizardStore } from "./state/wizard-store";
import { useIssuesStore } from "./state/issues-store";
import {
  toolsChip,
  useBinariesStore,
} from "./state/binaries-store";
import { usePreviewStore } from "./state/preview-store";
import { useQuickViewStore } from "./state/quick-view-store";
import {
  backgroundWorkLine,
  useDerivedWorkStore,
} from "./state/derived-work-store";
import PreviewSurface from "./components/PreviewSurface";
import { log, toErrorFields } from "./repositories";
import BackgroundWorkModal from "./components/BackgroundWorkModal";
import { closePreview } from "./workflows/preview";
import { useAppBootstrapAndRestore } from "./hooks/useAppBootstrapAndRestore";
import { useGlobalCommands } from "./hooks/useGlobalCommands";
import { useMainWindowLifecycle } from "./hooks/useMainWindowLifecycle";
import { usePaneLayout } from "./hooks/usePaneLayout";

function ZoomOutIcon() {
  return <Minus aria-hidden="true" className="inline-block h-[1em] w-[1em]" />;
}

function ZoomInIcon() {
  return <Plus aria-hidden="true" className="inline-block h-[1em] w-[1em]" />;
}

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
  const issuesTotal = useIssuesStore((s) => s.total);
  const derivedWorkSnapshot = useDerivedWorkStore((s) => s.snapshot);
  const setBackgroundWorkOpen = useDerivedWorkStore((s) => s.setOpen);
  const derivedWorkLine = backgroundWorkLine(derivedWorkSnapshot);
  const setIssuesOpen = useIssuesStore((s) => s.setOpen);
  const binariesEntries = useBinariesStore((s) => s.entries);
  // The chip narrates ffmpeg's own install only; a model download in flight
  // is the modal's story.
  const ffmpegProgress = useBinariesStore((s) => s.installing["ffmpeg"]);
  const setBinariesModalOpen = useBinariesStore((s) => s.setModalOpen);
  const [aboutOpen, setAboutOpen] = useState(false);
  const [trashOpen, setTrashOpen] = useState(false);
  /** Transient media inspection lives in the main webview. */
  const quickViewOpen = useQuickViewStore((state) => state.open);
  const previewFollow = usePreviewStore((s) => s.follow);
  const previewPlacement = usePreviewStore((s) => s.placement);
  const previewCurrent = usePreviewStore((s) => s.current);
  const splitOpen = previewFollow && previewPlacement === "split";
  const { contentRowRef, paneWidths, beginPaneDrag, restorePaneIntents } =
    usePaneLayout(splitOpen);
  const { rightTab, setRightTab } = useAppBootstrapAndRestore({
    appData,
    counts,
    restorePaneIntents,
  });
  const {
    helpOpen,
    openHelp,
    closeHelp,
    openSettings,
    confirmPermanent,
    confirmTrash,
    cancelPermanentDelete,
    cancelTrashDelete,
    confirmPermanentDelete,
    confirmTrashDelete,
  } = useGlobalCommands();
  const { zoomLevel, zoomIn, zoomOut } = useMainWindowLifecycle({
    appData,
    loadError,
    splitOpen,
  });

  const status = statusLine({
    message: itemsMessage,
    scanning,
    progress,
    rescanNeeded,
    counts,
  });

  const allEmpty =
    counts !== null &&
    counts.images.length === 0 &&
    counts.videos.length === 0 &&
    counts.others.length === 0;

  // The boot gates: opaque full-screen overlays that are NOT part of the modal
  // stack, so anything behind them has to be told about them explicitly.
  const gateOpen = missingDirs.length > 0 || substitutedDirs.length > 0;

  return (
    <div className="flex h-screen flex-col bg-background text-ink">
      {wizardOpen && appData !== null ? (
        <Wizard />
      ) : gateOpen ? (
        <PresenceGate missing={missingDirs} substituted={substitutedDirs} />
      ) : null}
      <ComparisonView />
      <BinariesModal />
      <BackgroundWorkModal />
      <ShortcutsModal open={helpOpen} onClose={closeHelp} />
      <AboutModal open={aboutOpen} onClose={() => setAboutOpen(false)} />
      {quickViewOpen ? <QuickView /> : null}
      {confirmPermanent !== null ? (
        <ConfirmDialog
          title="Delete permanently?"
          message={`Permanently delete ${confirmPermanent} item${
            confirmPermanent === 1 ? "" : "s"
          } and every copy? This bypasses the trash and cannot be undone.`}
          confirmLabel="Delete permanently"
          onConfirm={confirmPermanentDelete}
          onCancel={cancelPermanentDelete}
        />
      ) : null}
      {confirmTrash !== null ? (
        <ConfirmDialog
          title="Move to trash?"
          message={`Move ${confirmTrash} item${
            confirmTrash === 1 ? "" : "s"
          } and every copy to the trash? Recoverable from Menu → Trash.`}
          confirmLabel="Move to trash"
          onConfirm={confirmTrashDelete}
          onCancel={cancelTrashDelete}
        />
      ) : null}
      <SettingsModal />
      <IssuesModal />
      {/* Renders itself only when the core set a store aside this launch. */}
      <QuarantineNotice />
      <TrashModal open={trashOpen} onClose={() => setTrashOpen(false)} />
      <div ref={contentRowRef} className="flex min-h-0 flex-1">
        <aside
          style={{ width: paneWidths.left }}
          className="flex shrink-0 flex-col overflow-hidden bg-surface"
        >
          {/* Title section. Deliberately scoped to the SIDEBAR's width rather
              than spanning the window: a full-width band would cost every
              pane its height, and only this one has room to spare. The
              hamburger sits at its right end. The version is not here — it
              belongs to About, and a permanent version number is not standing
              state anyone needs at a glance. */}
          <header
            style={{ height: HEADER_HEIGHT }}
            className="flex shrink-0 items-center justify-between gap-2 border-b border-border pl-4 pr-2"
          >
            <h1 className="truncate text-base font-semibold tracking-tight text-ink-strong">
              OneCopy
            </h1>
            <Menu
              ariaLabel="Application menu"
              trigger={(props) => (
                <button
                  {...props}
                  aria-label="Open menu"
                  className="flex h-8 w-8 shrink-0 items-center justify-center rounded-md text-ink-muted transition-colors hover:bg-surface-muted hover:text-ink"
                >
                  <MenuIcon size={18} />
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
              <MenuItem onSelect={openSettings}>Settings…</MenuItem>
              <MenuItem onSelect={() => setBinariesModalOpen(true)}>Managed tools…</MenuItem>
              <MenuItem onSelect={() => setTrashOpen(true)}>Trash…</MenuItem>
              <MenuSeparator />
              {/* A contained widget, not menu items — arrow navigation skips it
                  because only [role="menuitem"] participates. */}
              <div className="flex items-center justify-between gap-2 px-3 py-1 text-sm text-ink">
                <span>Zoom</span>
                <span className="flex items-center gap-1">
                  <button
                    className="flex h-5 w-5 items-center justify-center rounded border border-border text-xs hover:bg-surface-muted"
                    aria-label="Zoom out"
                    onClick={zoomOut}
                  >
                    <ZoomOutIcon />
                  </button>
                  <span className="w-10 text-center font-mono text-xs text-ink-muted">
                    {Math.round(zoomLevel * 100)}%
                  </span>
                  <button
                    className="flex h-5 w-5 items-center justify-center rounded border border-border text-xs hover:bg-surface-muted"
                    aria-label="Zoom in"
                    onClick={zoomIn}
                  >
                    <ZoomInIcon />
                  </button>
                </span>
              </div>
              <MenuSeparator />
              <MenuItem
                onSelect={() => {
                  // The core builds the path (paths.rs is the one authority);
                  // the old frontend-built `openPath` was silently rejected by
                  // the opener plugin's empty path scope. "Reveal app home"
                  // was dropped with the fix — the data root is
                  // machine-managed and no fleet app reveals its own innards.
                  void invoke("reveal_data_subdir", { name: "logs" }).catch((error) =>
                    log.warn("reveal logs failed", toErrorFields(error)),
                  );
                }}
              >
                Reveal logs folder
              </MenuItem>
              <MenuSeparator />
              <MenuItem onSelect={openHelp}>Keyboard shortcuts…</MenuItem>
              <MenuItem onSelect={() => setAboutOpen(true)}>About OneCopy…</MenuItem>
            </Menu>
          </header>
          <div className="min-h-0 flex-1 overflow-y-auto p-2">
            <Sidebar counts={counts} />
          </div>
        </aside>
        <div
          role="separator"
          aria-orientation="vertical"
          aria-label="Resize sidebar"
          // The element IS the hit area; the 1px child is the visible line. At
          // 4px the target was too small to catch, which reads as "the panes
          // do not resize" rather than as a near miss.
          style={{ width: SPLITTER_WIDTH }}
          className="group flex shrink-0 cursor-col-resize justify-center"
          onMouseDown={beginPaneDrag("left")}
        >
          <div className="w-px bg-border transition-colors group-hover:bg-border-strong" />
        </div>
        <main
          style={{ minWidth: GRID_MIN_WIDTH }}
          className="flex min-w-0 flex-1 flex-col overflow-hidden"
        >
          {loadError !== null ? (
            <p className="m-auto text-danger">{loadError}</p>
          ) : selected !== null ? (
            <Grid
              items={items}
              loading={itemsLoading}
              layout={selected.kind === "other" ? "list" : "tiles"}
              mayClaimFocus={!wizardOpen && !gateOpen}
            />
          ) : allEmpty ? (
            // Mid-scan the counts are empty because nothing has been indexed
            // YET, not because there is nothing — the same distinction the
            // sidebar's empty state makes.
            <p className="m-auto text-ink-muted">
              {scanning ? "Scanning…" : "Nothing to handle"}
            </p>
          ) : (
            <p className="m-auto text-ink-muted">Select a month</p>
          )}
        </main>
        {splitOpen ? (
          <>
            <div
              role="separator"
              aria-orientation="vertical"
              aria-label="Resize preview"
              style={{ width: SPLITTER_WIDTH }}
              className="group flex shrink-0 cursor-col-resize justify-center"
              onMouseDown={beginPaneDrag("preview")}
            >
              <div className="w-px bg-border transition-colors group-hover:bg-border-strong" />
            </div>
            {/* The in-window preview: a SIDE pane, because screens are wide —
                sidebar › grid › preview › right pane. It renders the anchor
                from `current`, which `open` seeds immediately, so activating
                the preview with a selection shows that image at once. */}
            <div
              style={{ width: paneWidths.preview }}
              className="relative shrink-0 overflow-hidden bg-surface"
            >
              <PreviewSurface
                hash={previewCurrent?.hash ?? null}
                detail={previewCurrent?.detail ?? null}
                pathId={previewCurrent?.pathId ?? null}
                seekMs={previewCurrent?.seekMs}
                playAfterSeek={previewCurrent?.playAfterSeek}
              />
              <button
                aria-label="Close preview"
                title="Close preview (Space)"
                className="absolute right-2 top-2 flex h-6 w-6 items-center justify-center rounded-md bg-surface/80 text-ink-muted transition-colors hover:bg-surface-muted hover:text-ink"
                onClick={closePreview}
              >
                <X size={14} />
              </button>
            </div>
          </>
        ) : null}
        <div
          role="separator"
          aria-orientation="vertical"
          aria-label="Resize details pane"
          style={{ width: SPLITTER_WIDTH }}
          className="group flex shrink-0 cursor-col-resize justify-center"
          onMouseDown={beginPaneDrag("right")}
        >
          <div className="w-px bg-border transition-colors group-hover:bg-border-strong" />
        </div>
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
                className={`flex-1 px-2 py-2.5 text-sm ${
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
            className="min-h-0 flex-1 overflow-y-auto outline-none"
          >
            {rightTab === "details" ? (
              <MetadataPane detail={detail} hash={selectedHash} />
            ) : (
              <DestinationsTab />
            )}
          </div>
        </aside>
      </div>
      {/* The status bar: standing state always, transient conditions on top.
          What it says is decided by `statusLine`, which is where the priority
          order and the "never blank" rule live. */}
      <footer className="flex shrink-0 items-center justify-between gap-3 border-t border-border bg-surface px-3 py-1 text-xs">
        <span
          className={`min-w-0 truncate ${
            status.tone === "danger"
              ? "text-danger"
              : status.tone === "warning"
                ? "text-warning"
                : "text-ink-muted"
          }`}
          title={status.title}
        >
          {status.text}
        </span>
        <span className="flex shrink-0 items-center gap-3">
          {/* Why the fans spin while work runs in the background. */}
          <button
            className="text-ink-muted hover:text-ink hover:underline"
            title="Open Background work"
            onClick={() => setBackgroundWorkOpen(true)}
          >
            {derivedWorkLine}
          </button>
          {/* The issues count: NOTHING at zero, a danger-tinted count when
              conditions exist. No toasts anywhere — the design case is a
              multi-day unattended scan, so the count simply waits here. */}
          {issuesTotal > 0 ? (
            <button
              className="text-danger hover:underline"
              title="Open the issues list"
              onClick={() => setIssuesOpen(true)}
            >
              {issuesTotal} issue{issuesTotal === 1 ? "" : "s"}
            </button>
          ) : null}
          {/* The managed-tools chip (toolsChip owns the words and the
              loudness — see its rules); clicking always opens the modal. */}
          {(() => {
            const chip = toolsChip(ffmpegProgress !== undefined, ffmpegProgress ?? "", binariesEntries);
            return chip !== null ? (
              <button
                className={
                  chip.role === "warning"
                    ? "text-warning hover:underline"
                    : "text-ink-muted hover:text-ink"
                }
                title="Managed tools"
                onClick={() => setBinariesModalOpen(true)}
              >
                {chip.text}
              </button>
            ) : null;
          })()}
        </span>
      </footer>
    </div>
  );
}
