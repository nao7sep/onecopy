// The Records window's reads (src-tauri/src/records_view.rs) and its live
// signal. Each read is bounded here, so a stalled read shows the window's
// failure note rather than loading forever.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  RecordDetail,
  RecordKind,
  RecordSources,
  RecordsPage,
  RecordsQuery,
} from "../models/records";

export const RECORDS_READ_TIMEOUT_MS = 10_000;
// Sent by the core after each commit that stored a record.
export const RECORDS_CHANGED_EVENT = "records://changed";

async function bounded<T>(read: Promise<T>, what: string): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      read,
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new Error(`${what} timed out`)), RECORDS_READ_TIMEOUT_MS);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

export function readRecordsPage(query: RecordsQuery): Promise<RecordsPage> {
  return bounded(invoke<RecordsPage>("records_page", { query }), "records page read");
}

export function readRecordDetail(kind: RecordKind, id: number): Promise<RecordDetail | null> {
  return bounded(invoke<RecordDetail | null>("records_detail", { kind, id }), "record read");
}

export function readRecordSources(): Promise<RecordSources> {
  return bounded(invoke<RecordSources>("records_sources"), "record sources read");
}

/** The list pane's saved width, or null when none is saved. */
export function readRecordsListWidth(): Promise<number | null> {
  return bounded(invoke<number | null>("records_list_width"), "records list width read");
}

export function saveRecordsListWidth(width: number): Promise<unknown> {
  return invoke("patch_state", { patch: { recordsListWidth: width } });
}

export function openRecordsWindow(): Promise<void> {
  return invoke<void>("open_records_window");
}

/** Calls `listener` after each stored record; resolves to the unsubscribe. */
export function onRecordsChanged(listener: () => void): Promise<() => void> {
  return listen(RECORDS_CHANGED_EVENT, () => listener());
}
