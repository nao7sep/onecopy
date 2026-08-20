// Managed-dependency state: one entry per registry row (the ffmpeg binary,
// the model files), the footer chip reading the whole registry, a named
// modal with per-row actions, progress from the install events. Installs run
// IN PARALLEL per entry (developer, 2026-08-17) — the map below narrates each
// one independently, and only a second operation on the SAME entry is
// refused (core-side, per-id claims). No check ever runs automatically — the
// modal's one check button and the config-gated launch check are the only
// triggers (the honest-state model's UI counterpart).

import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { log, toErrorFields } from "../repositories";

export type DependencyStatus =
  | "not-installed"
  | "update-available"
  | "up-to-date"
  | "installed-unchecked";

export interface DependencyState {
  id: string;
  label: string;
  kind: "binary" | "model";
  status: DependencyStatus;
  /** Read from the artifact on every status — the binary's own banner, the
   * sidecar beside it, or a model's size-exact match against its pin, never
   * from the facts store. Null on a present entry means the version could not
   * be read: not absent, and never dressed up as up to date. */
  installedVersion: string | null;
  /** What the app RECORDED, which is only ever network knowledge: nothing here
   * describes the artifact on disk. */
  facts: {
    latestKnownVersion: string | null;
    lastCheckedAtUtc: string | null;
  };
  path: string;
  /** True when this entry's "latest" is DISCOVERABLE — a binary resolved
   * live from upstream. A model's latest is a constant compiled into the
   * app, so there is nothing to look up and nothing to check. */
  checkable: boolean;
  /** A pinned artifact's upstream publication date — how old the model
   * actually is; null for binaries, whose live version answers that. */
  released: string | null;
}

/** Install-progress phase tokens rendered as words — this is not a console
 * app (developer, 2026-08-17); raw tokens never reach the user. */
const INSTALL_PHASE_LABELS: Record<string, string> = {
  resolve: "Resolving",
  download: "Downloading",
  verify: "Verifying",
  install: "Installing",
};

export function installLine(phase: string, detail: string): string {
  const label =
    INSTALL_PHASE_LABELS[phase] ?? phase.charAt(0).toUpperCase() + phase.slice(1);
  return `${label} — ${detail}`;
}

interface BinariesState {
  entries: DependencyState[];
  /** Progress line per entry currently installing — several at once is
   * normal (the whole point of per-id claims). */
  installing: Record<string, string>;
  /** The last failure per entry, shown in its row until the next attempt. */
  errors: Record<string, string>;
  /** True while the registry-wide check runs (the button narrates it). */
  checking: boolean;
  /** Epoch ms until which re-checking is pointless (fresh-check cooldown —
   * kind to upstream APIs, and honest: nothing can have changed in a
   * minute). */
  cooldownUntil: number;
  /** The last run's plain-words outcome, shown beside the button for the
   * cooldown minute — "You're up to date" beats a hover tooltip. */
  lastCheckOutcome: string | null;
  modalOpen: boolean;
  load: () => Promise<void>;
  install: (id: string) => Promise<void>;
  /** Installs every entry that needs anything — missing or updatable — all
   * at once. */
  installAll: () => Promise<void>;
  /** ONE check for the whole registry (per-entry checking was busywork):
   * sequential over the installed CHECKABLE entries — models have no
   * upstream to ask — refreshing state at the end. */
  checkAll: () => Promise<void>;
  setModalOpen: (open: boolean) => void;
}

export const useBinariesStore = create<BinariesState>((set, get) => ({
  entries: [],
  installing: {},
  errors: {},
  checking: false,
  cooldownUntil: 0,
  lastCheckOutcome: null,
  modalOpen: false,

  load: async () => {
    try {
      const entries = await invoke<DependencyState[]>("binaries_state");
      set({ entries: Array.isArray(entries) ? entries : [] });
    } catch (error) {
      log.error("binaries state load failed", toErrorFields(error));
    }
  },

  install: async (id) => {
    set((s) => {
      const errors = { ...s.errors };
      delete errors[id];
      return { installing: { ...s.installing, [id]: "Starting…" }, errors };
    });
    try {
      await invoke("binaries_install", { id });
    } catch (error) {
      set((s) => {
        const installing = { ...s.installing };
        delete installing[id];
        return { installing, errors: { ...s.errors, [id]: messageOf(error) } };
      });
      log.error("binaries install start failed", { id, ...toErrorFields(error) });
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
    set({ checking: true, lastCheckOutcome: null });
    const installed = entries.filter(
      (entry) =>
        entry.checkable &&
        entry.status !== "not-installed" &&
        installing[entry.id] === undefined,
    );
    let failures = 0;
    for (const entry of installed) {
      try {
        await invoke("binaries_check", { id: entry.id });
        set((s) => {
          const errors = { ...s.errors };
          delete errors[entry.id];
          return { errors };
        });
      } catch (error) {
        // A failed check writes nothing core-side (the honest-state rule) —
        // but silence here read as "the button does nothing", so the row
        // carries the reason.
        failures += 1;
        set((s) => ({
          errors: { ...s.errors, [entry.id]: `Check failed — ${messageOf(error)}` },
        }));
        log.error("binaries check failed", { id: entry.id, ...toErrorFields(error) });
      }
    }
    await get().load();
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
    });
    const outcome =
      failures > 0
        ? `${failures} check${failures === 1 ? "" : "s"} failed — see below`
        : updates > 0
          ? `${updates} update${updates === 1 ? "" : "s"} available`
          : "You're up to date";
    set({
      checking: false,
      cooldownUntil: Date.now() + COOLDOWN_MS,
      lastCheckOutcome: outcome,
    });
    setTimeout(() => {
      useBinariesStore.setState({ cooldownUntil: 0, lastCheckOutcome: null });
    }, COOLDOWN_MS);
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
    await listen<{ id: string; phase: string; detail: string }>(
      "binaries://progress",
      (event) => {
        useBinariesStore.setState((s) => ({
          installing: {
            ...s.installing,
            [event.payload.id]: installLine(event.payload.phase, event.payload.detail),
          },
        }));
      },
    );
    await listen<{ id: string }>("binaries://done", (event) => {
      useBinariesStore.setState((s) => {
        const installing = { ...s.installing };
        delete installing[event.payload.id];
        return { installing };
      });
      void useBinariesStore.getState().load();
    });
    await listen<{ id: string; message: string }>("binaries://error", (event) => {
      useBinariesStore.setState((s) => {
        const installing = { ...s.installing };
        delete installing[event.payload.id];
        return {
          installing,
          errors: { ...s.errors, [event.payload.id]: event.payload.message },
        };
      });
      void useBinariesStore.getState().load();
    });
    // The launch-time update check (config-gated, core-side) finished after
    // this store's initial load — refresh so the chip reflects it.
    await listen("binaries://changed", () => {
      void useBinariesStore.getState().load();
    });
  } catch (error) {
    log.warn("binaries event wiring failed", toErrorFields(error));
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
  return null;
}
