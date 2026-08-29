// Application-edge reactions to source checking, file-information completion,
// watcher updates, and derived-output events.

import { listen } from "@tauri-apps/api/event";
import type { SectionItem } from "../models/items";
import type { ScanProgress } from "../models/scan";
import { log, toErrorFields } from "../repositories";
import { recordInterfaceFailure } from "../utils/failureSurface";
import { useIssuesStore } from "../state/issues-store";
import { useItemsStore } from "../state/items-store";
import {
  type FileInformationState,
  type SourceCheckState,
  useSectionsStore,
} from "../state/sections-store";

let installation: Promise<void> | null = null;
let refreshTimer: ReturnType<typeof setTimeout> | null = null;
let derivedIssuesTimer: ReturnType<typeof setTimeout> | null = null;

function refreshLibrarySoon(): void {
  if (refreshTimer !== null) return;
  refreshTimer = setTimeout(() => {
    refreshTimer = null;
    void useSectionsStore.getState().loadCounts();
    void useItemsStore.getState().refresh();
    void useIssuesStore.getState().load();
  }, 250);
}

function refreshLibraryNow(): void {
  void useSectionsStore.getState().loadCounts();
  void useItemsStore.getState().refresh();
  void useIssuesStore.getState().load();
}

function refreshDerivedIssues(): void {
  if (derivedIssuesTimer !== null) return;
  derivedIssuesTimer = setTimeout(() => {
    derivedIssuesTimer = null;
    void useIssuesStore.getState().load();
  }, 500);
}

async function install(): Promise<void> {
  try {
    await listen<Omit<SourceCheckState, "progress">>("source-check://state", (event) => {
      useSectionsStore.setState((state) => ({
        sourceCheck: { ...event.payload, progress: state.sourceCheck.progress },
      }));
    });
    await listen<ScanProgress>("source-check://progress", (event) => {
      useSectionsStore.setState((state) => ({
        sourceCheck: {
          ...state.sourceCheck,
          running: true,
          progress: event.payload,
        },
      }));
      refreshLibrarySoon();
    });
    await listen<{ stopped?: boolean; error?: string }>("source-check://done", (event) => {
      useSectionsStore.setState((state) => ({
        sourceCheck: { running: false, stopping: false, progress: null },
        rescanNeeded:
          event.payload.stopped === true || event.payload.error !== undefined
            ? true
            : state.rescanNeeded,
      }));
      if (event.payload.error !== undefined) {
        log.error("source-folder check failed", {
          error: { message: event.payload.error },
        });
      }
      refreshLibraryNow();
    });

    await listen<Omit<FileInformationState, "progress">>(
      "file-information://state",
      (event) => {
        useSectionsStore.setState((state) => ({
          fileInformation: {
            ...event.payload,
            progress: state.fileInformation.progress,
          },
        }));
      },
    );
    await listen<ScanProgress>("file-information://progress", (event) => {
      useSectionsStore.setState((state) => ({
        fileInformation: {
          ...state.fileInformation,
          running: true,
          progress: event.payload,
        },
      }));
      refreshLibrarySoon();
    });
    await listen<{ error?: string }>("file-information://done", (event) => {
      useSectionsStore.setState((state) => ({
        fileInformation: {
          ...state.fileInformation,
          running: false,
          stopping: false,
          progress: null,
        },
      }));
      if (event.payload.error !== undefined) {
        log.error("file-information completion failed", {
          error: { message: event.payload.error },
        });
      }
      refreshLibraryNow();
      void useSectionsStore.getState().loadIndexWork();
    });

    await listen("watch://updated", refreshLibrarySoon);
    await listen<{ previousHash: string; item: SectionItem }>(
      "derived://item",
      (event) => {
        useItemsStore
          .getState()
          .applyDerivedItem(event.payload.previousHash, event.payload.item);
      },
    );
    await listen("derived://issues", refreshDerivedIssues);
    await listen<{ message: string }>("derived://worker-failed", (event) => {
      useItemsStore.setState({
        message: `Previews and analysis stopped: ${event.payload.message}`,
      });
      void useIssuesStore.getState().load();
    });
    await listen("derived://similarity-updated", () => {
      void useItemsStore.getState().refresh();
    });
    await listen("watch://rescan-needed", () => {
      useSectionsStore.setState({ rescanNeeded: true });
    });
    await listen<{ reason: string }>("watch://failed", (event) => {
      useSectionsStore.setState({ rescanNeeded: true });
      log.error("filesystem watcher failed", {
        error: { message: event.payload.reason },
      });
      void useIssuesStore.getState().load();
    });
    await listen("failure://reported", () => {
      void useIssuesStore.getState().load();
    });
    await listen<{ message: string }>("failure://direct", (event) => {
      useItemsStore.setState({ message: event.payload.message });
    });
    await useSectionsStore.getState().loadIndexWork();
  } catch (error) {
    log.warn("library event wiring failed", toErrorFields(error));
    const message = error instanceof Error ? error.message : String(error);
    recordInterfaceFailure(message);
    useItemsStore.setState({
      message: "Live library updates are unavailable. Restart OneCopy to repair them.",
    });
  }
}

export function installScanEventWiring(): Promise<void> {
  installation ??= install();
  return installation;
}
