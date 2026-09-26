import { describe, expect, it } from "vitest";
import { transitionCoalescer, type CoalescerState } from "../../src/models/refresh-coalescer";

function apply(state: CoalescerState, events: Parameters<typeof transitionCoalescer>[1][]): CoalescerState {
  return events.reduce((current, event) => transitionCoalescer(current, event).state, state);
}

describe("transitionCoalescer", () => {
  it("starts a debounce timer on the first trigger from idle", () => {
    const result = transitionCoalescer("idle", { kind: "trigger" });
    expect(result).toEqual({ state: "debouncing", actions: ["startTimer"] });
  });

  it("absorbs repeated triggers while a timer is already pending, without restarting it", () => {
    const result = transitionCoalescer("debouncing", { kind: "trigger" });
    expect(result).toEqual({ state: "debouncing", actions: [] });
  });

  it("starts exactly one round when the debounce timer fires", () => {
    const result = transitionCoalescer("debouncing", { kind: "timerFired" });
    expect(result).toEqual({ state: "running", actions: ["startRound"] });
  });

  it("ignores a stray timer callback outside the debouncing state", () => {
    expect(transitionCoalescer("idle", { kind: "timerFired" })).toEqual({
      state: "idle",
      actions: [],
    });
    expect(transitionCoalescer("running", { kind: "timerFired" })).toEqual({
      state: "running",
      actions: [],
    });
  });

  it("is single-flight: a trigger while a round is running queues a trailing rerun instead of starting another round", () => {
    const result = transitionCoalescer("running", { kind: "trigger" });
    expect(result).toEqual({ state: "runningPending", actions: [] });
  });

  it("coalesces any number of triggers received while running into exactly one pending rerun", () => {
    const state = apply("running", [
      { kind: "trigger" },
      { kind: "trigger" },
      { kind: "trigger" },
      { kind: "triggerImmediate" },
    ]);
    expect(state).toBe("runningPending");
  });

  it("starts the trailing rerun immediately (no extra debounce) once the round completes", () => {
    const result = transitionCoalescer("runningPending", { kind: "roundCompleted" });
    expect(result).toEqual({ state: "running", actions: ["startRound"] });
  });

  it("returns to idle when a round completes with nothing pending", () => {
    const result = transitionCoalescer("running", { kind: "roundCompleted" });
    expect(result).toEqual({ state: "idle", actions: [] });
  });

  it("ignores a stray roundCompleted when no round is running", () => {
    expect(transitionCoalescer("idle", { kind: "roundCompleted" })).toEqual({
      state: "idle",
      actions: [],
    });
    expect(transitionCoalescer("debouncing", { kind: "roundCompleted" })).toEqual({
      state: "debouncing",
      actions: [],
    });
  });

  it("triggerImmediate from idle starts a round without a debounce timer", () => {
    expect(transitionCoalescer("idle", { kind: "triggerImmediate" })).toEqual({
      state: "running",
      actions: ["startRound"],
    });
  });

  it("triggerImmediate cancels a pending debounce and starts the round now", () => {
    expect(transitionCoalescer("debouncing", { kind: "triggerImmediate" })).toEqual({
      state: "running",
      actions: ["clearTimer", "startRound"],
    });
  });

  it("triggerImmediate while running still coalesces into one trailing rerun", () => {
    expect(transitionCoalescer("running", { kind: "triggerImmediate" })).toEqual({
      state: "runningPending",
      actions: [],
    });
    expect(transitionCoalescer("runningPending", { kind: "triggerImmediate" })).toEqual({
      state: "runningPending",
      actions: [],
    });
  });

  it("never allows more than one round in flight across an arbitrarily long burst of events", () => {
    // Simulates 8/s scan progress events during a slow round: only the first
    // trigger should ever have produced a "startRound" action; every event
    // packed on top of it while running must collapse into the same pending
    // rerun rather than piling up additional in-flight rounds.
    let state: CoalescerState = "idle";
    let startedRounds = 0;
    const burst = Array.from({ length: 500 }, () => ({ kind: "trigger" as const }));
    for (const event of burst) {
      const transition = transitionCoalescer(state, event);
      state = transition.state;
      startedRounds += transition.actions.filter((action) => action === "startRound").length;
    }
    // The debounce timer never fired in this simulation, so no round started
    // yet, and the whole burst coalesced into a single pending debounce.
    expect(startedRounds).toBe(0);
    expect(state).toBe("debouncing");

    // The timer fires, and now the round is "running". A further burst must
    // never start a second round while the first is in flight.
    const afterTimer = transitionCoalescer(state, { kind: "timerFired" });
    state = afterTimer.state;
    startedRounds += afterTimer.actions.filter((action) => action === "startRound").length;
    expect(startedRounds).toBe(1);

    for (const event of burst) {
      const transition = transitionCoalescer(state, event);
      state = transition.state;
      startedRounds += transition.actions.filter((action) => action === "startRound").length;
    }
    expect(startedRounds).toBe(1);
    expect(state).toBe("runningPending");

    // Completion runs exactly the one trailing rerun, then settles.
    const afterCompletion = transitionCoalescer(state, { kind: "roundCompleted" });
    state = afterCompletion.state;
    startedRounds += afterCompletion.actions.filter((action) => action === "startRound").length;
    expect(startedRounds).toBe(2);
    expect(state).toBe("running");

    const settled = transitionCoalescer(state, { kind: "roundCompleted" });
    expect(settled).toEqual({ state: "idle", actions: [] });
  });
});
