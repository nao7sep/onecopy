import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
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
  HEADER_HEIGHT,
  PREVIEW_PANE_DEFAULT_WIDTH,
  PREVIEW_PANE_MIN_WIDTH,
  RIGHT_PANE_DEFAULT_WIDTH,
  RIGHT_PANE_MIN_WIDTH,
  SIDEBAR_DEFAULT_WIDTH,
  SIDEBAR_MIN_WIDTH,
  SPLITTER_WIDTH,
  clampPaneWidths,
  computeMinWindowHeight,
  computeMinWindowWidth,
} from "./utils/windowSizing";
import { parseSavedBounds, restorableBounds, shrinkToFit } from "./utils/windowBounds";
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
import { useSettingsStore } from "./state/settings-store";
import { isEditableTarget, isHelpShortcut, isSettingsShortcut, shadowsMacTextEditing } from "./utils/shortcuts";
import { hasOpenModal } from "./utils/modalStack";
import { isComposingEvent } from "./hooks/useComposing";
import { Menu, MenuItem, MenuSeparator } from "./components/Menu";
import AboutModal from "./components/AboutModal";
import ScenesModal from "./components/ScenesModal";
import TrashModal from "./components/TrashModal";
import ConfirmDialog from "./components/ConfirmDialog";
import { Menu as MenuIcon, X } from "lucide-react";
import { useWizardStore } from "./state/wizard-store";
import { useComparisonStore } from "./state/comparison-store";
import { useIssuesStore } from "./state/issues-store";
import {
  toolsChip,
  useBinariesStore,
} from "./state/binaries-store";
import { itemKey } from "./state/items-store";
import { DEFAULT_DESC, type SortChoice, type SortOrder } from "./models/items";
import { handleSpaceLook, usePreviewStore } from "./state/preview-store";
import { installActivityPings, useBackfillStore } from "./state/backfill-store";
import PreviewSurface from "./components/PreviewSurface";
import { log, reportWindowCall, toErrorFields } from "./repositories";

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
  const issuesTotal = useIssuesStore((s) => s.total);
  const backfillLine = useBackfillStore((s) => s.line);
  const setIssuesOpen = useIssuesStore((s) => s.setOpen);
  const binariesEntries = useBinariesStore((s) => s.entries);
  // The chip narrates ffmpeg's own install only; a model download in flight
  // is the modal's story.
  const ffmpegProgress = useBinariesStore((s) => s.installing["ffmpeg"]);
  const setBinariesModalOpen = useBinariesStore((s) => s.setModalOpen);
  const [helpOpen, setHelpOpen] = useState(false);
  const [aboutOpen, setAboutOpen] = useState(false);
  const [trashOpen, setTrashOpen] = useState(false);
  /** Enter on an anchor video opens the scenes modal for this hash. */
  const [scenesFor, setScenesFor] = useState<string | null>(null);
  /** Pending permanent deletion awaiting confirmation (item count shown). */
  const [confirmPermanent, setConfirmPermanent] = useState<number | null>(null);
  /** Pending TRASH deletion awaiting confirmation — exists only when the
   * opt-in `confirmTrashDelete` config flag is on. */
  const [confirmTrash, setConfirmTrash] = useState<number | null>(null);
  const previewFollow = usePreviewStore((s) => s.follow);
  const previewPlacement = usePreviewStore((s) => s.placement);
  const previewCurrent = usePreviewStore((s) => s.current);
  const splitOpen = previewFollow && previewPlacement === "split";
  const mainColRef = useRef<HTMLElement | null>(null);

  // The one open-Settings path — the menu item and the Cmd+, chord both use
  // it, so the two can never drift.
  const openSettings = () =>
    useSettingsStore.getState().openWith(useAppStore.getState().appData?.config ?? null);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      // While typing: a bare "?" is text, not the help alias, and on macOS the
      // Ctrl half of a dual-bound chord stands down — the Cmd half is the
      // binding and always fires (keyboard-shortcut-conventions).
      const editable = isEditableTarget(event.target);
      if (editable && (event.key === "?" || shadowsMacTextEditing(event))) return;
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

  // The backfill scheduler's view of the user: throttled input pings.
  useEffect(() => {
    installActivityPings(window);
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

  // App chrome: the derived window minimum, applied at startup and when the
  // split preview changes it; zoom is persisted app-level STATE (not a
  // preference) on the discrete ladder, with cross-layout shortcut detection
  // (JIS ";" included). Failures degrade silently in the browser-dev case
  // where no Tauri window exists.
  //
  // NEVER while maximized: on Windows the min-size call knocks a maximized
  // window back to normal, which is how Space (split placement flips this
  // effect's dependency) un-maximized the developer's always-maximized main
  // window. A maximized window cannot go below any minimum anyway, so the
  // constraint waits in `pendingMinSize` and the resize listener below
  // applies it once the window is normal again.
  const pendingMinSize = useRef<LogicalSize | null>(null);
  useEffect(() => {
    const size = new LogicalSize(computeMinWindowWidth(splitOpen), computeMinWindowHeight());
    const window = getCurrentWindow();
    void (async () => {
      try {
        if (await window.isMaximized()) {
          pendingMinSize.current = size;
          return;
        }
        pendingMinSize.current = null;
        await window.setMinSize(size);
      } catch (error) {
        reportWindowCall("setMinSize")(error);
      }
    })();
  }, [splitOpen]);

  // Window bounds: the window is created HIDDEN (tauri.conf `visible: false` —
  // on Windows a visible window paints a white frame before WebView2 loads),
  // saved bounds are restored while nobody can see the jump, and only then is
  // the window shown. Bounds are state like zoom: physical pixels, saved
  // debounced from the move/resize events, validated against the LIVE monitor
  // set at boot — this machine swings between one and three screens, and a
  // window restored onto a detached monitor has no reachable title bar.
  const bootShown = useRef(false);
  useEffect(() => {
    if (bootShown.current || (appData === null && loadError === null)) return;
    bootShown.current = true;
    const window = getCurrentWindow();
    // Whatever restore does, the window MUST end up visible — a thrown
    // monitor query on some exotic setup may cost the placement, never the app.
    const showFallback = setTimeout(() => {
      void window.show().catch(reportWindowCall("show"));
    }, 3000);
    void (async () => {
      try {
        const { availableMonitors, currentMonitor, PhysicalPosition, PhysicalSize } =
          await import("@tauri-apps/api/window");
        const state = useAppStore.getState().appData?.state;
        const saved = restorableBounds(
          parseSavedBounds(state?.windowBounds),
          await availableMonitors(),
        );
        if (saved !== null) {
          // The NORMAL-state geometry, applied even when maximizing next:
          // it is what un-maximizing returns to.
          await window.setPosition(new PhysicalPosition(saved.x, saved.y));
          await window.setSize(new PhysicalSize(saved.width, saved.height));
        } else {
          // First launch (or stale bounds): the 1400×900 default overflows a
          // small laptop screen; shrink to the hosting monitor.
          const monitor = await currentMonitor();
          const inner = await window.innerSize();
          const fitted = monitor !== null ? shrinkToFit(inner, monitor.size) : null;
          if (fitted !== null) {
            await window.setSize(new PhysicalSize(fitted.width, fitted.height));
          }
        }
        // The app is almost always used maximized (developer, 2026-08-17) —
        // a state worth restoring in its own right, independent of whether
        // the normal-state bounds survived the monitor check.
        if (state?.windowMaximized === true) {
          await window.maximize();
        }
      } catch (error) {
        reportWindowCall("restore bounds")(error);
      } finally {
        clearTimeout(showFallback);
        await window.show().catch(reportWindowCall("show"));
        await window.setFocus().catch(reportWindowCall("boot setFocus"));
      }
    })();
  }, [appData, loadError]);

  // The save half: on move/resize, read the live outer position + inner size
  // (the restore counterparts) and persist, debounced — drags fire dozens of
  // events per second and state.json wants the landing place, not the journey.
  //
  // Maximized is a FLAG, never geometry: while maximized, only
  // `windowMaximized` updates — writing the maximized rect into
  // `windowBounds` would overwrite the remembered normal size, so
  // un-maximizing (now or next session) would have nowhere real to return
  // to. This is also where a deferred min-size constraint lands once the
  // window is normal again (see the min-size effect).
  useEffect(() => {
    const window = getCurrentWindow();
    let timer: ReturnType<typeof setTimeout> | null = null;
    const save = () => {
      if (timer !== null) clearTimeout(timer);
      timer = setTimeout(() => {
        void (async () => {
          try {
            if (await window.isMaximized()) {
              await useAppStore.getState().patchState({ windowMaximized: true });
              return;
            }
            if (pendingMinSize.current !== null) {
              const size = pendingMinSize.current;
              pendingMinSize.current = null;
              await window.setMinSize(size).catch(reportWindowCall("setMinSize"));
            }
            const position = await window.outerPosition();
            const size = await window.innerSize();
            await useAppStore.getState().patchState({
              windowMaximized: false,
              windowBounds: {
                x: position.x,
                y: position.y,
                width: size.width,
                height: size.height,
              },
            });
          } catch (error) {
            reportWindowCall("save bounds")(error);
          }
        })();
      }, 500);
    };
    const unlistens: Array<() => void> = [];
    void window.onMoved(save).then((fn) => unlistens.push(fn));
    void window.onResized(save).then((fn) => unlistens.push(fn));
    return () => {
      if (timer !== null) clearTimeout(timer);
      for (const fn of unlistens) fn();
    };
  }, []);

  // Pane widths: persisted INTENT in pixels (written only on drag-end); the
  // displayed width is the intent clamped against the live container width,
  // so a narrow window never rewrites what the user chose.
  const [paneIntents, setPaneIntents] = useState({
    left: SIDEBAR_DEFAULT_WIDTH,
    right: RIGHT_PANE_DEFAULT_WIDTH,
    preview: PREVIEW_PANE_DEFAULT_WIDTH,
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
    splitOpen ? paneIntents.preview : null,
  );

  const beginPaneDrag =
    (side: "left" | "right" | "preview") => (event: React.MouseEvent) => {
      event.preventDefault();
      const startX = event.clientX;
      const start = { ...paneIntentsRef.current };
      // Window-wide resize cursor for the drag's duration — the pointer roams
      // over elements carrying their own cursors otherwise (cursor conventions).
      document.body.classList.add("col-resizing");
      const onMove = (e: MouseEvent) => {
        const delta = e.clientX - startX;
        // The sidebar grows rightward; the preview and right pane grow
        // leftward (their dividers sit on their left edges).
        const next =
          side === "left"
            ? { ...paneIntentsRef.current, left: Math.max(SIDEBAR_MIN_WIDTH, start.left + delta) }
            : side === "preview"
              ? {
                  ...paneIntentsRef.current,
                  preview: Math.max(PREVIEW_PANE_MIN_WIDTH, start.preview - delta),
                }
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
          previewPaneWidth: paneIntentsRef.current.preview,
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
    void getCurrentWebview().setZoom(next).catch(reportWindowCall("setZoom"));
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
      void getCurrentWebview().setZoom(level).catch(reportWindowCall("setZoom"));
    }
  }, [appData]);

  useEffect(() => {
    const onZoomKey = (event: KeyboardEvent) => {
      // Mid-composition the chord belongs to the pending IME candidate;
      // matters on macOS where Ctrl+; is an IME conversion chord.
      if (isComposingEvent(event)) return;
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
      if (isEditableTarget(event.target)) return;
      if (event.key === "Delete" || event.key === "Backspace") {
        event.preventDefault();
        const { selectedKeys, selectedItem } = useItemsStore.getState();
        const count = selectedKeys.size > 0 ? selectedKeys.size : selectedItem !== null ? 1 : 0;
        if (count === 0) return;
        if (event.shiftKey) {
          // Permanent deletion always confirms with the count (the design's
          // rule, NOT configurable — this one bypasses the net).
          setConfirmPermanent(count);
        } else if (
          useAppStore.getState().appData?.config?.confirmTrashDelete === true
        ) {
          // Opt-in confirmation for the ordinary trash delete (developer,
          // 2026-08-17). OFF by default: the trash is the net, and a dialog
          // on every Delete would break the keystroke-paced cull — but a
          // deliberate user can trade that pace for the extra stop.
          setConfirmTrash(count);
        } else {
          void useItemsStore.getState().deleteSelected(false);
        }
      } else if (
        (event.key === " " || event.key.toLowerCase() === "p") &&
        !event.metaKey &&
        !event.ctrlKey &&
        !event.altKey
      ) {
        // Space = LOOK, with P as its alias; the chrome control is the third
        // entry — all through the store's one rule so they cannot drift. A
        // video loaded in the preview owns Space (play/pause), which is why
        // the rule may decline to claim the event.
        handleSpaceLook(event);
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
          // Similar photos exist: Enter means "show them all at once". A
          // group with no other live members says so instead of surprising
          // the user with a different surface.
          void useComparisonStore
            .getState()
            .openGroup(item.hash)
            .then((opened) => {
              if (opened) return;
              useItemsStore.setState({
                message: "No similar photos left in this group",
              });
            });
        } else {
          // No similars: Enter says so and does nothing else (developer,
          // 2026-08-17). It used to open the 100% view — but Enter is the
          // trained "open the similar set" reflex, pressed without checking
          // the ≈ badge first, and a surprise mode-switch punishes exactly
          // that habit. Inspection stays where it belongs: Space to peek,
          // Z or a click for 100%.
          useItemsStore.setState({ message: "No similar photos for this image" });
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
    // Per-lane sort choices; legacy shapes (a bare string, or a lane holding
    // a bare order string) restore into their natural direction.
    const isOrder = (v: unknown): v is SortOrder =>
      v === "time" || v === "name" || v === "size" || v === "resolution" || v === "ext";
    const asChoice = (v: unknown): SortChoice | null => {
      if (isOrder(v)) return { order: v, desc: DEFAULT_DESC[v] };
      if (typeof v === "object" && v !== null) {
        const rec = v as Record<string, unknown>;
        if (isOrder(rec.order)) {
          return { order: rec.order, desc: rec.desc === true };
        }
      }
      return null;
    };
    const savedOrders = (state.sortOrders ?? {}) as Record<string, unknown>;
    useItemsStore.setState((current) => ({
      sortOrders: {
        media:
          asChoice(savedOrders.media) ?? asChoice(state.sortOrder) ?? current.sortOrders.media,
        other: asChoice(savedOrders.other) ?? current.sortOrders.other,
      },
    }));
    if (state.rightPaneTab === "destinations") setRightTabRaw("destinations");
    const left = state.sidebarWidth;
    const right = state.rightPaneWidth;
    const preview = state.previewPaneWidth;
    setPaneIntents((current) => ({
      left: typeof left === "number" && Number.isFinite(left) ? left : current.left,
      right: typeof right === "number" && Number.isFinite(right) ? right : current.right,
      preview:
        typeof preview === "number" && Number.isFinite(preview) ? preview : current.preview,
    }));
    void import("./state/preview-store").then(({ usePreviewStore }) => {
      const placement = state.previewPlacement;
      usePreviewStore
        .getState()
        .restoreFollow(
          state.previewFollow === true,
          placement === "split" || placement === "window" ? placement : null,
        );
    });
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
      {confirmTrash !== null ? (
        <ConfirmDialog
          title="Move to trash?"
          message={`Move ${confirmTrash} item${
            confirmTrash === 1 ? "" : "s"
          } and every copy to the trash? Recoverable from Menu → Trash.`}
          confirmLabel="Move to trash"
          onConfirm={() => {
            setConfirmTrash(null);
            void useItemsStore.getState().deleteSelected(false);
          }}
          onCancel={() => setConfirmTrash(null)}
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
              <MenuItem onSelect={() => setHelpOpen(true)}>Keyboard shortcuts…</MenuItem>
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
          ref={mainColRef}
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
                zoom={previewCurrent?.zoom === true}
              />
              <button
                aria-label="Close preview"
                title="Close preview (Space)"
                className="absolute right-2 top-2 flex h-6 w-6 items-center justify-center rounded-md bg-surface/80 text-ink-muted transition-colors hover:bg-surface-muted hover:text-ink"
                onClick={() => usePreviewStore.getState().close()}
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
          {/* The idle backfill's narration — why the fans spin while the
              user is away; disappears the moment there is nothing to say. */}
          {backfillLine !== null ? (
            <span className="text-ink-muted">{backfillLine}</span>
          ) : null}
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
