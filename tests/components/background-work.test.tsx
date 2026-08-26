// @vitest-environment happy-dom

import { act, cleanup, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import BackgroundWorkModal from "../../src/components/BackgroundWorkModal";
import {
  backgroundWorkLine,
  mergeBackgroundRuntime,
  type BackgroundClassSnapshot,
  type BackgroundWorkSnapshot,
  useDerivedWorkStore,
} from "../../src/state/derived-work-store";
import { fireEvent, invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";

const ids: BackgroundClassSnapshot["id"][] = [
  "previews",
  "snapshots",
  "similarity",
  "faces",
  "transcripts",
];

function snapshot(
  overrides: Partial<BackgroundWorkSnapshot> = {},
  rows: Partial<Record<BackgroundClassSnapshot["id"], Partial<BackgroundClassSnapshot>>> = {},
): BackgroundWorkSnapshot {
  return {
    masterPaused: false,
    classes: ids.map((id) => ({
      id,
      state: "up-to-date" as const,
      queued: 0,
      failed: 0,
      done: null,
      total: null,
      reason: null,
      ...rows[id],
    })),
    ...overrides,
  };
}

let current: BackgroundWorkSnapshot;

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  current = snapshot({}, { previews: { state: "queued", queued: 12 } });
  mockCommands({
    background_work_snapshot: () => current,
    background_work_set_paused: ({ classId, paused }) => {
      if (classId === null) current = snapshot({ masterPaused: Boolean(paused) });
      return null;
    },
  });
  useDerivedWorkStore.setState({
    snapshot: current,
    open: true,
    loading: false,
    changing: null,
    error: null,
  });
});

afterEach(cleanup);

describe("Background work", () => {
  it("keeps the status segment meaningful for running, queued, and settled work", () => {
    expect(backgroundWorkLine(current)).toBe("Background work");
    expect(
      backgroundWorkLine(
        snapshot({}, { transcripts: { state: "running", queued: 1, done: 42, total: 100 } }),
      ),
    ).toBe("Transcription 42/100");
    expect(backgroundWorkLine(snapshot())).toBe("Background work: up to date");
  });

  it("patches runtime progress without re-reading output debt", () => {
    const merged = mergeBackgroundRuntime(current, {
      masterPaused: false,
      pausedClasses: [],
      active: { id: "previews", done: 4, total: 12, stopping: false },
    });

    expect(merged?.classes[0]).toMatchObject({ state: "running", done: 4, total: 12 });
    expect(merged?.classes[0].queued).toBe(12);
  });

  it("handles live runtime events without another database snapshot command", () => {
    fireEvent("derived://state-changed", {
      masterPaused: false,
      pausedClasses: [],
      active: { id: "previews", done: 5, total: 12, stopping: false },
    });

    expect(useDerivedWorkStore.getState().snapshot?.classes[0]).toMatchObject({
      state: "running",
      done: 5,
      total: 12,
    });
    expect(invokeCalls.some((call) => call.command === "background_work_snapshot")).toBe(false);
  });

  it("shows every fixed class and sends a class-specific pause", async () => {
    render(<BackgroundWorkModal />);

    for (const label of [
      "Previews and posters",
      "Video snapshots",
      "Similar photos",
      "Face scoring",
      "Transcription",
    ]) {
      expect(document.body.textContent).toContain(label);
    }

    const pause = [...document.querySelectorAll("button")].find(
      (button) => button.textContent === "Pause",
    );
    await act(async () => pause!.click());

    expect(
      invokeCalls.some(
        (call) =>
          call.command === "background_work_set_paused" &&
          call.args.classId === "previews" &&
          call.args.paused === true,
      ),
    ).toBe(true);
  });

  it("does not allow resume to race a class that is still stopping", () => {
    useDerivedWorkStore.setState({
      snapshot: snapshot(
        { masterPaused: true },
        { transcripts: { state: "stopping", queued: 3 } },
      ),
    });
    render(<BackgroundWorkModal />);

    const resumeAll = [...document.querySelectorAll("button")].find(
      (button) => button.textContent === "Resume all",
    );
    expect(resumeAll?.disabled).toBe(true);
    expect(document.body.textContent).toContain("Stopping and releasing resources…");
  });
});
