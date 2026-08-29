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
} from "../repositories";
import { applyTheme, applyUiFont } from "../utils/theme";
import { listen } from "@tauri-apps/api/event";
import { recordInterfaceFailure } from "../utils/failureSurface";

interface AppState {
  appData: LoadedAppData | null;
  loadError: string | null;
  /** Quarantines waiting to be shown. Dismissing clears them; nothing else
   * does, so the notice cannot be missed by a re-render. */
  quarantines: QuarantineRecord[];
  dismissQuarantines: () => void;
  initialize: () => Promise<LoadedAppData | null>;
  patchConfig: (patch: Record<string, unknown>) => Promise<void>;
  patchState: (patch: Record<string, unknown>) => Promise<void>;
}

// State writes are debounced and coalesced: selection/zoom/pane state can
// change per keystroke, and one write per pause is plenty (the backup store
// dedups identical content, but churn is churn).
let pendingStatePatch: Record<string, unknown> | null = null;
let stateFlushTimer: ReturnType<typeof setTimeout> | null = null;
const STATE_FLUSH_MS = 400;

// Startup appearance and application bootstrap both need the same document.
// Keep that first read single-flight: load_app_data also carries one-shot
// quarantine records, so two independent readers could let the appearance
// probe consume a recovery notice before the main surface sees it.
let initialization: Promise<LoadedAppData | null> | null = null;

export const useAppStore = create<AppState>((set, get) => ({
  appData: null,
  loadError: null,
  quarantines: [],

  dismissQuarantines: () => set({ quarantines: [] }),

  patchConfig: async (patch) => {
    try {
      const merged = await patchConfigFile(patch);
      set((s) =>
        s.appData === null ? s : { appData: { ...s.appData, config: merged } },
      );
      // The main window re-themes live; other windows apply at their load.
      applyTheme(merged.theme);
      applyUiFont(merged.uiFontFamily);
    } catch (error) {
      log.error("config patch failed", toErrorFields(error));
      throw error;
    }
  },

  patchState: async (patch) => {
    // Publish optimistically so readers see the new state immediately; the
    // disk write coalesces on a short timer.
    set((s) =>
      s.appData === null
        ? s
        : { appData: { ...s.appData, state: { ...(s.appData.state ?? {}), ...patch } } },
    );
    pendingStatePatch = { ...(pendingStatePatch ?? {}), ...patch };
    if (stateFlushTimer !== null) clearTimeout(stateFlushTimer);
    stateFlushTimer = setTimeout(() => {
      const toWrite = pendingStatePatch;
      pendingStatePatch = null;
      stateFlushTimer = null;
      if (toWrite === null) return;
      patchStateFile(toWrite).catch((error) => {
        log.error("state patch failed", toErrorFields(error));
      });
    }, STATE_FLUSH_MS);
  },

  initialize: () => {
    const loaded = get().appData;
    if (loaded !== null) return Promise.resolve(loaded);
    if (initialization !== null) return initialization;

    initialization = (async () => {
      try {
        const data = await loadAppData();
        set((s) => ({
          appData: data,
          loadError: null,
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
        set({ loadError: String(error) });
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
    recordInterfaceFailure(error instanceof Error ? error.message : String(error));
  }
})();
