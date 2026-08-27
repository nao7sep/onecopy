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

const PHASE_LABELS: Record<ManagedInstallPhase, string> = {
  resolve: "Resolving",
  download: "Downloading",
  verify: "Verifying",
  install: "Installing",
};

export function managedInstallLine(progress: ManagedInstallProgress): string {
  const label = PHASE_LABELS[progress.phase];
  const units =
    progress.phase === "download" || progress.phase === "verify"
      ? byteUnits(progress.done, progress.total)
      : `${progress.done.toLocaleString()}/${progress.total?.toLocaleString() ?? "?"}`;
  const next =
    progress.total !== null &&
    progress.done >= progress.total &&
    progress.nextPhase !== null
      ? ` · Next: ${PHASE_LABELS[progress.nextPhase]}`
      : "";
  return `${label} — ${units}${next}`;
}

export function managedInstallActivityLine(
  activity: ManagedInstallActivity,
): string {
  if (activity.cancelling) return "Cancelling…";
  if (activity.progress === null) return "Starting…";
  return managedInstallLine(activity.progress);
}

function byteUnits(done: number, total: number | null): string {
  if (total === null || total <= 0) return formatBytes(done);
  const bounded = Math.min(done, total);
  const percent = Math.min(100, Math.floor((bounded * 100) / total));
  return `${formatBytes(bounded)} / ${formatBytes(total)} (${percent}%)`;
}
