// @vitest-environment happy-dom

import { act, cleanup, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import BackgroundWorkModal from "../../src/components/BackgroundWorkModal";
import {
  backgroundWorkLine,
  mergeBackgroundRuntime,
  mergeActiveItemWork,
  type BackgroundClassSnapshot,
  type BackgroundWorkSnapshot,
  useDerivedWorkStore,
} from "../../src/state/derived-work-store";
import { EMPTY_ITEM_WORK } from "../../src/models/items";
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
    background_work_snapshot: () => ({ ...current, activeItem: null }),
    background_work_set_paused: ({ classId, paused }) => {
      if (classId === null) current = snapshot({ masterPaused: Boolean(paused) });
      return null;
    },
    set_file_information_paused: () => null,
  });
  useDerivedWorkStore.setState({
    snapshot: current,
    open: true,
    loading: false,
    changing: null,
    error: null,
    activeItem: null,
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
      active: { id: "previews", hash: "photo-hash", done: 4, total: 12, stopping: false },
    });

    expect(merged?.classes[0]).toMatchObject({ state: "running", done: 4, total: 12 });
    expect(merged?.classes[0].queued).toBe(12);
  });

  it("overlays live progress only on the matching item and class", () => {
    const states = {
      ...EMPTY_ITEM_WORK,
      preview: {
        state: "pending" as const,
        hasValue: false,
        reason: null,
        done: null,
        total: null,
      },
    };
    const active = {
      id: "previews" as const,
      hash: "photo-hash",
      done: 4,
      total: 12,
      stopping: false,
    };

    expect(mergeActiveItemWork(states, "photo-hash", active).preview).toMatchObject({
      state: "running",
      done: 4,
      total: 12,
    });
    expect(mergeActiveItemWork(states, "other-hash", active)).toBe(states);
  });

  it("handles live runtime events without another database snapshot command", () => {
    fireEvent("derived://state-changed", {
      masterPaused: false,
      pausedClasses: [],
      active: { id: "previews", hash: "photo-hash", done: 5, total: 12, stopping: false },
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

    const previews = [...document.querySelectorAll("li")].find((row) =>
      row.textContent?.includes("Previews and posters"),
    );
    const pause = [...(previews?.querySelectorAll("button") ?? [])].find(
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

  it("keeps the existing master control scoped to previews and analysis", async () => {
    render(<BackgroundWorkModal />);

    const pauseAll = [...document.querySelectorAll("button")].find(
      (button) => button.textContent === "Pause previews and analysis",
    );
    await act(async () => pauseAll!.click());

    expect(
      invokeCalls.some(
        (call) =>
          call.command === "background_work_set_paused" &&
          call.args.classId === null &&
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

    const transcript = [...document.querySelectorAll("li")].find((row) =>
      row.textContent?.includes("Transcription"),
    );
    const resume = [...(transcript?.querySelectorAll("button") ?? [])].find(
      (button) => button.textContent === "Resume",
    );
    expect(resume?.disabled).toBe(true);
    expect(document.body.textContent).toContain("Stopping and releasing resources…");
  });
});
