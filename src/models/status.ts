// What the status bar says, as a pure function of what the app knows.
//
// The bar used to render scan progress and NOTHING ELSE, so the strip was
// blank except during a scan — which made a finished scan look like the status
// bar had disappeared. The app-chrome conventions call a status bar "a curated
// summary surface" that is "always reserved and always visible"; a surface that
// is only ever occupied for a few seconds fails that on both counts.
//
// The standing state for an inbox-zero handler is HOW MUCH IS LEFT. It is the
// one number that answers "am I making progress", it changes with every cull,
// and it is the reason the app exists. Transient conditions outrank it, in the
// order below, because each is something the user needs to act on:
//
//   1. a delete that did not happen  — the one thing they just did that failed
//   2. an explicit file mutation     — the user's foreground destructive work
//   3. a scan in progress            — work happening now, with its own detail
//   4. rescan needed                 — the index is knowingly incomplete
//   5. the library totals            — the standing state, whenever nothing above
//
// Deliberately NOT here: the app version (About owns it), the ffmpeg version
// (the tools modal owns it, and the chip beside this line carries the states
// that matter), and anything that would accumulate.

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
  /** A failed delete or a refused command; null when the last action was clean. */
  message: string | null;
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
  if (input.message !== null && input.message !== "") {
    return { tone: "danger", text: input.message, title: input.message };
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
  if (input.progress?.phase === "indexed") {
    const terminal = progressLine(input.progress);
    return {
      tone: input.progress.failures > 0 ? "warning" : "normal",
      text: line === "" ? terminal : `${terminal} · ${line}`,
      title: progressTitle(input.progress),
    };
  }
  return line === ""
    ? { tone: "normal", text: "Nothing to handle" }
    : { tone: "normal", text: line };
}
