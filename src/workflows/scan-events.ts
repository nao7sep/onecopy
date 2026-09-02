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
import { reconcileComparisonMembership } from "./comparison";

let installation: Promise<void> | null = null;
let refreshTimer: ReturnType<typeof setTimeout> | null = null;
let derivedIssuesTimer: ReturnType<typeof setTimeout> | null = null;

interface SequencedProgress {
  eventSequence: number;
  progress: ScanProgress;
}

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
    await listen<Omit<SourceCheckState, "progress">>(
      "source-check://state",
      (event) => {
        useSectionsStore.setState((state) =>
          event.payload.eventSequence <= state.sourceCheck.eventSequence
            ? state
            : {
                sourceCheck: {
                  ...event.payload,
                  progress: state.sourceCheck.progress,
                },
              },
        );
      },
    );
    await listen<SequencedProgress>("source-check://progress", (event) => {
      let accepted = false;
      useSectionsStore.setState((state) => {
        if (event.payload.eventSequence <= state.sourceCheck.eventSequence) return state;
        accepted = true;
        return {
          sourceCheck: {
            ...state.sourceCheck,
            running: true,
            eventSequence: event.payload.eventSequence,
            progress: event.payload.progress,
          },
        };
      });
      if (accepted) refreshLibrarySoon();
    });
    await listen<{ eventSequence: number; stopped?: boolean; error?: string }>(
      "source-check://done",
      (event) => {
        let accepted = false;
        useSectionsStore.setState((state) => {
          if (event.payload.eventSequence <= state.sourceCheck.eventSequence) return state;
          accepted = true;
          return {
            sourceCheck: {
              running: false,
              stopping: false,
              lastResult:
                event.payload.error !== undefined
                  ? "failed"
                  : event.payload.stopped === true
                    ? "stopped"
                    : "completed",
              eventSequence: event.payload.eventSequence,
              progress: null,
            },
            rescanNeeded:
              event.payload.stopped === true || event.payload.error !== undefined
                ? true
                : state.rescanNeeded,
          };
        });
        if (!accepted) return;
        if (event.payload.error !== undefined) {
          log.error("source-folder check failed", {
            error: { message: event.payload.error },
          });
        }
        refreshLibraryNow();
        void reconcileComparisonMembership();
      },
    );

    await listen<Omit<FileInformationState, "progress">>(
      "file-information://state",
      (event) => {
        useSectionsStore.setState((state) =>
          event.payload.eventSequence <= state.fileInformation.eventSequence
            ? state
            : {
                fileInformation: {
                  ...event.payload,
                  progress: state.fileInformation.progress,
                },
              },
        );
      },
    );
    await listen<SequencedProgress>("file-information://progress", (event) => {
      let accepted = false;
      useSectionsStore.setState((state) => {
        if (event.payload.eventSequence <= state.fileInformation.eventSequence) return state;
        accepted = true;
        return {
          fileInformation: {
            ...state.fileInformation,
            running: true,
            eventSequence: event.payload.eventSequence,
            progress: event.payload.progress,
          },
        };
      });
      if (accepted) refreshLibrarySoon();
    });
    await listen<{ eventSequence: number; error?: string }>("file-information://done", (event) => {
      let accepted = false;
      useSectionsStore.setState((state) => {
        if (event.payload.eventSequence <= state.fileInformation.eventSequence) return state;
        accepted = true;
        return {
          fileInformation: {
            ...state.fileInformation,
            running: false,
            stopping: false,
            eventSequence: event.payload.eventSequence,
            progress: null,
          },
        };
      });
      if (!accepted) return;
      if (event.payload.error !== undefined) {
        log.error("file-information completion failed", {
          error: { message: event.payload.error },
        });
      }
      refreshLibraryNow();
      void useSectionsStore.getState().loadIndexWork();
    });

    await listen("watch://updated", () => {
      refreshLibrarySoon();
      void reconcileComparisonMembership();
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
    await listen<{ message: string }>("derived://worker-failed", (event) => {
      log.error("previews and analysis worker stopped", {
        error: { message: event.payload.message },
      });
      useItemsStore.setState({
        message: "Previews and analysis stopped. Restart OneCopy, then try again.",
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
    recordInterfaceFailure(
      "Live library updates are unavailable. Restart OneCopy to repair them.",
    );
    useItemsStore.setState({
      message:
        "Live library updates are unavailable. Restart OneCopy to repair them.",
    });
  }
}

export function installScanEventWiring(): Promise<void> {
  installation ??= install();
  return installation;
}
