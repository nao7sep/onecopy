// Pure decision model for coalescing a repeated "please refresh" signal into a
// single-flight round with at most one trailing rerun. Scan-time events (source
// checking, file-information completion, similarity rebuilds) can arrive many
// times a second; without this, each one started its own counts+reconcile
// round regardless of whether the previous round had finished, so a round that
// took longer than the event rate queued backend work without bound.
//
// The state machine below owns only the decision of when a round should start.
// The effectful shell (a workflow module) owns the actual timer and the actual
// async round; it drives this reducer with events and executes the actions it
// returns.

export type CoalescerState = "idle" | "debouncing" | "running" | "runningPending";

export type CoalescerEvent =
  /** A debounced request: coalesce with any other request arriving before the
   * debounce timer fires. */
  | { kind: "trigger" }
  /** A request that skips the debounce and starts immediately unless a round
   * is already running, in which case it still queues exactly one rerun. */
  | { kind: "triggerImmediate" }
  /** The debounce timer fired. */
  | { kind: "timerFired" }
  /** The in-flight round finished (success or failure alike; the shell decides
   * what "finished" means for its own round). */
  | { kind: "roundCompleted" };

export type CoalescerAction = "startTimer" | "clearTimer" | "startRound";

export interface CoalescerTransition {
  state: CoalescerState;
  actions: CoalescerAction[];
}

/** One pure transition. Given the coalescer's current state and an incoming
 * event, returns the next state and the actions the shell must perform (start
 * or clear the debounce timer, start a round). No transition is a function of
 * time itself: the shell decides when a timer fires and reports it back. */
export function transitionCoalescer(
  state: CoalescerState,
  event: CoalescerEvent,
): CoalescerTransition {
  switch (event.kind) {
    case "trigger":
      if (state === "idle") return { state: "debouncing", actions: ["startTimer"] };
      // A debounce timer is already pending, or a round already owns the
      // eventual rerun; either way this signal is absorbed.
      return { state: coalescePending(state), actions: [] };
    case "triggerImmediate":
      if (state === "idle") return { state: "running", actions: ["startRound"] };
      if (state === "debouncing") return { state: "running", actions: ["clearTimer", "startRound"] };
      return { state: "runningPending", actions: [] };
    case "timerFired":
      // A timer can only legitimately fire from "debouncing"; a stray or
      // superseded timer callback in another state is ignored.
      return state === "debouncing"
        ? { state: "running", actions: ["startRound"] }
        : { state, actions: [] };
    case "roundCompleted":
      switch (state) {
        case "running":
          return { state: "idle", actions: [] };
        case "runningPending":
          return { state: "running", actions: ["startRound"] };
        case "idle":
        case "debouncing":
          // A stray completion (the shell should not report one while no
          // round is running) leaves the state untouched.
          return { state, actions: [] };
      }
  }
}

function coalescePending(state: "debouncing" | "running" | "runningPending"): CoalescerState {
  return state === "debouncing" ? "debouncing" : "runningPending";
}
