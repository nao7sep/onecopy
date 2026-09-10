// The app-level startup data (config + state + data root), owned in one store
// so every surface that needs the current config reads one source of truth —
// and a settings save can refresh it everywhere at once.
//
// This store is the ONE writer for both persisted documents: every mutation
// goes through patchConfig/patchState, which send only the changed keys, let
// the core merge into the file it holds, and publish the merged result here.
// No caller ever spreads a cached copy over the file again.

import { create } from "zustand";
import {
  loadAppData,
  log,
  patchConfigFile,
  patchStateFile,
  toErrorFields,
  type LoadedAppData,
  type QuarantineRecord,
  type StartupFailure,
} from "../repositories";
import { listen } from "@tauri-apps/api/event";
import { recordInterfaceFailure } from "../utils/failureSurface";
import { reportActionFailure } from "./notifications-store";

interface AppState {
  appData: LoadedAppData | null;
  startupFailure: StartupFailure | null;
  /** Quarantines waiting to be shown. Dismissing clears them; nothing else
   * does, so the notice cannot be missed by a re-render. */
  quarantines: QuarantineRecord[];
  dismissQuarantines: () => void;
  initialize: () => Promise<LoadedAppData | null>;
  patchConfig: (
    patch: Record<string, unknown>,
    options?: { reportFailure?: boolean },
  ) => Promise<void>;
  patchState: (
    patch: Record<string, unknown>,
    options?: { immediate?: boolean },
  ) => Promise<void>;
}

// State writes are debounced and coalesced: selection/zoom/pane state can
// change per keystroke, and one write per pause is plenty (the backup store
// dedups identical content, but churn is churn).
let pendingStatePatch: Record<string, unknown> | null = null;
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
  pendingStatePatch = null;
  pendingStateWaiters = [];
  stateFlushTimer = null;
  if (toWrite === null) return stateWriteTail;

  const write = stateWriteTail.then(() =>
    patchStateFile(toWrite).then(() => undefined),
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

export function reportStatePatchFailure(error: unknown): void {
  log.error("state patch failed", toErrorFields(error));
  reportActionFailure(
    "interface-state-save-failed",
    "OneCopy couldn’t save the current interface state. Your changes remain available in this session.",
    error,
  );
}

/** Settles a passive view-state write at the app-state owner. Explicit actions
 * that already have a local result await patchState directly instead. */
export function retainStatePatch(patch: Record<string, unknown>): void {
  void useAppStore.getState().patchState(patch).catch(reportStatePatchFailure);
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

// Main bootstrap is single-flight because load_app_data carries one-shot
// quarantine records. Auxiliary appearance reads never use this channel.
let initialization: Promise<LoadedAppData | null> | null = null;

export const useAppStore = create<AppState>((set, get) => ({
  appData: null,
  startupFailure: null,
  quarantines: [],

  dismissQuarantines: () => set({ quarantines: [] }),

  patchConfig: async (patch, options) => {
    try {
      const merged = await patchConfigFile(patch, options?.reportFailure ?? true);
      set((s) =>
        s.appData === null ? s : { appData: { ...s.appData, config: merged } },
      );
    } catch (error) {
      log.error("config patch failed", toErrorFields(error));
      throw error;
    }
  },

  patchState: async (patch, options) => {
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
          hasConfig: data.config !== null,
          hasState: data.state !== null,
        });
        return data;
      } catch (error) {
        set({
          startupFailure: {
            title: "OneCopy could not start safely",
            message:
              "OneCopy could not load its saved application data. Your existing files were not changed. Quit OneCopy, then try again.",
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
    recordInterfaceFailure("OneCopy could not monitor saved-data recovery. Reload the window before continuing.");
  }
})();
