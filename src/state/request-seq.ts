// Stale-response guards for the async read commands.
//
// The get_* reads used to run on Tauri's main thread, which serialized them
// FIFO — and the stores quietly leaned on that: fire two reloads, the second
// response always landed second. Moving the reads onto the async runtime (so
// a 30k-item month query stops freezing the window) breaks that promise:
// responses may arrive OUT OF ORDER, and an older snapshot landing last would
// resurrect deleted rows or roll counts backwards until the next refresh.
//
// The rule, one line per store: begin() stamps a request; the returned
// function answers "is this still the latest?" at response time. Anything
// that is not the latest is discarded unread — the newer request's response
// carries strictly fresher data for the same question.

export interface RequestSeq {
  /** Stamps a new request and returns its freshness check. */
  begin: () => () => boolean;
}

export function requestSeq(): RequestSeq {
  let current = 0;
  return {
    begin: () => {
      current += 1;
      const seq = current;
      return () => seq === current;
    },
  };
}
