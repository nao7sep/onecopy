// Managed-dependency state: one entry per registry row (the ffmpeg binary,
// native runtimes and model files), the footer chip reading the whole registry, a named
// modal with per-row actions, progress from the install events. Installs run
// IN PARALLEL per entry (developer, 2026-08-17) — the map below narrates each
// one independently, and only a second operation on the SAME entry is
// refused (core-side, per-id claims). No check ever runs automatically — the
// modal's one check button and the config-gated launch check are the only
// triggers (the honest-state model's UI counterpart).

import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  managedInstallLine,
  type ManagedInstallActivity,
  type ManagedInstallProgress,
} from "../models/dependencyProgress";
import { log, toErrorFields } from "../repositories";
import { recordInterfaceFailure } from "../utils/failureSurface";
import { recordActionFailure } from "./notifications-store";

export type DependencyStatus =
  | "not-installed"
  | "update-available"
  | "up-to-date"
  | "installed-unchecked";

export interface DependencyState {
  id: string;
  label: string;
  kind: "binary" | "runtime" | "model";
  status: DependencyStatus;
  /** Read from the artifact on every status — the binary's own banner, the
   * sidecar beside it, or a pinned artifact's verified-install identity, never from the
   * facts store. Null on a present entry means the version could not be read:
   * not absent, and never dressed up as up to date. */
  installedVersion: string | null;
  /** What the app RECORDED, which is only ever network knowledge: nothing here
   * describes the artifact on disk. */
  facts: {
    latestKnownVersion: string | null;
    lastCheckedAtUtc: string | null;
  };
  path: string;
  /** Absence blocks at least one built-in core presentation path, rather
   * than only withholding optional enrichment. */
  requiredForCore: boolean;
  /** True when this entry's "latest" is DISCOVERABLE — a binary resolved
   * live from upstream. A pinned artifact's latest is selected by the app build, so
   * there is nothing to look up and nothing to check. */
  checkable: boolean;
  /** A pinned artifact's upstream publication date; null for binaries, whose
   * live version answers that. */
  released: string | null;
}

export interface InstallStep {
  phase: string;
  text: string;
}

export interface ManagedInstallOperation extends ManagedInstallActivity {
  operationId: string;
}

export type BinaryInstallResult =
  | { outcome: "installed"; operationId: string; state: DependencyState }
  | { outcome: "cancelled"; operationId: string; state: DependencyState }
  | {
      outcome: "failed";
      operationId: string;
      state: DependencyState;
      error: string;
    };

/** Preserve phase changes while replacing noisy progress updates within a phase. */
function installStep(
  steps: InstallStep[] | undefined,
  phase: string,
  text: string,
): InstallStep[] {
  const next = [...(steps ?? [])];
  const index = next.findIndex((step) => step.phase === phase);
  if (index < 0) next.push({ phase, text });
  else next[index] = { phase, text };
  return next;
}

function terminalStep(
  steps: InstallStep[] | undefined,
  text: string,
): InstallStep[] {
  return installStep(steps, "result", text);
}

function operationId(): string {
  return globalThis.crypto.randomUUID();
}

function replaceEntry(
  entries: DependencyState[],
  replacement: DependencyState,
): DependencyState[] {
  return entries.map((entry) =>
    entry.id === replacement.id ? replacement : entry,
  );
}

function mergeRegistrySnapshot(
  current: DependencyState[],
  snapshot: DependencyState[],
  protectedIds: ReadonlySet<string>,
): DependencyState[] {
  const currentById = new Map(current.map((entry) => [entry.id, entry]));
  return snapshot.map((entry) =>
    protectedIds.has(entry.id) ? (currentById.get(entry.id) ?? entry) : entry,
  );
}

let registryRevision = 0;
let loadSequence = 0;

interface BinariesState {
  entries: DependencyState[];
  loading: boolean;
  loadError: string | null;
  /** Typed activity per entry currently installing — several at once is
   * normal (the whole point of per-id claims). */
  installing: Record<string, ManagedInstallOperation>;
  /** The current attempt's durable phase history, retained through its result. */
  installHistory: Record<string, InstallStep[]>;
  /** The last failure per entry, shown in its row until the next attempt. */
  errors: Record<string, string>;
  /** True while the registry-wide check runs (the button narrates it). */
  checking: boolean;
  checkingId: string | null;
  checkingOperationId: string | null;
  checkCancelling: boolean;
  /** Epoch ms until which re-checking is pointless (fresh-check cooldown —
   * kind to upstream APIs, and honest: nothing can have changed in a
   * minute). */
  cooldownUntil: number;
  /** The last run's plain-words outcome, shown beside the button for the
   * cooldown minute — "You're up to date" beats a hover tooltip. */
  lastCheckOutcome: string | null;
  lastCheckOutcomeLevel: "error" | "info" | null;
  modalOpen: boolean;
  load: () => Promise<void>;
  install: (id: string) => Promise<void>;
  cancel: (id: string) => Promise<void>;
  /** Installs every entry that needs anything — missing or updatable — all
   * at once. */
  installAll: () => Promise<void>;
  /** ONE check for the whole registry (per-entry checking was busywork):
   * sequential over the installed CHECKABLE entries — models have no
   * upstream to ask — applying each authoritative checked row directly. */
  checkAll: () => Promise<void>;
  cancelCheck: (id: string) => Promise<void>;
  setModalOpen: (open: boolean) => void;
}

export const useBinariesStore = create<BinariesState>((set, get) => ({
  entries: [],
  loading: false,
  loadError: null,
  installing: {},
  installHistory: {},
  errors: {},
  checking: false,
  checkingId: null,
  checkingOperationId: null,
  checkCancelling: false,
  cooldownUntil: 0,
  lastCheckOutcome: null,
  lastCheckOutcomeLevel: null,
  modalOpen: false,

  load: async () => {
    const sequence = ++loadSequence;
    const revision = registryRevision;
    const protectedIds = new Set(Object.keys(get().installing));
    if (get().checkingId !== null) protectedIds.add(get().checkingId!);
    set({ loading: true, loadError: null });
    try {
      const entries = await invoke<DependencyState[]>("binaries_state");
      if (sequence !== loadSequence) return;
      if (revision !== registryRevision) {
        set({ loading: false });
        return;
      }
      set((state) => ({
        entries: mergeRegistrySnapshot(
          state.entries,
          Array.isArray(entries) ? entries : [],
          protectedIds,
        ),
        loading: false,
        loadError: null,
      }));
    } catch (error) {
      if (sequence !== loadSequence) return;
      if (revision !== registryRevision) {
        set({ loading: false });
        return;
      }
      log.error("binaries state load failed", toErrorFields(error));
      set({ loading: false, loadError: "Managed tools are unavailable." });
    }
  },

  install: async (id) => {
    if (get().installing[id] !== undefined) return;
    const currentOperationId = operationId();
    registryRevision += 1;
    set((s) => {
      const errors = { ...s.errors };
      delete errors[id];
      return {
        installing: {
          ...s.installing,
          [id]: {
            operationId: currentOperationId,
            progress: null,
            cancelling: false,
          },
        },
        installHistory: { ...s.installHistory, [id]: [] },
        errors,
      };
    });
    try {
      const result = await invoke<BinaryInstallResult>("binaries_install", {
        id,
        operationId: currentOperationId,
      });
      if (result.operationId !== currentOperationId) return;
      let failedMessage: string | null = null;
      set((s) => {
        if (s.installing[id]?.operationId !== currentOperationId) return s;
        const installing = { ...s.installing };
        const errors = { ...s.errors };
        delete installing[id];
        if (result.outcome === "failed") {
          errors[id] = result.error;
          failedMessage = result.error;
        } else {
          delete errors[id];
        }
        return {
          entries: replaceEntry(s.entries, result.state),
          installing,
          installHistory: {
            ...s.installHistory,
            [id]: terminalStep(
              s.installHistory[id],
              result.outcome === "installed"
                ? "Installed"
                : result.outcome === "cancelled"
                  ? "Cancelled"
                  : "Install failed",
            ),
          },
          errors,
        };
      });
      if (failedMessage !== null) {
        recordActionFailure(
          "managed-tool-install-failed",
          "The managed-tool installation failed.",
          failedMessage,
        );
      }
    } catch (error) {
      set((s) => {
        if (s.installing[id]?.operationId !== currentOperationId) return s;
        const installing = { ...s.installing };
        delete installing[id];
        return {
          installing,
          installHistory: {
            ...s.installHistory,
            [id]: terminalStep(s.installHistory[id], "Install failed"),
          },
          errors: { ...s.errors, [id]: messageOf(error) },
        };
      });
      log.error("binaries install start failed", { id, ...toErrorFields(error) });
      recordActionFailure(
        "managed-tool-install-failed",
        "Couldn’t start installing this managed tool.",
        error,
      );
    }
  },

  cancel: async (id) => {
    const previous = get().installing[id];
    if (previous === undefined || previous.cancelling) return;
    set((s) => ({
      installing: {
        ...s.installing,
        [id]: { ...previous, cancelling: true },
      },
    }));
    try {
      await invoke<boolean>("binaries_cancel", {
        id,
        operationId: previous.operationId,
      });
    } catch (error) {
      log.error("binaries install cancellation failed", { id, ...toErrorFields(error) });
      set((state) => ({
        errors: {
          ...state.errors,
          [id]: "Couldn’t cancel this managed-tool installation.",
        },
      }));
      recordActionFailure(
        "managed-tool-cancel-failed",
        "Couldn’t cancel this managed-tool installation.",
        error,
      );
      set((s) => {
        if (
          s.installing[id]?.operationId !== previous.operationId ||
          s.installing[id]?.cancelling !== true
        ) return s;
        const installing = { ...s.installing };
        installing[id] = previous;
        return { installing };
      });
    }
  },

  installAll: async () => {
    const { entries, installing, install } = get();
    const actionable = entries.filter(
      (entry) =>
        (entry.status === "not-installed" || entry.status === "update-available") &&
        installing[entry.id] === undefined,
    );
    await Promise.all(actionable.map((entry) => install(entry.id)));
  },

  checkAll: async () => {
    const { entries, installing, checking } = get();
    if (checking) return;
    const started = Date.now();
    registryRevision += 1;
    set({
      checking: true,
      checkingId: null,
      checkingOperationId: null,
      checkCancelling: false,
      lastCheckOutcome: null,
      lastCheckOutcomeLevel: null,
    });
    const installed = entries.filter(
      (entry) =>
        entry.checkable &&
        entry.status !== "not-installed" &&
        installing[entry.id] === undefined,
    );
    let failures = 0;
    let cancelled = false;
    for (const entry of installed) {
      const currentOperationId = operationId();
      set({ checkingId: entry.id, checkingOperationId: currentOperationId });
      try {
        const states = await invoke<DependencyState[]>("binaries_check", {
          id: entry.id,
          operationId: currentOperationId,
        });
        set((s) => {
          if (s.checkingOperationId !== currentOperationId) return s;
          const errors = { ...s.errors };
          delete errors[entry.id];
          const checked = states.find((state) => state.id === entry.id);
          return {
            entries: checked === undefined ? s.entries : replaceEntry(s.entries, checked),
            errors,
          };
        });
      } catch (error) {
        const message = messageOf(error);
        if (message.includes("dependency operation cancelled")) {
          cancelled = true;
          break;
        }
        // A failed check writes nothing core-side (the honest-state rule) —
        // but silence here read as "the button does nothing", so the row
        // carries the reason.
        failures += 1;
        set((s) => ({
          errors: { ...s.errors, [entry.id]: `Check failed — ${message}` },
        }));
        log.error("binaries check failed", { id: entry.id, ...toErrorFields(error) });
      }
    }
    // Model checks are LOCAL pin comparisons: a real run can finish in tens
    // of milliseconds, which reads as a dead button. Hold the "Checking…"
    // state long enough to be a visible acknowledgement of the click.
    const elapsed = Date.now() - started;
    if (elapsed < MIN_CHECKING_MS) {
      await new Promise((resolve) => setTimeout(resolve, MIN_CHECKING_MS - elapsed));
    }
    // What the check actually cost, so a "the button feels slow" report can
    // be answered from the log instead of guessed at: workMs is the real
    // round trip, and the floor is the rest of what the user sees.
    const updates = get().entries.filter(
      (e) => e.checkable && e.status === "update-available",
    ).length;
    log.info("update check finished", {
      workMs: elapsed,
      entries: installed.length,
      failures,
      updates,
      cancelled,
    });
    const outcome =
      cancelled
        ? "Check cancelled"
        : failures > 0
        ? `${failures} check${failures === 1 ? "" : "s"} failed — see below`
        : updates > 0
          ? `${updates} update${updates === 1 ? "" : "s"} available`
          : "You're up to date";
    set({
      checking: false,
      checkingId: null,
      checkingOperationId: null,
      checkCancelling: false,
      cooldownUntil: cancelled ? 0 : Date.now() + COOLDOWN_MS,
      lastCheckOutcome: outcome,
      lastCheckOutcomeLevel: failures > 0 ? "error" : "info",
    });
    if (failures > 0) {
      recordActionFailure(
        "managed-tool-check-failed",
        `${failures} managed-tool check${failures === 1 ? "" : "s"} failed.`,
      );
    }
    setTimeout(() => {
      useBinariesStore.setState((state) =>
        state.lastCheckOutcome === outcome && !state.checking
          ? {
              cooldownUntil: 0,
              lastCheckOutcome: null,
              lastCheckOutcomeLevel: null,
            }
          : state,
      );
    }, cancelled ? MIN_CHECKING_MS : COOLDOWN_MS);
  },

  cancelCheck: async (id) => {
    const currentOperationId = get().checkingOperationId;
    if (
      get().checkingId !== id ||
      currentOperationId === null ||
      get().checkCancelling
    ) return;
    set({ checkCancelling: true });
    try {
      const active = await invoke<boolean>("binaries_cancel", {
        id,
        operationId: currentOperationId,
      });
      if (!active) set({ checkCancelling: false });
    } catch (error) {
      set({
        checkCancelling: false,
        lastCheckOutcome: "Couldn’t cancel the managed-tool check.",
        lastCheckOutcomeLevel: "error",
      });
      log.error("binaries check cancellation failed", { id, ...toErrorFields(error) });
      recordActionFailure(
        "managed-tool-check-cancel-failed",
        "Couldn’t cancel the managed-tool check.",
        error,
      );
    }
  },

  setModalOpen: (open) => set({ modalOpen: open }),
}));

/** How other apps do it: the button disables during the check and briefly
 * after, with a visible "Checked <time>" as the real feedback — a minute's
 * cooldown also keeps repeated clicks from leaning on upstream rate limits
 * (GitHub allows unauthenticated callers very few requests per hour). */
export const COOLDOWN_MS = 60_000;

/** The visible floor for the checking state, measured from the CLICK — the
 * work happens inside it and the sleep tops up the remainder. A FLOOR, never
 * a fixed wait: a slow ffmpeg check that really takes 900 ms shows 900 ms.
 *
 * 600 ms, chosen once the main-thread freeze and the missing press feedback
 * were fixed (2026-08-17). Below ~400 ms a state that appears and vanishes
 * reads as a flicker rather than an event; past ~1 s the app starts feeling
 * padded, and Nielsen's 1 s bound is where an interaction stops feeling
 * immediate. The floor no longer has to carry the whole message either: the
 * press darkens the button in the same frame, and the outcome line ("You're
 * up to date") persists for the cooldown minute as the durable proof. */
export const MIN_CHECKING_MS = 600;

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** The ffmpeg entry — the chip and the scan honesty both read this one row. */
export function ffmpegEntry(
  entries: DependencyState[] | null | undefined,
): DependencyState | null {
  return entries?.find((entry) => entry.id === "ffmpeg") ?? null;
}

void (async () => {
  try {
    await listen<{ id: string; operationId: string } & ManagedInstallProgress>(
      "binaries://progress",
      (event) => {
        const { id, operationId: eventOperationId, ...progress } = event.payload;
        useBinariesStore.setState((s) => {
          const active = s.installing[id];
          if (active?.operationId !== eventOperationId) return s;
          return {
            installing: {
              ...s.installing,
              [id]: {
                ...active,
                progress,
              },
            },
            installHistory: {
              ...s.installHistory,
              [id]: installStep(
                s.installHistory[id],
                progress.phase,
                managedInstallLine(progress),
              ),
            },
          };
        });
      },
    );
    // The launch-time update check (config-gated, core-side) finished after
    // this store's initial load — refresh so the chip reflects it.
    await listen("binaries://changed", () => {
      void useBinariesStore.getState().load();
    });
  } catch (error) {
    log.warn("binaries event wiring failed", toErrorFields(error));
    const message = error instanceof Error ? error.message : String(error);
    recordInterfaceFailure(message);
    useBinariesStore.setState({
      loadError: "Live managed-tool status is unavailable. Restart OneCopy to repair it.",
    });
  }
})();

/** What the footer's managed-tools chip says and how loudly, or null for
 * silence. One chip for the whole registry (developer, 2026-08-17 — the
 * registry outgrew ffmpeg, so the text must not read as if ffmpeg were the
 * only tool):
 *
 * - Installing: the live progress line, neutral.
 * - ffmpeg absent: a WARNING (developer, 2026-08-17, overruling the earlier
 *   neutral: without it every video and every HEIC is a placeholder — that
 *   is a capability hole, not an FYI) with appealing, remedy-shaped copy.
 * - Any entry with an update: a warning naming that tools want attention.
 * - A missing MODEL stays silent: it disables one enhancement, not a media
 *   kind, and its own feature surface names the remedy. Up-to-date and
 *   installed-unchecked stay silent per the managed-runtime-dependencies
 *   conventions (no permanent benign FYIs).
 */
export interface ToolsChip {
  text: string;
  role: "neutral" | "warning";
}

export function toolsChip(
  installing: boolean,
  progress: string,
  entries: DependencyState[],
): ToolsChip | null {
  if (installing) return { text: progress, role: "neutral" };
  if (ffmpegEntry(entries)?.status === "not-installed") {
    return { text: "Install video & HEIC support", role: "warning" };
  }
  const updates = entries.filter((e) => e.status === "update-available").length;
  if (updates > 0) {
    return {
      text: updates === 1 ? "Tool update available" : "Tool updates available",
      role: "warning",
    };
  }
  // The permanent informational line (fleet decision, 2026-08-21, superseding the
  // earlier all-silent tuning): a present ffmpeg whose currency is unknown shows in
  // normal muted ink so the user always has one standing path to notice tools may
  // be stale — never a warning tint, and quiet only when a check confirmed current.
  // Absent OPTIONAL models stay off the chip: their features surface the need at
  // point of use, and the modal lists them.
  const ffmpeg = ffmpegEntry(entries);
  if (ffmpeg?.status === "installed-unchecked") {
    return {
      text: ffmpeg.installedVersion === null ? "Tool version unreadable" : "Tools not checked",
      role: "neutral",
    };
  }
  return null;
}
