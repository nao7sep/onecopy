import type { MessageKey } from "../i18n/catalogues";
import type { Translator } from "../i18n/translate";
import { message, type Message } from "../i18n/translate";
import { formatBytes } from "./items";

export type ManagedInstallPhase =
  | "resolve"
  | "download"
  | "verify"
  | "install";

export interface ManagedInstallProgress {
  phase: ManagedInstallPhase;
  done: number;
  total: number | null;
  nextPhase: ManagedInstallPhase | null;
}

export interface ManagedInstallActivity {
  progress: ManagedInstallProgress | null;
  cancelling: boolean;
}

// The two byte-carrying phases name their own units inside one sentence, so
// each phase is one key rather than a label concatenated with a detail.
const PHASE_KEYS: Record<ManagedInstallPhase, MessageKey> = {
  resolve: "install.resolving",
  download: "install.downloading",
  verify: "install.verifying",
  install: "install.installing",
};

export function managedInstallLine(
  progress: ManagedInstallProgress,
  number: Translator["number"],
  percentOf: Translator["percent"],
): Message {
  const key = PHASE_KEYS[progress.phase];
  return progress.phase === "download" || progress.phase === "verify"
    ? message(key, { bytes: byteUnits(progress.done, progress.total, number, percentOf) })
    : message(key);
}

export function managedInstallActivityLine(
  activity: ManagedInstallActivity,
  number: Translator["number"],
  percentOf: Translator["percent"],
): Message {
  if (activity.cancelling) return message("common.cancelling");
  if (activity.progress === null) return message("common.starting");
  return managedInstallLine(activity.progress, number, percentOf);
}

function byteUnits(
  done: number,
  total: number | null,
  number: Translator["number"],
  percentOf: Translator["percent"],
): string {
  if (total === null || total <= 0) return formatBytes(done, number);
  const bounded = Math.min(done, total);
  const percent = Math.min(100, Math.floor((bounded * 100) / total));
  return `${formatBytes(bounded, number)} / ${formatBytes(total, number)} (${percentOf(percent / 100)})`;
}
