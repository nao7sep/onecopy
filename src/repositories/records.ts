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

type PageRequest = {
  query: RecordsQuery;
  success: (page: RecordsPage) => void;
  failure: (error: unknown) => void;
  automatic: boolean;
};

/** One physical page read and one coalesced next intent. A display timeout
 * reports failure without releasing the in-flight native read. */
export class RecordsPageReader {
  private active = false;
  private pending: PageRequest | null = null;
  private pendingTimer: ReturnType<typeof setTimeout> | undefined;

  get busy(): boolean { return this.active; }

  clearPending(): void {
    clearTimeout(this.pendingTimer);
    this.pendingTimer = undefined;
    this.pending = null;
  }

  read(query: RecordsQuery, success: PageRequest["success"], failure: PageRequest["failure"], automatic = false): void {
    const request = { query, success, failure, automatic };
    if (this.active) {
      // Live signals cannot displace an explicit filter change or retry.
      if (!automatic || this.pending === null || this.pending.automatic) {
        this.clearPending();
        this.pending = request;
        this.pendingTimer = setTimeout(() => {
          this.clearPending();
          request.failure(new Error("records page read is still waiting for the previous read"));
        }, RECORDS_READ_TIMEOUT_MS);
      }
      return;
    }
    this.start(request);
  }

  private start(request: PageRequest): void {
    this.active = true;
    let expired = false;
    const fail = (error: unknown) => {
      if (this.pending?.automatic) this.clearPending();
      request.failure(error);
    };
    const timer = setTimeout(() => {
      expired = true;
      fail(new Error("records page read timed out"));
    }, RECORDS_READ_TIMEOUT_MS);
    const complete = (deliver: () => void) => {
      clearTimeout(timer);
      this.active = false;
      const next = this.pending;
      this.clearPending();
      if (next !== null) this.start(next);
      // Release physical admission before publishing a settled result: React
      // can immediately request another page to fill an underfull viewport.
      if (!expired) deliver();
    };
    void invoke<RecordsPage>("records_page", { query: request.query }).then(
      (page) => complete(() => request.success(page)),
      (error: unknown) => {
        if (!expired && this.pending?.automatic) this.clearPending();
        complete(() => request.failure(error));
      },
    );
  }
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
