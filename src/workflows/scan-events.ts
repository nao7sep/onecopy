// Application-edge reactions to scan, watcher, and derived-output events.
// Event adapters may coordinate stores; stores never reach sideways into one
// another. Installation is idempotent because the main shell may remount in
// development while the native window remains alive.

import { listen } from "@tauri-apps/api/event";
import type { SectionItem } from "../models/items";
import { progressLine } from "../models/scan";
import { log, toErrorFields } from "../repositories";
import { useIssuesStore } from "../state/issues-store";
import { useItemsStore } from "../state/items-store";
import { useSectionsStore } from "../state/sections-store";

let installation: Promise<void> | null = null;
let lastScanPhase: string | null = null;
let derivedIssuesRefresh: ReturnType<typeof setTimeout> | null = null;

function refreshDerivedIssues(): void {
  if (derivedIssuesRefresh !== null) return;
  derivedIssuesRefresh = setTimeout(() => {
    derivedIssuesRefresh = null;
    void useIssuesStore.getState().load();
  }, 500);
}

async function install(): Promise<void> {
  try {
    await listen<{ phase: string; detail: string }>("scan://progress", (event) => {
      const phase = event.payload.phase;
      if (phase !== lastScanPhase) {
        lastScanPhase = phase;
        void useSectionsStore.getState().loadCounts();
      }
      useSectionsStore.setState({
        scanning: true,
        progress: progressLine(phase, event.payload.detail),
      });
    });
    await listen<{ cancelled?: boolean }>("scan://done", (event) => {
      // A cancelled walk may have left whole directories unread. Keep the
      // repair signal visible even though the worker reached its done event.
      const cancelled = event.payload?.cancelled === true;
      lastScanPhase = null;
      useSectionsStore.setState({
        scanning: false,
        progress: "",
        rescanNeeded: cancelled || useSectionsStore.getState().rescanNeeded,
      });
      void useSectionsStore.getState().loadCounts();
      void useItemsStore.getState().refresh();
      void useIssuesStore.getState().load();
    });
    await listen<{ message: string }>("scan://error", (event) => {
      useSectionsStore.setState({ scanning: false, progress: "" });
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
