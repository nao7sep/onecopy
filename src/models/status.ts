// Main's standing library totals, live work, and context-bound command result.
// Persistent failures belong to Notifications/Issues; mutation receipts retain
// their own truthful accounting and dismissal boundary.

import type { MessageKey } from "../i18n/catalogues";
import type { Message, Translator } from "../i18n/translate";
import type { SectionCounts } from "./sections";
import { progressLine, progressTitle, type ScanProgress } from "./scan";
import {
  mutationProgressLine,
  mutationResultLine,
  type MutationProgress,
  type MutationResult,
} from "./mutation";

export type StatusTone = "danger" | "warning" | "normal";

/** Resolved words, not a descriptor: the footer's line is a variable join of
 * independently translated facts (see `libraryLine`), and `statusLine` runs on
 * every render, so a language change re-composes it. */
export interface Status {
  text: string;
  tone: StatusTone;
  /** Hover text where the short form leaves something out. */
  title?: string;
  /** This line is a command's answer, so the footer gives it a live region.
   * Standing library facts and background progress do not interrupt. */
  announce?: boolean;
}

/** A command's outcome as it is HELD, which is the other half of the same
 * distinction: the store keeps it until the next command, so it carries
 * descriptors and `statusLine` resolves them on the render that shows them. */
export interface MainFeedback {
  text: Message;
  tone: StatusTone;
  title?: Message;
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

/** `1,204 images · 87 videos · 15 other files`, kinds with nothing omitted.
 *
 * A join of independent facts, each a locale-formatted count, so the
 * translator composes it here instead of one descriptor reaching the footer. */
export function libraryLine(counts: SectionCounts, t: Translator["t"]): string {
  const parts: string[] = [];
  const push = (n: number, key: MessageKey) => {
    if (n > 0) parts.push(t(key, { count: n }));
  };
  push(total(counts.images), "status.imageCount");
  push(total(counts.videos), "status.videoCount");
  push(total(counts.others), "status.otherFileCount");
  return parts.join(" · ");
}

export function statusLine(input: {
  feedback: MainFeedback | null;
  mutation: { progress: MutationProgress; cancelling: boolean } | null;
  mutationResult: MutationResult | null;
  exiting: boolean;
  scanning: boolean;
  workKind?: "source-check" | "file-information";
  stopping: boolean;
  progress: ScanProgress | null;
  rescanNeeded: boolean;
  counts: SectionCounts | null;
}, t: Translator["t"], number: Translator["number"], percentOf: Translator["percent"]): Status {
  if (input.exiting) {
    return {
      tone: "normal",
      text: t("status.exiting"),
      title: t("status.exitingTitle"),
    };
  }
  if (input.mutation !== null) {
    const text = mutationProgressLine(
      input.mutation.progress,
      input.mutation.cancelling,
      t,
      number,
    );
    return {
      tone: input.mutation.progress.failures > 0 ? "warning" : "normal",
      text,
      title: text,
    };
  }
  if (input.mutationResult !== null) {
    const text = mutationResultLine(input.mutationResult, t);
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
  if (input.feedback !== null) {
    const { text, tone, title } = input.feedback;
    return {
      tone,
      text: t(text.key, text.values),
      ...(title === undefined ? {} : { title: t(title.key, title.values) }),
      announce: true,
    };
  }
  if (input.scanning) {
    const sourceCheck = input.workKind === "source-check";
    if (input.stopping) {
      return {
        tone: "normal",
        text: t(sourceCheck ? "status.stoppingSourceCheck" : "status.pausingFileInformation"),
        title: t("status.stoppingTitle"),
      };
    }
    return input.progress === null
      ? {
          tone: "normal",
          text: t(sourceCheck ? "app.checkingSources" : "status.completingFileInformation"),
        }
      : {
          tone: "normal",
          text: progressLine(input.progress, t, percentOf),
          title: progressTitle(input.progress, t),
        };
  }
  if (input.rescanNeeded) {
    return {
      tone: "warning",
      text: t("status.rescanNeeded"),
      title: t("status.rescanNeededTitle"),
    };
  }
  if (input.counts === null) {
    // Before the first counts land. Never blank: a blank strip reads as a
    // broken status bar rather than as an app that has not finished starting.
    return { tone: "normal", text: t("common.starting") };
  }
  const line = libraryLine(input.counts, t);
  return line === ""
    ? { tone: "normal", text: t("app.nothingToHandle") }
    : { tone: "normal", text: line };
}
