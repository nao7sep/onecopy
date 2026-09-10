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
import type {
  ManagedInstallActivity,
  ManagedInstallProgress,
} from "../models/dependencyProgress";
import { log, toErrorFields } from "../repositories";
import { recordInterfaceFailure } from "../utils/failureSurface";
import { createEventInstaller } from "../utils/eventInstallation";
import { recordActionFailure } from "./notifications-store";
import { useAppStore } from "./app-store";
import {
  finishActivityOperation,
  newActivityOperationId,
  recordActivity,
} from "../repositories/activity";

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
  /** Known transfer size; an archived runtime uses its download, not DLL size. */
  downloadBytes: number | null;
}

type BinaryCheckOutcome =
  | { outcome: "completed"; states: DependencyState[] }
  | { outcome: "cancelled" };

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
  /** The last failure per entry, shown in its row until the next attempt. */
  errors: Record<string, string>;
  /** True while the registry-wide check runs (the button narrates it). */
  checking: boolean;
  checkingId: string | null;
  checkingOperationId: string | null;
  checkCancelling: boolean;
  /** Brief acknowledgement owned by the button after a successful check. */
  checkFeedback: "checked" | null;
  /** Registry-check failures that are not owned by a particular entry. */
  checkError: string | null;
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
}

export const useBinariesStore = create<BinariesState>((set, get) => ({
  entries: [],
  loading: false,
  loadError: null,
  installing: {},
  errors: {},
  checking: false,
  checkingId: null,
  checkingOperationId: null,
  checkCancelling: false,
  checkFeedback: null,
  checkError: null,

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
    const currentOperationId = newActivityOperationId("managedTools");
    recordActivity({
      kind: "started",
      owner: "managedTools",
      operationId: currentOperationId,
      subject: "installTools",
      current: "running",
      reason: "user",
      itemCount: 1,
    });
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
        errors,
      };
    });
    try {
      const result = await invoke<BinaryInstallResult>("binaries_install", {
        id,
        operationId: currentOperationId,
      });
      if (result.operationId !== currentOperationId) {
        recordActivity({
          kind: "stale",
          owner: "managedTools",
          operationId: currentOperationId,
          previous: "running",
          current: "stale",
          reason: "staleResponse",
          itemCount: 1,
        });
        finishActivityOperation("managedTools", currentOperationId);
        return;
      }
      let failedMessage: string | null = null;
      let applied = false;
      set((s) => {
        if (s.installing[id]?.operationId !== currentOperationId) return s;
        applied = true;
        const installing = { ...s.installing };
        const errors = { ...s.errors };
        delete installing[id];
        if (result.outcome === "failed") {
          errors[id] = "The managed-tool installation could not finish. Try again.";
          failedMessage = result.error;
        } else {
          delete errors[id];
        }
        return {
          entries: replaceEntry(s.entries, result.state),
          installing,
          errors,
        };
      });
      if (!applied) {
        recordActivity({
          kind: "stale",
          owner: "managedTools",
          operationId: currentOperationId,
          previous: "running",
          current: "stale",
          reason: "superseded",
          itemCount: 1,
        });
        finishActivityOperation("managedTools", currentOperationId);
        return;
      }
      recordActivity({
        kind:
          result.outcome === "installed"
            ? "completed"
            : result.outcome === "cancelled"
              ? "cancelled"
              : "failed",
        owner: "managedTools",
        operationId: currentOperationId,
        previous: "running",
        current:
          result.outcome === "installed"
            ? "succeeded"
            : result.outcome === "cancelled"
              ? "cancelled"
              : "failed",
        reason:
          result.outcome === "installed"
            ? "completion"
            : result.outcome === "cancelled"
              ? "user"
              : "error",
        itemCount: 1,
      });
      finishActivityOperation("managedTools", currentOperationId);
      if (failedMessage !== null) {
        recordActionFailure(
          "managed-tool-install-failed",
          "The managed-tool installation failed.",
          failedMessage,
        );
      }
    } catch (error) {
      let applied = false;
      set((s) => {
        if (s.installing[id]?.operationId !== currentOperationId) return s;
        applied = true;
        const installing = { ...s.installing };
        delete installing[id];
        return {
          installing,
          errors: {
            ...s.errors,
            [id]: "The managed-tool installation could not start. Try again.",
          },
        };
      });
      if (!applied) {
        recordActivity({
          kind: "stale",
          owner: "managedTools",
          operationId: currentOperationId,
          previous: "running",
          current: "stale",
          reason: "superseded",
          itemCount: 1,
        });
        finishActivityOperation("managedTools", currentOperationId);
        return;
      }
      recordActivity({
        kind: "failed",
        owner: "managedTools",
        operationId: currentOperationId,
        previous: "running",
        current: "failed",
        reason: "error",
        itemCount: 1,
      });
      finishActivityOperation("managedTools", currentOperationId);
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
    recordActivity({
      kind: "stopping",
      owner: "managedTools",
      operationId: previous.operationId,
      previous: "running",
      current: "stopping",
      reason: "user",
      itemCount: 1,
    });
    try {
      await invoke<boolean>("binaries_cancel", {
        id,
        operationId: previous.operationId,
      });
    } catch (error) {
      const current = get().installing[id];
      if (
        current?.operationId !== previous.operationId ||
        current.cancelling !== true
      ) {
        recordActivity({
          kind: "stale",
          owner: "managedTools",
          operationId: previous.operationId,
          current: "stale",
          reason: "superseded",
          itemCount: 1,
        });
        return;
      }
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
    const activityOperationId = newActivityOperationId("managedTools");
    registryRevision += 1;
    set({
      checking: true,
      checkingId: null,
      checkingOperationId: null,
      checkCancelling: false,
      checkFeedback: null,
      checkError: null,
    });
    const installed = entries.filter(
      (entry) =>
        entry.checkable &&
        entry.status !== "not-installed" &&
        installing[entry.id] === undefined,
    );
    if (installed.length > 0) {
      try {
        await useAppStore.getState().patchState(
          { managedToolUpdateLastAttemptAtUtc: new Date().toISOString() },
          { immediate: true },
        );
      } catch (error) {
        const message = "OneCopy couldn’t record this managed-tool check. Try again.";
        log.error("managed-tool check attempt save failed", toErrorFields(error));
        recordActionFailure("managed-tool-check-attempt-save-failed", message, error);
        set({ checking: false, checkError: message });
        return;
      }
    }
    recordActivity({
      kind: "started",
      owner: "managedTools",
      operationId: activityOperationId,
      subject: "checkToolUpdates",
      current: "running",
      reason: "user",
      itemCount: installed.length,
    });
    let failures = 0;
    let cancelled = false;
    for (const entry of installed) {
      const currentOperationId = operationId();
      set({ checkingId: entry.id, checkingOperationId: currentOperationId });
      try {
        const outcome = await invoke<BinaryCheckOutcome>("binaries_check", {
          id: entry.id,
          operationId: currentOperationId,
        });
        if (outcome.outcome === "cancelled") {
          cancelled = true;
          break;
        }
        const states = outcome.states;
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
        // A failed check writes nothing core-side (the honest-state rule) —
        // but silence here read as "the button does nothing", so the row
        // carries the reason.
        failures += 1;
        set((s) => ({
          errors: {
            ...s.errors,
            [entry.id]: "This managed tool could not be checked. Try again.",
          },
        }));
        log.error("binaries check failed", { id: entry.id, ...toErrorFields(error) });
      }
    }
    const elapsed = Date.now() - started;
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
    set({
      checking: false,
      checkingId: null,
      checkingOperationId: null,
      checkCancelling: false,
      checkFeedback: !cancelled && failures === 0 ? "checked" : null,
      checkError:
        failures > 0
          ? `${failures} managed-tool check${failures === 1 ? "" : "s"} failed.`
          : null,
    });
    recordActivity({
      kind: cancelled ? "cancelled" : failures > 0 ? "failed" : "completed",
      owner: "managedTools",
      operationId: activityOperationId,
      previous: "running",
      current: cancelled ? "cancelled" : failures > 0 ? "failed" : "succeeded",
      reason: cancelled ? "user" : failures > 0 ? "error" : "completion",
      itemCount: installed.length,
    });
    finishActivityOperation("managedTools", activityOperationId);
    if (failures > 0) {
      recordActionFailure(
        "managed-tool-check-failed",
        `${failures} managed-tool check${failures === 1 ? "" : "s"} failed.`,
      );
    }
    if (!cancelled && failures === 0) setTimeout(() => {
      useBinariesStore.setState((state) =>
        state.checkFeedback === "checked" && !state.checking
          ? { checkFeedback: null }
          : state,
      );
    }, CHECKED_FEEDBACK_MS);
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
        checkError: "Couldn’t cancel the managed-tool check.",
      });
      log.error("binaries check cancellation failed", { id, ...toErrorFields(error) });
      recordActionFailure(
        "managed-tool-check-cancel-failed",
        "Couldn’t cancel the managed-tool check.",
        error,
      );
    }
  },

}));

export const CHECKED_FEEDBACK_MS = 2_000;

/** The ffmpeg entry — the chip and the scan honesty both read this one row. */
export function ffmpegEntry(
  entries: DependencyState[] | null | undefined,
): DependencyState | null {
  return entries?.find((entry) => entry.id === "ffmpeg") ?? null;
}

const installEvents = createEventInstaller(
  async (listeners) => {
    await listeners.listen<{ id: string; operationId: string } & ManagedInstallProgress>(
      "binaries://progress",
      (event) => {
        const { id, operationId: eventOperationId, ...progress } = event.payload;
        let accepted = false;
        let meaningfulProgress = false;
        useBinariesStore.setState((s) => {
          const active = s.installing[id];
          if (active?.operationId !== eventOperationId) return s;
          accepted = true;
          meaningfulProgress =
            active.progress?.phase !== progress.phase ||
            (progress.total !== null && progress.done === progress.total);
          return {
            installing: {
              ...s.installing,
              [id]: {
                ...active,
                progress,
              },
            },
          };
        });
        if (!accepted) {
          recordActivity({
            kind: "stale",
            owner: "managedTools",
            operationId: eventOperationId,
            current: "stale",
            reason: "staleResponse",
          });
        } else if (meaningfulProgress) {
          recordActivity({
            kind: "progressed",
            owner: "managedTools",
            operationId: eventOperationId,
            current: "running",
            done: progress.done,
            ...(progress.total === null ? {} : { total: progress.total }),
          });
        }
      },
    );
    // The launch-time update check (config-gated, core-side) finished after
    // this store's initial load — refresh so the chip reflects it.
    await listeners.listen("binaries://changed", () => {
      void useBinariesStore.getState().load();
    });
  },
  (error) => {
    log.warn("binaries event wiring failed", toErrorFields(error));
    recordInterfaceFailure(
      "Live managed-tool status is unavailable. Restart OneCopy to repair it.",
    );
    useBinariesStore.setState({
      loadError: "Live managed-tool status is unavailable. Restart OneCopy to repair it.",
    });
  },
);

/** Ready bootstrap owns app-lifetime managed-tool event admission. */
export function installBinariesEventWiring(): Promise<void> {
  return installEvents();
}

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
