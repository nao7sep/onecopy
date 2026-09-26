// Application-edge reactions to source checking, file-information completion,
// watcher updates, and derived-output events.

import type { SectionItem } from "../models/items";
import type { ScanProgress } from "../models/scan";
import { log, toErrorFields } from "../repositories";
import { message } from "../i18n/translate";
import {
  presentEscapedDetail,
  presentEscapedFailure,
  recordInterfaceFailure,
} from "../utils/failureSurface";
import { createEventInstaller } from "../utils/eventInstallation";
import { useIssuesStore } from "../state/issues-store";
import { useItemsStore } from "../state/items-store";
import {
  type FileInformationState,
  type SourceCheckState,
  useSectionsStore,
} from "../state/sections-store";
import { reconcileComparisonMembership } from "./comparison";
import {
  finishActivityOperation,
  latestActivityOperationId,
  recordActivity,
} from "../repositories/activity";
import { transitionCoalescer, type CoalescerState } from "../models/refresh-coalescer";

let refreshTimer: ReturnType<typeof setTimeout> | null = null;
let derivedIssuesTimer: ReturnType<typeof setTimeout> | null = null;

function recordStaleWork(
  owner: "sourceCheck" | "fileInformation",
  generation: number,
): void {
  recordActivity({
    kind: "stale",
    owner,
    operationId: latestActivityOperationId(owner) ?? `${owner}:auto`,
    generation,
    current: "stale",
    reason: "staleResponse",
  });
}

interface SequencedProgress {
  eventSequence: number;
  progress: ScanProgress;
}

// Single-flight counts+refresh, driven by the pure coalescer model. Scan-time
// progress can fire 8 times a second; without single-flight gating, a round
// slower than the event rate queued another round on top of it without bound
// (C-H1). At most one round runs at a time, and at most one more is queued to
// run immediately after it finishes.
//
// A round is either "light" (counts plus the currently displayed window only,
// keyed by the anchor Main already resolved) or "full" (counts plus a whole
// `reconcile_section` selection/anchor round-trip). A tick that cannot remove
// section members -- source-check/file-information progress, similarity
// relabeling -- only ever needs the light round: round-tripping a 100k+
// selection through `reconcile_section` on every such tick serialized ~200k
// identities each way for no reason (C-M2). Watcher updates and completion
// events can remove members, so they request (and, once requested, keep) a
// full round; if a full round is requested while a light one is pending or
// running, the eventual round escalates to full rather than downgrading.
let libraryRefreshState: CoalescerState = "idle";
let libraryRefreshFull = false;

function runLibraryRefreshRound(): void {
  const full = libraryRefreshFull;
  libraryRefreshFull = false;
  void Promise.allSettled([
    useSectionsStore.getState().loadCounts(),
    full ? useItemsStore.getState().refresh() : useItemsStore.getState().refreshWindowOnly(),
    useIssuesStore.getState().load(),
  ]).then(() => {
    driveLibraryRefresh({ kind: "roundCompleted" });
  });
}

function driveLibraryRefresh(event: Parameters<typeof transitionCoalescer>[1]): void {
  const { state, actions } = transitionCoalescer(libraryRefreshState, event);
  libraryRefreshState = state;
  for (const action of actions) {
    switch (action) {
      case "startTimer":
        if (refreshTimer !== null) clearTimeout(refreshTimer);
        refreshTimer = setTimeout(() => {
          refreshTimer = null;
          driveLibraryRefresh({ kind: "timerFired" });
        }, 250);
        break;
      case "clearTimer":
        if (refreshTimer !== null) {
          clearTimeout(refreshTimer);
          refreshTimer = null;
        }
        break;
      case "startRound":
        runLibraryRefreshRound();
        break;
    }
  }
}

/** Debounced, single-flight, window-only refresh for high-frequency progress
 * signals that cannot remove section members (source-check/file-information
 * progress, similarity relabeling): coalesces bursts and never starts a round
 * on top of one already in flight. */
function refreshLibrarySoon(): void {
  driveLibraryRefresh({ kind: "trigger" });
}

/** Debounced, single-flight, full refresh for signals that can remove section
 * members but still arrive as a burst (watcher updates): still coalesced, but
 * the eventual round reconciles the whole selection rather than only the
 * window. */
function refreshLibrarySoonFull(): void {
  libraryRefreshFull = true;
  driveLibraryRefresh({ kind: "trigger" });
}

/** Immediate, full refresh for low-frequency, must-be-accurate completions (an
 * operation finished, a full source check ended). Still single-flight: if a
 * round is already running, this queues exactly one trailing rerun rather
 * than starting a second round concurrently. */
function refreshLibraryNow(): void {
  libraryRefreshFull = true;
  driveLibraryRefresh({ kind: "triggerImmediate" });
}

function refreshDerivedIssues(): void {
  if (derivedIssuesTimer !== null) return;
  derivedIssuesTimer = setTimeout(() => {
    derivedIssuesTimer = null;
    void useIssuesStore.getState().load();
  }, 500);
}

const install = createEventInstaller(
  async (listeners) => {
    await listeners.listen<Omit<SourceCheckState, "progress">>(
      "source-check://state",
      (event) => {
        let accepted = false;
        useSectionsStore.setState((state) => {
          if (event.payload.eventSequence <= state.sourceCheck.eventSequence) return state;
          accepted = true;
          return {
            sourceCheck: {
              ...event.payload,
              progress: state.sourceCheck.progress,
            },
          };
        });
        if (!accepted) recordStaleWork("sourceCheck", event.payload.eventSequence);
      },
    );
    await listeners.listen<SequencedProgress>("source-check://progress", (event) => {
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
      if (!accepted) {
        recordStaleWork("sourceCheck", event.payload.eventSequence);
      } else {
        recordActivity({
          kind: "progressed",
          owner: "sourceCheck",
          operationId: latestActivityOperationId("sourceCheck") ?? "sourceCheck:auto",
          current: "running",
          done: event.payload.progress.done,
          total: event.payload.progress.total,
        });
        refreshLibrarySoon();
      }
    });
    await listeners.listen<{ eventSequence: number; sourceCheck: Omit<SourceCheckState, "progress">; stopped?: boolean; error?: string }>(
      "source-check://done",
      (event) => {
        let accepted = false;
        useSectionsStore.setState((state) => {
          if (event.payload.eventSequence <= state.sourceCheck.eventSequence) return state;
          accepted = true;
          return {
            sourceCheck: {
              ...event.payload.sourceCheck,
              eventSequence: event.payload.eventSequence,
              progress: null,
            },
            rescanNeeded:
              event.payload.stopped === true || event.payload.error !== undefined
                ? true
                : state.rescanNeeded,
          };
        });
        if (!accepted) {
          recordStaleWork("sourceCheck", event.payload.eventSequence);
          return;
        }
        const operationId =
          latestActivityOperationId("sourceCheck") ?? "sourceCheck:auto";
        recordActivity({
          kind:
            event.payload.error !== undefined
              ? "failed"
              : event.payload.stopped === true
                ? "cancelled"
                : "completed",
          owner: "sourceCheck",
          operationId,
          previous: "running",
          current:
            event.payload.error !== undefined
              ? "failed"
              : event.payload.stopped === true
                ? "cancelled"
                : "succeeded",
          reason:
            event.payload.error !== undefined
              ? "error"
              : event.payload.stopped === true
                ? "user"
                : "completion",
        });
        finishActivityOperation("sourceCheck", operationId);
        if (event.payload.error !== undefined) {
          log.error("source-folder check failed", {
            error: { message: event.payload.error },
          });
        }
        refreshLibraryNow();
        void reconcileComparisonMembership();
      },
    );

    await listeners.listen<Omit<FileInformationState, "progress">>(
      "file-information://state",
      (event) => {
        let accepted = false;
        useSectionsStore.setState((state) => {
          if (event.payload.eventSequence <= state.fileInformation.eventSequence) return state;
          accepted = true;
          return {
            fileInformation: {
              ...event.payload,
              progress: state.fileInformation.progress,
            },
          };
        });
        if (!accepted) recordStaleWork("fileInformation", event.payload.eventSequence);
      },
    );
    await listeners.listen<SequencedProgress>("file-information://progress", (event) => {
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
      if (!accepted) {
        recordStaleWork("fileInformation", event.payload.eventSequence);
      } else {
        recordActivity({
          kind: "progressed",
          owner: "fileInformation",
          operationId:
            latestActivityOperationId("fileInformation") ?? "fileInformation:auto",
          current: "running",
          done: event.payload.progress.done,
          total: event.payload.progress.total,
        });
        refreshLibrarySoon();
      }
    });
    await listeners.listen<{ eventSequence: number; error?: string }>("file-information://done", (event) => {
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
      if (!accepted) {
        recordStaleWork("fileInformation", event.payload.eventSequence);
        return;
      }
      const operationId =
        latestActivityOperationId("fileInformation") ?? "fileInformation:auto";
      recordActivity({
        kind: event.payload.error === undefined ? "completed" : "failed",
        owner: "fileInformation",
        operationId,
        previous: "running",
        current: event.payload.error === undefined ? "succeeded" : "failed",
        reason: event.payload.error === undefined ? "completion" : "error",
      });
      finishActivityOperation("fileInformation", operationId);
      if (event.payload.error !== undefined) {
        log.error("file-information completion failed", {
          error: { message: event.payload.error },
        });
      }
      refreshLibraryNow();
      void useSectionsStore.getState().loadIndexWork();
    });

    await listeners.listen("watch://updated", () => {
      // The watcher can remove section members (an external delete/move), so
      // this needs the full reconcile, not the window-only refresh.
      refreshLibrarySoonFull();
      void reconcileComparisonMembership();
    });
    await listeners.listen<{ previousHash: string; item: SectionItem }>(
      "derived://item",
      (event) => {
        useItemsStore
          .getState()
          .applyDerivedItem(event.payload.previousHash, event.payload.item);
      },
    );
    await listeners.listen("derived://issues", refreshDerivedIssues);
    await listeners.listen<{ message: string }>("derived://worker-failed", (event) => {
      log.error("previews and analysis worker stopped", {
        error: { message: event.payload.message },
      });
      // The backend already publishes the persistent worker failure notice.
      void useIssuesStore.getState().load();
    });
    await listeners.listen("derived://similarity-updated", () => {
      // A similarity rebuild after a settings change fires once per rebuilt
      // month bucket -- hundreds of events in a burst -- so this goes through
      // the same single-flight coalescer as scan progress (C-H1) instead of
      // calling refresh() unthrottled per event.
      refreshLibrarySoon();
    });
    await listeners.listen("watch://rescan-needed", () => {
      useSectionsStore.setState({ rescanNeeded: true });
    });
    await listeners.listen<{ reason: string }>("watch://failed", (event) => {
      useSectionsStore.setState({ rescanNeeded: true });
      log.error("filesystem watcher failed", {
        error: { message: event.payload.reason },
      });
      void useIssuesStore.getState().load();
    });
    await listeners.listen("failure://reported", () => {
      void useIssuesStore.getState().load();
    });
    await listeners.listen<{ message: string }>("failure://direct", (event) => {
      // Recorded core detail: shown as it arrived, not restated.
      presentEscapedDetail(event.payload.message);
    });
    await useSectionsStore.getState().loadIndexWork();
  },
  (error) => {
    log.warn("library event wiring failed", toErrorFields(error));
    recordInterfaceFailure(message("scan.liveUpdatesUnavailable"));
    presentEscapedFailure(message("scan.liveUpdatesUnavailableReload"));
  },
);

export function installScanEventWiring(): Promise<void> {
  return install();
}
