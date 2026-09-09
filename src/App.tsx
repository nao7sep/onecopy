import { useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";
import { useAppStore } from "./state/app-store";
import {
  GRID_MIN_WIDTH,
  HEADER_HEIGHT,
  SPLITTER_WIDTH,
} from "./utils/windowSizing";
import { useSectionsStore } from "./state/sections-store";
import { activeMaintenanceStatus, statusLine } from "./models/status";
import { currentMainFeedback, useMainFeedbackStore } from "./state/main-feedback-store";
import { useItemsStore } from "./state/items-store";
import { itemKey } from "./models/items";
import Sidebar from "./components/Sidebar";
import Grid from "./components/Grid";
import MetadataPane from "./components/MetadataPane";
import DestinationsTab from "./components/DestinationsTab";
import Wizard from "./components/Wizard";
import {
  MissingSourcesNotice,
  SubstitutedSourceGate,
} from "./components/SourceAvailability";
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
import MutationResultActions from "./components/MutationResultActions";
import ConfirmDialog from "./components/ConfirmDialog";
import { Menu as MenuIcon, Minus, Plus, X } from "lucide-react";
import { useWizardStore } from "./state/wizard-store";
import { useIssuesStore } from "./state/issues-store";
import {
  toolsChip,
  useBinariesStore,
} from "./state/binaries-store";
import { managedInstallActivityLine } from "./models/dependencyProgress";
import { usePreviewStore } from "./state/preview-store";
import { useQuickViewStore } from "./state/quick-view-store";
import {
  backgroundWorkLine,
  useDerivedWorkStore,
} from "./state/derived-work-store";
import PreviewSurface from "./components/PreviewSurface";
import { log, toErrorFields, type LoadedAppData } from "./repositories";
import BackgroundWorkModal from "./components/BackgroundWorkModal";
import ActivityTraceModal from "./components/ActivityTraceModal";
import { closePreview } from "./workflows/preview";
import { useAppBootstrapAndRestore } from "./hooks/useAppBootstrapAndRestore";
import { useGlobalCommands } from "./hooks/useGlobalCommands";
import { useMainWindowLifecycle } from "./hooks/useMainWindowLifecycle";
import { usePaneLayout } from "./hooks/usePaneLayout";
import { useMutationStore } from "./state/mutation-store";
import { useDestinationDragBoundary } from "./hooks/useDestinationDragBoundary";
import DestinationDragProvider from "./components/DestinationDragProvider";
import { setMediumAutoplay, setSoundEnabled } from "./workflows/playback";
import NotificationHost from "./components/NotificationHost";
import StartupFailureScreen from "./components/StartupFailureScreen";
import { reportActionFailure } from "./state/notifications-store";
import { bootstrapApplication } from "./workflows/app-lifecycle";
import { useAppShellStore } from "./state/app-shell-store";

function ZoomOutIcon() {
  return <Minus aria-hidden="true" className="inline-block h-[1em] w-[1em]" />;
}

function ZoomInIcon() {
  return <Plus aria-hidden="true" className="inline-block h-[1em] w-[1em]" />;
}

// Loading, Blocked, and Ready are mutually exclusive roots. Feature hooks do
// not exist until the backend has admitted the Ready state.
export default function App() {
  const appData = useAppStore((state) => state.appData);
  const startupFailure = useAppStore((state) => state.startupFailure);
  const bootstrapStarted = useRef(false);

  useEffect(() => {
    if (bootstrapStarted.current) return;
    bootstrapStarted.current = true;
    void bootstrapApplication();
  }, []);

  if (startupFailure !== null) {
    return <StartupFailureScreen failure={startupFailure} />;
  }
  if (appData === null) {
    return (
      <div
        role="status"
        aria-label="Starting OneCopy"
        className="flex h-screen items-center justify-center bg-background text-ink-muted"
      >
        Starting OneCopy…
      </div>
    );
  }

  return <ReadyApp appData={appData} />;
}

// The admitted main-window shell: the sidebar listbox, thumbnail grid, right
// pane, and scan lifecycle in the status bar.
export function ReadyApp({ appData }: { appData: LoadedAppData }) {
  useDestinationDragBoundary();
  const soundEnabled = appData?.state?.soundEnabled !== false;
  const videoAutoplay = appData?.config?.videoAutoplay !== false;
  const audioAutoplay = appData?.config?.audioAutoplay !== false;
  const counts = useSectionsStore((s) => s.counts);
  const sourceCheck = useSectionsStore((s) => s.sourceCheck);
  const fileInformation = useSectionsStore((s) => s.fileInformation);
  const maintenanceStatus = activeMaintenanceStatus(sourceCheck, fileInformation);
  const { scanning } = maintenanceStatus;
  const rescanNeeded = useSectionsStore((s) => s.rescanNeeded);
  const feedback = useMainFeedbackStore(currentMainFeedback);
  const mutationProgress = useMutationStore((s) => s.progress);
  const mutationCancelling = useMutationStore((s) => s.cancelling);
  const mutationResult = useMutationStore((s) => s.result);
  const exitQuiescing = useMutationStore((s) => s.exiting);
  const cancelMutation = useMutationStore((s) => s.cancel);
  const dismissMutationResult = useMutationStore((s) => s.dismissResult);
  const startSourceCheck = useSectionsStore((s) => s.startSourceCheck);
  const stopSourceCheck = useSectionsStore((s) => s.stopSourceCheck);
  const selected = useItemsStore((s) => s.selected);
  const items = useItemsStore((s) => s.items);
  const itemsLoading = useItemsStore((s) => s.loading);
  const itemsLoadError = useItemsStore((s) => s.loadError);
  const detail = useItemsStore((s) => s.detail);
  const selectedItemKey = useItemsStore((s) => s.selectedItem);
  const selectedHash =
    selectedItemKey !== null && !selectedItemKey.startsWith("path-")
      ? selectedItemKey
      : null;
  const selectedSectionItem =
    selectedItemKey === null
      ? null
      : (items.find((item) => itemKey(item) === selectedItemKey) ?? null);
  const wizardOpen = useWizardStore((s) => s.open);
  const missingDirs = useWizardStore((s) => s.missingDirs);
  const substitutedDirs = useWizardStore((s) => s.substitutedDirs);
  const issuesTotal = useIssuesStore((s) => s.total);
  const derivedWorkSnapshot = useDerivedWorkStore((s) => s.snapshot);
  const derivedWorkLine = backgroundWorkLine(derivedWorkSnapshot);
  const binariesEntries = useBinariesStore((s) => s.entries);
  // The chip narrates ffmpeg's own install only; a model download in flight
  // is the modal's story.
  const ffmpegProgress = useBinariesStore((s) => s.installing["ffmpeg"]);
  const utilitySurface = useAppShellStore((s) => s.utilitySurface);
  const openUtility = useAppShellStore((s) => s.openUtility);
  const closeUtility = useAppShellStore((s) => s.closeUtility);
  /** Transient media inspection lives in the main webview. */
  const quickViewOpen = useQuickViewStore(
    (state) => state.session?.presentation === "quick",
  );
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
    openHelp,
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
    splitOpen,
  });

  const status = statusLine({
    feedback,
    mutation: mutationProgress === null
      ? null
      : { progress: mutationProgress, cancelling: mutationCancelling },
    mutationResult,
    exiting: exitQuiescing,
    ...maintenanceStatus,
    rescanNeeded,
    counts,
  });

  const allEmpty =
    counts !== null &&
    counts.images.length === 0 &&
    counts.videos.length === 0 &&
    counts.others.length === 0;

  // A substituted physical volume is unsafe to act on and remains a blocking
  // gate. Missing roots are ordinary availability failures: Main and every
  // independent available copy remain usable.
  const substitutedSourceGateOpen = substitutedDirs.length > 0;
  const reopenSetup = () =>
    useWizardStore
      .getState()
      .reopen(useAppStore.getState().appData?.config ?? null);

  return (
    <DestinationDragProvider>
    <div className="flex h-screen flex-col bg-background text-ink">
      {wizardOpen && appData !== null ? (
        <Wizard />
      ) : substitutedSourceGateOpen ? (
        <SubstitutedSourceGate
          substituted={substitutedDirs}
          onRecheck={() => void useWizardStore.getState().recheckPresence()}
          onReconfigure={reopenSetup}
        />
      ) : null}
      <ComparisonView onRevealTrash={() => openUtility("deletedFiles")} />
      <NotificationHost />
      <BinariesModal
        open={utilitySurface === "managedTools"}
        onClose={closeUtility}
      />
      <BackgroundWorkModal
        open={utilitySurface === "backgroundWork"}
        onClose={closeUtility}
      />
      {appData.debugEnabled ? (
        <ActivityTraceModal
          open={utilitySurface === "activityTrace"}
          onClose={closeUtility}
        />
      ) : null}
      <ShortcutsModal
        open={utilitySurface === "shortcuts"}
        onClose={closeUtility}
      />
      <AboutModal open={utilitySurface === "about"} onClose={closeUtility} />
      {quickViewOpen ? <QuickView /> : null}
      {confirmPermanent !== null ? (
        <ConfirmDialog
          title="Delete permanently?"
          message={`Permanently delete ${confirmPermanent} item${
            confirmPermanent === 1 ? "" : "s"
          } and every copy? This bypasses recoverable deletion and cannot be undone.`}
          confirmLabel="Delete permanently"
          onConfirm={confirmPermanentDelete}
          onCancel={cancelPermanentDelete}
        />
      ) : null}
      {confirmTrash !== null ? (
        <ConfirmDialog
          title="Delete these items?"
          message={`Delete ${confirmTrash} item${
            confirmTrash === 1 ? "" : "s"
          } and every copy? They remain recoverable from Menu → Deleted files.`}
          confirmLabel="Delete"
          onConfirm={confirmTrashDelete}
          onCancel={cancelTrashDelete}
        />
      ) : null}
      <SettingsModal open={utilitySurface === "settings"} onClose={closeUtility} />
      <IssuesModal open={utilitySurface === "issues"} onClose={closeUtility} />
      {/* Renders itself only when the core set a store aside this launch. */}
      <QuarantineNotice />
      <TrashModal open={utilitySurface === "deletedFiles"} onClose={closeUtility} />
      {!wizardOpen && !substitutedSourceGateOpen && missingDirs.length > 0 ? (
        <MissingSourcesNotice
          missing={missingDirs}
          onRecheck={() => void useWizardStore.getState().recheckPresence()}
          onReconfigure={reopenSetup}
        />
      ) : null}
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
              <MenuItem
                disabled={sourceCheck.running}
                onSelect={() => void startSourceCheck()}
              >
                {sourceCheck.running ? "Checking source folders…" : "Check source folders"}
              </MenuItem>
              <MenuItem onSelect={() => openUtility("backgroundWork")}>Background work…</MenuItem>
              <MenuSeparator />
              <MenuItem onSelect={() => openUtility("deletedFiles")}>Deleted files…</MenuItem>
              <MenuSeparator />
              <MenuItem onSelect={openSettings}>Settings…</MenuItem>
              <MenuItem onSelect={() => openUtility("managedTools")}>Managed tools…</MenuItem>
              <MenuItem
                onSelect={() =>
                  reopenSetup()
                }
              >
                Re-run setup wizard…
              </MenuItem>
              <MenuSeparator />
              <MenuItem onSelect={() => openUtility("issues")}>Issues…</MenuItem>
              <MenuItem onSelect={() => openUtility("activityTrace")}>Activity trace…</MenuItem>
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
                  void invoke("reveal_data_subdir", { name: "logs" }).catch((error) => {
                    log.warn("reveal logs failed", toErrorFields(error));
                    reportActionFailure("reveal-logs-failed", "Couldn’t reveal the logs folder.", error);
                  });
                }}
              >
                Reveal logs folder
              </MenuItem>
              <MenuSeparator />
              <MenuItem onSelect={openHelp}>Keyboard shortcuts…</MenuItem>
              <MenuItem onSelect={() => openUtility("about")}>About OneCopy…</MenuItem>
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
          {selected !== null ? (
            <Grid
              items={items}
              loading={itemsLoading}
              loadError={itemsLoadError}
              layout={selected.kind === "other" ? "list" : "tiles"}
            />
          ) : allEmpty ? (
            // While library work is active, empty counts do not yet prove the
            // configured sources have nothing to handle.
            <p className="m-auto text-ink-muted">
              {scanning ? "Updating library…" : "Nothing to handle"}
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
                surface="preview-split"
                hash={previewCurrent?.hash ?? null}
                detail={previewCurrent?.detail ?? null}
                pathId={previewCurrent?.pathId ?? null}
              />
              <button
                aria-label="Close preview"
                title="Close preview"
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
              <MetadataPane detail={detail} hash={selectedHash} item={selectedSectionItem} />
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
          title={status.title ?? status.text}
          role={status === feedback ? status.tone === "danger" ? "alert" : "status" : undefined}
        >
          {status.text}
        </span>
        <span className="flex shrink-0 items-center gap-3">
          <button
            className={soundEnabled ? "text-ink" : "text-ink-muted"}
            aria-pressed={soundEnabled}
            title="Toggle sound for every OneCopy player"
            onClick={() => {
              void setSoundEnabled(!soundEnabled).catch((error) => {
                log.error("sound setting failed", toErrorFields(error));
                reportActionFailure("sound-setting-failed", "Couldn’t change Sound.", error);
              });
            }}
          >
            Sound {soundEnabled ? "on" : "off"}
          </button>
          <button
            className={videoAutoplay ? "text-ink" : "text-ink-muted"}
            aria-pressed={videoAutoplay}
            title="Toggle automatic video playback"
            onClick={() => {
              void setMediumAutoplay("video", !videoAutoplay).catch((error) => {
                log.error("video autoplay setting failed", toErrorFields(error));
                reportActionFailure("video-autoplay-setting-failed", "Couldn’t change video autoplay.", error);
              });
            }}
          >
            Video autoplay {videoAutoplay ? "on" : "off"}
          </button>
          <button
            className={audioAutoplay ? "text-ink" : "text-ink-muted"}
            aria-pressed={audioAutoplay}
            title="Toggle automatic audio playback"
            onClick={() => {
              void setMediumAutoplay("audio", !audioAutoplay).catch((error) => {
                log.error("audio autoplay setting failed", toErrorFields(error));
                reportActionFailure("audio-autoplay-setting-failed", "Couldn’t change audio autoplay.", error);
              });
            }}
          >
            Audio autoplay {audioAutoplay ? "on" : "off"}
          </button>
          {mutationProgress === null && mutationResult !== null && !exitQuiescing ? (
            <MutationResultActions
              result={mutationResult}
              onRevealTrash={() => openUtility("deletedFiles")}
              onDismiss={dismissMutationResult}
            />
          ) : null}
          {mutationProgress !== null && !exitQuiescing ? (
            <button
              className="text-ink-muted hover:text-ink hover:underline disabled:no-underline"
              disabled={mutationCancelling}
              title="Cancel safely after the current file"
              onClick={() => void cancelMutation()}
            >
              {mutationCancelling ? "Cancelling…" : "Cancel file operation"}
            </button>
          ) : null}
          {sourceCheck.running ? (
            <button
              className="text-ink-muted hover:text-ink hover:underline disabled:no-underline"
              disabled={sourceCheck.stopping}
              title="Stop safely after the current cancellable read, file, or durable step"
              onClick={() => void stopSourceCheck()}
            >
              {sourceCheck.stopping ? "Stopping…" : "Stop checking source folders"}
            </button>
          ) : null}
          {/* Why the fans spin while work runs in the background. */}
          <button
            className="text-ink-muted hover:text-ink hover:underline"
            title="Open Background work"
            onClick={() => {
              openUtility("backgroundWork");
              void useDerivedWorkStore.getState().load();
            }}
          >
            {derivedWorkLine}
          </button>
          {/* Active conditions keep a durable status-bar count even after
              their nonblocking notification has been dismissed. */}
          {issuesTotal > 0 ? (
            <button
              className="text-danger hover:underline"
              title="Open the issues list"
              onClick={() => openUtility("issues")}
            >
              {issuesTotal} issue{issuesTotal === 1 ? "" : "s"}
            </button>
          ) : null}
          {/* The managed-tools chip (toolsChip owns the words and the
              loudness — see its rules); clicking always opens the modal. */}
          {(() => {
            const chip = toolsChip(
              ffmpegProgress !== undefined,
              ffmpegProgress === undefined
                ? ""
                : managedInstallActivityLine(ffmpegProgress),
              binariesEntries,
            );
            return chip !== null ? (
              <button
                className={
                  chip.role === "warning"
                    ? "text-warning hover:underline"
                    : "text-ink-muted hover:text-ink"
                }
                title="Managed tools"
                onClick={() => openUtility("managedTools")}
              >
                {chip.text}
              </button>
            ) : null;
          })()}
        </span>
      </footer>
    </div>
    </DestinationDragProvider>
  );
}
