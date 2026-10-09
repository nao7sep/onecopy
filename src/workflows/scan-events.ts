// Application-edge reactions to source checking, file-information completion,
// watcher updates, and derived-output events.

import { scopeIncludes, type SectionItem, type SectionLocation } from "../models/items";
import type { MutationKind } from "../models/mutation";
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

import { refreshSourceAvailability } from "./source-availability";

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

// Every library change event may name the Main sections it changed; an
// absent `sections` is an unscoped change that refreshes whatever Main shows.
interface ScopedChange {
  sections?: SectionLocation[] | null;
}

interface SequencedProgress extends ScopedChange {
  eventSequence: number;
  progress: ScanProgress;
}

// Single-flight counts+refresh, driven by the pure coalescer model. Scan-time
// progress can fire 8 times a second; without single-flight gating, a round
// slower than the event rate queued another round on top of it without bound
// (C-H1). At most one round runs at a time, and at most one more is queued to
// run immediately after it finishes.
//
// A round is either "light" or "full". A full round round-trips the whole
// selection through `reconcile_section`. A light round reloads only the window
// Main wants and reconciles only when the section order changed since the last
// reconcile (the items store owns that check), so high-frequency progress
// ticks do not serialize a 100k+ selection each way when nothing moved
// (C-M2). Watcher updates and completion events request (and, once requested,
// keep) a full round; if a full round is requested while a light one is
// pending or running, the eventual round escalates to full rather than
// downgrading.
let libraryRefreshState: CoalescerState = "idle";
let libraryRefreshFull = false;
// null means an unscoped event; it dominates any narrower pending changes.
let libraryRefreshSections: Map<string, SectionLocation> | null = new Map();
let libraryRefreshComparison = false;
function addRefreshScope(sections?: SectionLocation[] | null): void {
  if (sections == null) libraryRefreshSections = null;
  else if (libraryRefreshSections !== null) {
    for (const section of sections) libraryRefreshSections.set(`${section.kind}:${section.month}`, section);
  }
}

function runLibraryRefreshRound(): void {
  const full = libraryRefreshFull;
  libraryRefreshFull = false;
  const scope = libraryRefreshSections;
  libraryRefreshSections = new Map();
  const comparison = libraryRefreshComparison;
  libraryRefreshComparison = false;
  const affected = scopeIncludes(scope === null ? null : [...scope.values()], useItemsStore.getState().selected);
  void Promise.allSettled([
    useSectionsStore.getState().loadCounts(),
    affected ? (full ? useItemsStore.getState().refresh() : useItemsStore.getState().refreshWindow()) : Promise.resolve(),
    useIssuesStore.getState().load(),
    comparison ? reconcileComparisonMembership() : Promise.resolve(),
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

/** Debounced, single-flight, light refresh for high-frequency progress
 * signals (source-check/file-information progress, similarity relabeling):
 * coalesces bursts and never starts a round on top of one already in flight. */
function refreshLibrarySoon(sections?: SectionLocation[] | null): void {
  addRefreshScope(sections);
  driveLibraryRefresh({ kind: "trigger" });
}

/** Debounced, single-flight, full refresh for signals that can remove section
 * members but still arrive as a burst (watcher updates): still coalesced, but
 * the eventual round reconciles the whole selection rather than only the
 * window. */
function refreshLibrarySoonFull(sections?: SectionLocation[] | null): void {
  addRefreshScope(sections);
  libraryRefreshFull = true;
  driveLibraryRefresh({ kind: "trigger" });
}

/** Immediate, full refresh for low-frequency, must-be-accurate completions (an
 * operation finished, a full source check ended). Still single-flight: if a
 * round is already running, this queues exactly one trailing rerun rather
 * than starting a second round concurrently. */
function refreshLibraryNow(sections?: SectionLocation[] | null): void {
  addRefreshScope(sections);
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
    await listeners.listen("source://availability", () => {
      void refreshSourceAvailability().then(() => useIssuesStore.getState().load());
    });
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
        refreshLibrarySoon(event.payload.sections);
      }
    });
    await listeners.listen<ScopedChange & { eventSequence: number; sourceCheck: Omit<SourceCheckState, "progress">; stopped?: boolean; error?: string }>(
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
        void refreshSourceAvailability();
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
        refreshLibraryNow(event.payload.sections);
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
        refreshLibrarySoon(event.payload.sections);
      }
    });
    await listeners.listen<ScopedChange & { eventSequence: number; error?: string }>("file-information://done", (event) => {
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
      refreshLibraryNow(event.payload.sections);
      void useSectionsStore.getState().loadIndexWork();
    });

    await listeners.listen<ScopedChange>("watch://updated", (event) => {
      // The watcher can remove section members (an external delete/move),
      // so it requests a full round.
      libraryRefreshComparison = true;
      refreshLibrarySoonFull(event.payload.sections);
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
    await listeners.listen<ScopedChange>("derived://similarity-updated", (event) => {
      // A similarity rebuild after a settings change fires once per rebuilt
      // month bucket -- hundreds of events in a burst -- so this goes through
      // the same single-flight coalescer as scan progress (C-H1) instead of
      // calling refresh() unthrottled per event.
      refreshLibrarySoon(event.payload.sections);
    });
    await listeners.listen("watch://rescan-needed", () => {
      useSectionsStore.setState({ rescanNeeded: true });
    });
    await listeners.listen<ScopedChange & { rescanNeeded: boolean }>("watch://recovered", (event) => {
      useSectionsStore.setState({ rescanNeeded: event.payload.rescanNeeded });
      void refreshSourceAvailability();
      refreshLibraryNow(event.payload.sections);
      void reconcileComparisonMembership();
    });
    // Delete and destination batches are refreshed by the workflow that
    // started them, which awaits that refresh before recovering Main's
    // selection. Restoring and emptying deleted files start from the Deleted
    // files surfaces, which do not refresh Main, so their completion does.
    const refreshAfterMutation = (kind: MutationKind, sections: SectionLocation[] | null | undefined) => {
      if (kind !== "restore" && kind !== "trash-empty") return;
      if (sections != null && sections.length === 0) return;
      refreshLibraryNow(sections);
    };
    await listeners.listen<ScopedChange & { progress: { kind: MutationKind } }>("mutation://done", (event) => {
      refreshAfterMutation(event.payload.progress.kind, event.payload.sections);
    });
    await listeners.listen<ScopedChange & { kind: MutationKind }>("mutation://error", (event) => {
      refreshAfterMutation(event.payload.kind, event.payload.sections);
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
    recordInterfaceFailure(message("scan.liveUpdatesUnavailable"), error);
    presentEscapedFailure(message("scan.liveUpdatesUnavailableReload"));
  },
);

export function installScanEventWiring(): Promise<void> {
  return install();
}
