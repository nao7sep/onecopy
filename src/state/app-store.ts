// The app-level startup data (config + state + data root), owned in one store
// so every surface that needs the current config reads one source of truth —
// and a settings save can refresh it everywhere at once.
//
// This store is the ONE writer for both persisted documents: every mutation
// goes through saveConfig/patchState, which send only the changed sets or
// keys, let the core decide what the file it holds receives, and publish the
// result here. No caller ever spreads a cached copy over the file again.

import { create } from "zustand";
import {
  loadAppData,
  log,
  saveConfigFile,
  patchStateFile,
  toErrorFields,
  type LoadedAppData,
  type QuarantineRecord,
  type StartupFailure,
} from "../repositories";
import { listen } from "@tauri-apps/api/event";
import { recordInterfaceFailure } from "../utils/failureSurface";
import { message } from "../i18n/translate";
import { reportActionFailure } from "./notifications-store";

interface AppState {
  appData: LoadedAppData | null;
  startupFailure: StartupFailure | null;
  /** Quarantines waiting to be shown. Dismissing clears them; nothing else
   * does, so the notice cannot be missed by a re-render. */
  quarantines: QuarantineRecord[];
  dismissQuarantines: () => void;
  initialize: () => Promise<LoadedAppData | null>;
  /** Queues one config write. `changes` may be computed from the settings as
   * published when the write starts; returning null skips the write. Resolves
   * to the published settings after it, or null when skipped. */
  saveConfig: (
    changes: ConfigChanges,
    options?: { reportFailure?: boolean },
  ) => Promise<Record<string, unknown> | null>;
  patchState: (
    patch: Record<string, unknown>,
    options?: { immediate?: boolean; reportFailure?: boolean },
  ) => Promise<void>;
}

export type ConfigChanges =
  | Record<string, unknown>
  | ((config: Record<string, unknown>) => Record<string, unknown> | null);

// Config writes run one at a time, in the order they were queued: the core
// handles each save_config call on its own worker, so overlapping calls could
// otherwise reach config.json in either order while the interface shows the
// later choice.
let configWriteTail: Promise<unknown> = Promise.resolve();
let configSequence = 0;
// Values published before they are saved (sound and volume while the user
// drags), each with the moment it was authored. A write confirms a key only
// when it carried that very value, authored no later than the write began;
// until then every publication shows the authored value, so neither an
// unrelated save nor a failed one makes the interface jump back.
const unsettledConfig = new Map<string, { value: unknown; sequence: number }>();

function publishConfig(saved: Record<string, unknown> | null): Record<string, unknown> | null {
  let published: Record<string, unknown> | null = null;
  useAppStore.setState((s) => {
    if (s.appData === null) return s;
    const authored = Object.fromEntries(
      [...unsettledConfig].map(([key, { value }]) => [key, value]),
    );
    published = { ...(saved ?? s.appData.config), ...authored };
    return { appData: { ...s.appData, config: published } };
  });
  return published;
}

function enqueueConfigWrite(
  changes: ConfigChanges,
  reportFailure: boolean,
): Promise<Record<string, unknown> | null> {
  const write = configWriteTail.then(async () => {
    const config = useAppStore.getState().appData?.config ?? {};
    const patch = typeof changes === "function" ? changes(config) : changes;
    if (patch === null) return null;
    const sequence = ++configSequence;
    let saved: Record<string, unknown>;
    try {
      saved = await saveConfigFile(patch, reportFailure);
    } catch (error) {
      log.error("config save failed", toErrorFields(error));
      throw error;
    }
    for (const [key, value] of Object.entries(patch)) {
      const authored = unsettledConfig.get(key);
      if (authored !== undefined && authored.sequence < sequence && Object.is(authored.value, value)) {
        unsettledConfig.delete(key);
      }
    }
    return publishConfig(saved);
  });
  configWriteTail = write.catch(() => undefined);
  return write;
}

/** Publishes settings at once and keeps them pending until a queued write
 * carries them; `saveUnsettledConfig` writes them. */
export function publishUnsettledConfig(patch: Record<string, unknown>): void {
  for (const [key, value] of Object.entries(patch)) {
    unsettledConfig.set(key, { value, sequence: ++configSequence });
  }
  publishConfig(null);
}

/** Queues a write of every published-but-unsaved setting. A failure keeps
 * them published and pending, for the next write or quit to retry. */
export function saveUnsettledConfig(): Promise<void> {
  return enqueueConfigWrite(
    () => unsettledConfig.size === 0
      ? null
      : Object.fromEntries([...unsettledConfig].map(([key, { value }]) => [key, value])),
    false,
  ).then(() => undefined);
}

/** Waits until every queued config write has settled and nothing published
 * is left unsaved. Rejects with the first failure, for quit to decide on. */
export async function flushConfigForShutdown(): Promise<void> {
  while (true) {
    const tail = configWriteTail;
    await tail;
    if (unsettledConfig.size > 0) {
      await saveUnsettledConfig();
      continue;
    }
    if (tail === configWriteTail) return;
  }
}

/** Test-only: forgets pending authored settings between fixtures. */
export function resetConfigWritesForTests(): void {
  unsettledConfig.clear();
  configWriteTail = Promise.resolve();
}

// State writes are debounced and coalesced: selection/zoom/pane state can
// change per keystroke, and one write per pause is plenty (the backup store
// dedups identical content, but churn is churn).
let pendingStatePatch: Record<string, unknown> | null = null;
// A coalesced write reports through the core only when every caller in it
// wants that; a caller with its own notice opts out so one failure is one
// record.
let pendingStateReportFailure = false;
// The last value each key held once actually confirmed on disk (or first
// observed, for a key no write has touched yet this session). A failed write
// restores from here, not from whatever the interface optimistically shows —
// which may already be a second, still-unconfirmed change. This map is never
// cleared merely because a write attempt started; only a SUCCESSFUL write
// advances a key's entry, so two failed writes in a row for the same key both
// roll back to the same last-good value instead of the first failure's
// now-stale optimistic snapshot (D-S15).
const ABSENT = Symbol("state key had no confirmed value");
let confirmedState: Record<string, unknown> = {};
function noteUnconfirmed(patch: Record<string, unknown>, published: Record<string, unknown>): void {
  for (const key of Object.keys(patch)) {
    if (key in confirmedState) continue;
    confirmedState[key] = key in published ? published[key] : ABSENT;
  }
}
let stateFlushTimer: ReturnType<typeof setTimeout> | null = null;
let pendingStateWaiters: Array<{
  resolve: () => void;
  reject: (error: unknown) => void;
}> = [];
let stateWriteTail: Promise<void> = Promise.resolve();
let stateShutdownStarted = false;
const STATE_FLUSH_MS = 400;

function flushPendingStatePatch(): Promise<void> {
  if (stateFlushTimer !== null) clearTimeout(stateFlushTimer);
  const toWrite = pendingStatePatch;
  const waiters = pendingStateWaiters;
  const reportFailure = pendingStateReportFailure;
  pendingStatePatch = null;
  pendingStateWaiters = [];
  pendingStateReportFailure = false;
  stateFlushTimer = null;
  if (toWrite === null) return stateWriteTail;

  const write = stateWriteTail.then(() =>
    patchStateFile(toWrite, reportFailure).then(() => undefined),
  );
  void write.then(
    () => {
      // These keys are now confirmed on disk with these values — the baseline
      // any later failure rolls back to.
      confirmedState = { ...confirmedState, ...toWrite };
    },
    () => revertPublishedState(confirmedState, toWrite),
  );
  stateWriteTail = write.catch(() => undefined);
  void write.then(
    () => {
      for (const waiter of waiters) waiter.resolve();
    },
    (error) => {
      for (const waiter of waiters) waiter.reject(error);
    },
  );
  return write;
}

// A write that never reached disk must not leave the interface claiming the new
// value: each key goes back unless something published a newer value meanwhile.
function revertPublishedState(
  previous: Record<string, unknown>,
  attempted: Record<string, unknown>,
): void {
  useAppStore.setState((s) => {
    if (s.appData === null) return s;
    const current = s.appData.state ?? {};
    const restored: Record<string, unknown> = { ...current };
    let changed = false;
    for (const [key, value] of Object.entries(attempted)) {
      if (current[key] !== value) continue;
      const base = previous[key];
      if (base === ABSENT) delete restored[key];
      else restored[key] = base;
      changed = true;
    }
    return changed ? { appData: { ...s.appData, state: restored } } : s;
  });
}

export function reportStatePatchFailure(error: unknown): void {
  log.error("state patch failed", toErrorFields(error));
  reportActionFailure(
    "interface-state-save-failed",
    message("app.stateSaveFailed"),
    error,
  );
}

/** Settles a passive view-state write at the app-state owner. Explicit actions
 * that already have a local result await patchState directly instead. */
export function retainStatePatch(patch: Record<string, unknown>): void {
  void useAppStore
    .getState()
    .patchState(patch, { reportFailure: false })
    .catch(reportStatePatchFailure);
}

/** Turns every later mutation into an immediate write and waits until the
 * serialized state boundary is stable. Shutdown calls this directly instead
 * of relying on another feature's save. */
export async function flushStatePatchesForShutdown(): Promise<void> {
  stateShutdownStarted = true;
  while (true) {
    await flushPendingStatePatch();
    const tail = stateWriteTail;
    await tail;
    if (pendingStatePatch === null && tail === stateWriteTail) return;
  }
}

/** Reopens ordinary coalescing when the native exit request fails and the app
 * remains alive. */
export function resumeStatePatchesAfterFailedShutdown(): void {
  stateShutdownStarted = false;
}

/** Test-only: forgets every key's confirmed-on-disk baseline, so a fixture
 * that seeds `appData.state` directly (bypassing patchState) starts from a
 * clean slate instead of an earlier test's baseline. */
export function resetConfirmedStateForTests(): void {
  confirmedState = {};
}

// Main bootstrap is single-flight because load_app_data carries one-shot
// quarantine records. Auxiliary appearance reads never use this channel.
let initialization: Promise<LoadedAppData | null> | null = null;

export const useAppStore = create<AppState>((set, get) => ({
  appData: null,
  startupFailure: null,
  quarantines: [],

  dismissQuarantines: () => set({ quarantines: [] }),

  saveConfig: (changes, options) => enqueueConfigWrite(changes, options?.reportFailure ?? true),

  patchState: async (patch, options) => {
    const published = get().appData?.state ?? {};
    noteUnconfirmed(patch, published);
    pendingStateReportFailure =
      pendingStateReportFailure || (options?.reportFailure ?? true);
    // Publish optimistically so readers see the new state immediately; the
    // disk write coalesces on a short timer.
    set((s) =>
      s.appData === null
        ? s
        : { appData: { ...s.appData, state: { ...(s.appData.state ?? {}), ...patch } } },
    );
    pendingStatePatch = { ...(pendingStatePatch ?? {}), ...patch };
    if (stateFlushTimer !== null) clearTimeout(stateFlushTimer);
    return new Promise<void>((resolve, reject) => {
      pendingStateWaiters.push({ resolve, reject });
      if (options?.immediate === true || stateShutdownStarted) {
        void flushPendingStatePatch().catch(() => undefined);
      } else {
        stateFlushTimer = setTimeout(() => {
          void flushPendingStatePatch().catch(() => undefined);
        }, STATE_FLUSH_MS);
      }
    });
  },

  initialize: () => {
    const loaded = get().appData;
    if (loaded !== null) return Promise.resolve(loaded);
    if (get().startupFailure !== null) return Promise.resolve(null);
    if (initialization !== null) return initialization;

    initialization = (async () => {
      try {
        const result = await loadAppData();
        if (result.status === "blocked") {
          set({ startupFailure: result.failure });
          return null;
        }
        const data = result.data;
        set((s) => ({
          appData: data,
          startupFailure: null,
          // Appended, never replaced: a mid-session quarantine event may already
          // be sitting here, and initialization must not swallow it.
          quarantines: [...s.quarantines, ...(data.quarantines ?? [])],
        }));
        log.info("app data loaded", {
          dataRoot: data.dataRoot,
          hasState: data.state !== null,
        });
        return data;
      } catch (error) {
        set({
          startupFailure: {
            title: "OneCopy could not start safely",
            message:
              "OneCopy could not load its saved application data. Your existing files were not changed. Quit OneCopy, then try again.",
            newerStores: [],
          },
        });
        log.error("app data load failed", toErrorFields(error));
        return null;
      } finally {
        initialization = null;
      }
    })();
    return initialization;
  },
}));

// A store can also be quarantined mid-session — a patch reads the file it is
// about to merge into — where there is no load result to carry the record. The
// core emits it instead, into the same list the boot load fills.
void (async () => {
  try {
    await listen<{ quarantines: QuarantineRecord[] }>("storage://quarantined", (event) => {
      const records = event.payload?.quarantines ?? [];
      if (records.length === 0) return;
      useAppStore.setState((s) => ({ quarantines: [...s.quarantines, ...records] }));
    });
  } catch (error) {
    log.warn("quarantine event wiring failed", toErrorFields(error));
    recordInterfaceFailure(message("app.recoveryMonitorFailed"));
  }
})();
