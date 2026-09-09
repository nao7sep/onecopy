// Main's standing library totals, live work, and context-bound command result.
// Persistent failures belong to Notifications/Issues; mutation receipts retain
// their own truthful accounting and dismissal boundary.

import type { SectionCounts } from "./sections";
import { progressLine, progressTitle, type ScanProgress } from "./scan";
import {
  mutationProgressLine,
  mutationResultLine,
  type MutationProgress,
  type MutationResult,
} from "./mutation";

export type StatusTone = "danger" | "warning" | "normal";

export interface Status {
  text: string;
  tone: StatusTone;
  /** Hover text where the short form leaves something out. */
  title?: string;
}

interface MaintenanceWork {
  running: boolean;
  stopping: boolean;
  progress: ScanProgress | null;
}

export function activeMaintenanceStatus(source: MaintenanceWork, information: MaintenanceWork) {
  const work = source.running ? source : information.running ? information : null;
  return {
    scanning: work !== null,
    stopping: work?.stopping ?? false,
    progress: work?.progress ?? null,
    workKind: source.running ? "source-check" as const : "file-information" as const,
  };
}

function total(sections: { count: number }[]): number {
  return sections.reduce((sum, section) => sum + section.count, 0);
}

/** `1,204 images · 87 videos · 15 other files`, kinds with nothing omitted. */
export function libraryLine(counts: SectionCounts): string {
  const parts: string[] = [];
  const push = (n: number, one: string, many: string) => {
    if (n > 0) parts.push(`${n.toLocaleString()} ${n === 1 ? one : many}`);
  };
  push(total(counts.images), "image", "images");
  push(total(counts.videos), "video", "videos");
  push(total(counts.others), "other file", "other files");
  return parts.join(" · ");
}

export function statusLine(input: {
  feedback: Status | null;
  mutation: { progress: MutationProgress; cancelling: boolean } | null;
  mutationResult: MutationResult | null;
  exiting: boolean;
  scanning: boolean;
  workKind?: "source-check" | "file-information";
  stopping: boolean;
  progress: ScanProgress | null;
  rescanNeeded: boolean;
  counts: SectionCounts | null;
}): Status {
  if (input.exiting) {
    return {
      tone: "normal",
      text: "Finishing current file before exit…",
      title: "OneCopy is waiting until it no longer owns an unsafe file change.",
    };
  }
  if (input.mutation !== null) {
    const text = mutationProgressLine(
      input.mutation.progress,
      input.mutation.cancelling,
    );
    return {
      tone: input.mutation.progress.failures > 0 ? "warning" : "normal",
      text,
      title: text,
    };
  }
  if (input.mutationResult !== null) {
    const text = mutationResultLine(input.mutationResult);
    const { summary } = input.mutationResult;
    return {
      tone: summary.error !== null
        ? "danger"
        : input.mutationResult.cancelled ||
            summary.filesFailed > 0 ||
            summary.itemsPartial > 0 ||
            summary.itemsUnstarted > 0 ||
            summary.filesUnstarted > 0
          ? "warning"
          : "normal",
      text,
      title: text,
    };
  }
  if (input.feedback !== null) return input.feedback;
  if (input.scanning) {
    const sourceCheck = input.workKind === "source-check";
    if (input.stopping) {
      return {
        tone: "normal",
        text: sourceCheck
          ? "Stopping source-folder check…"
          : "Pausing file-information work…",
        title: "Finishing the current safe step; unfinished work remains queued.",
      };
    }
    return input.progress === null
      ? {
          tone: "normal",
          text: sourceCheck ? "Checking source folders…" : "Completing file information…",
        }
      : {
          tone: "normal",
          text: progressLine(input.progress),
          title: progressTitle(input.progress),
        };
  }
  if (input.rescanNeeded) {
    return {
      tone: "warning",
      text: "Source-folder check needed",
      title: "OneCopy may have missed filesystem changes — run Check source folders to reconcile them",
    };
  }
  if (input.counts === null) {
    // Before the first counts land. Never blank: a blank strip reads as a
    // broken status bar rather than as an app that has not finished starting.
    return { tone: "normal", text: "Starting…" };
  }
  const line = libraryLine(input.counts);
  return line === ""
    ? { tone: "normal", text: "Nothing to handle" }
    : { tone: "normal", text: line };
}
