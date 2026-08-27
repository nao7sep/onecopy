// Application-edge reactions to scan, watcher, and derived-output events.
// Event adapters may coordinate stores; stores never reach sideways into one
// another. Installation is idempotent because the main shell may remount in
// development while the native window remains alive.

import { listen } from "@tauri-apps/api/event";
import type { SectionItem } from "../models/items";
import type { ScanProgress } from "../models/scan";
import { log, toErrorFields } from "../repositories";
import { useIssuesStore } from "../state/issues-store";
import { useItemsStore } from "../state/items-store";
import { useSectionsStore } from "../state/sections-store";

let installation: Promise<void> | null = null;
let lastScanPhase: string | null = null;
let lastScanFailures = 0;
let scanCountsRefresh: ReturnType<typeof setTimeout> | null = null;
let scanIssuesRefresh: ReturnType<typeof setTimeout> | null = null;
let derivedIssuesRefresh: ReturnType<typeof setTimeout> | null = null;

function refreshScanCountsSoon(): void {
  if (scanCountsRefresh !== null) return;
  scanCountsRefresh = setTimeout(() => {
    scanCountsRefresh = null;
    void useSectionsStore.getState().loadCounts();
  }, 250);
}

function refreshScanIssuesSoon(): void {
  if (scanIssuesRefresh !== null) return;
  scanIssuesRefresh = setTimeout(() => {
    scanIssuesRefresh = null;
    void useIssuesStore.getState().load();
  }, 500);
}

function clearScanRefreshes(): void {
  if (scanCountsRefresh !== null) clearTimeout(scanCountsRefresh);
  if (scanIssuesRefresh !== null) clearTimeout(scanIssuesRefresh);
  scanCountsRefresh = null;
  scanIssuesRefresh = null;
}

function refreshDerivedIssues(): void {
  if (derivedIssuesRefresh !== null) return;
  derivedIssuesRefresh = setTimeout(() => {
    derivedIssuesRefresh = null;
    void useIssuesStore.getState().load();
  }, 500);
}

async function install(): Promise<void> {
  try {
    await listen<ScanProgress>("scan://progress", (event) => {
      const progress = event.payload;
      const phase = progress.phase;
      if (phase !== lastScanPhase) {
        lastScanPhase = phase;
        lastScanFailures = 0;
        void useSectionsStore.getState().loadCounts();
      } else if (phase === "resolve") {
        // Each resolved row is already durably checkpointed. Refresh at a
        // bounded cadence so Undated drains during the phase instead of only
        // after its slowest file or the next phase transition.
        refreshScanCountsSoon();
      }
      if (progress.failures > lastScanFailures) {
        lastScanFailures = progress.failures;
        refreshScanIssuesSoon();
      }
      useSectionsStore.setState({
        scanning: true,
        progress,
      });
    });
    await listen<{ cancelled?: boolean }>("scan://done", (event) => {
      // A cancelled walk may have left whole directories unread. Keep the
      // repair signal visible even though the worker reached its done event.
      const cancelled = event.payload?.cancelled === true;
      clearScanRefreshes();
      lastScanPhase = null;
      lastScanFailures = 0;
      const currentProgress = useSectionsStore.getState().progress;
      useSectionsStore.setState({
        scanning: false,
        stopping: false,
        progress:
          !cancelled && currentProgress?.phase === "indexed" ? currentProgress : null,
        rescanNeeded: cancelled || useSectionsStore.getState().rescanNeeded,
      });
      void useSectionsStore.getState().loadCounts();
      void useItemsStore.getState().refresh();
      void useIssuesStore.getState().load();
    });
    await listen<{ message: string }>("scan://error", (event) => {
      clearScanRefreshes();
      lastScanPhase = null;
      lastScanFailures = 0;
      useSectionsStore.setState({ scanning: false, stopping: false, progress: null });
      log.error("scan failed", { error: { message: event.payload.message } });
    });
    await listen("watch://updated", () => {
      void useSectionsStore.getState().loadCounts();
      void useItemsStore.getState().refresh();
      void useIssuesStore.getState().load();
    });
    await listen<{ previousHash: string; item: SectionItem }>(
      "derived://item",
      (event) => {
        useItemsStore
          .getState()
          .applyDerivedItem(event.payload.previousHash, event.payload.item);
      },
    );
    await listen("derived://issues", refreshDerivedIssues);
    // Similarity is a cohort rebuild, so refresh once when the cohort settles.
    await listen("derived://similarity-updated", () => {
      void useItemsStore.getState().refresh();
    });
    await listen("watch://rescan-needed", () => {
      useSectionsStore.setState({ rescanNeeded: true });
    });
  } catch (error) {
    log.warn("scan event wiring failed", toErrorFields(error));
  }
}

export function installScanEventWiring(): Promise<void> {
  installation ??= install();
  return installation;
}
