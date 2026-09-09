// @vitest-environment happy-dom

import { act, cleanup, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import BackgroundWorkModal from "../../src/components/BackgroundWorkModal";
import {
  BackgroundActivityProjection,
  backgroundWorkLine,
  backgroundRows,
  backgroundRowCanResume,
  mergeBackgroundRuntime,
  mergeActiveItemWork,
  installDerivedWorkEventWiring,
  type BackgroundClassSnapshot,
  type BackgroundWorkSnapshot,
  useDerivedWorkStore,
} from "../../src/state/derived-work-store";
import { EMPTY_ITEM_WORK } from "../../src/models/items";
import { fireEvent, invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";
import { useSectionsStore } from "../../src/state/sections-store";

const ids: BackgroundClassSnapshot["id"][] = [
  "previews",
  "snapshots",
  "similarity",
  "faces",
  "video-transcripts",
  "audio-transcripts",
];

function snapshot(
  overrides: Partial<BackgroundWorkSnapshot> = {},
  rows: Partial<Record<BackgroundClassSnapshot["id"], Partial<BackgroundClassSnapshot>>> = {},
): BackgroundWorkSnapshot {
  return {
    workerRunning: true, pausedClasses: [],
    activeItem: null,
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
let wiringInstalled = false;

beforeEach(async () => {
  resetTauriMocks({ keepListeners: true });
  current = snapshot({}, { previews: { state: "queued", queued: 12 } });
  mockCommands({
    background_work_snapshot: () => ({ ...current, activeItem: null }),
    background_work_set_paused: ({ classId, paused }) => {
      const targets = classId === null ? ids : [classId as typeof ids[number]];
      const pausedSet = new Set(current.pausedClasses);
      for (const id of targets) { if (paused) pausedSet.add(id); else pausedSet.delete(id); }
      current = { ...current, workerRunning: current.workerRunning || !paused, pausedClasses: [...pausedSet] };
      if (classId === null) useSectionsStore.setState((state) => ({
        fileInformation: { ...state.fileInformation, paused: Boolean(paused) },
      }));
      return null;
    },
    set_file_information_paused: () => null,
    index_work_snapshot: () => useSectionsStore.getState(),
  });
  if (!wiringInstalled) {
    wiringInstalled = true;
    await installDerivedWorkEventWiring();
  }
  useDerivedWorkStore.setState({
    snapshot: current,
    loading: false,
    changing: null,
    error: null,
    activeItem: null,
  });
  useSectionsStore.setState({
    error: null,
    sourceCheck: {
      running: false,
      stopping: false,
      waiting: false,
      lastResult: "stopped",
      eventSequence: 0,
      progress: null,
    },
    fileInformation: {
      running: false,
      paused: false,
      stopping: false,
      queued: false,
      eventSequence: 0,
      progress: null,
    },
  });
});

afterEach(cleanup);

describe("Background work", () => {
  it("directly resumes stopped processing without unpausing other classes or enabling Settings-disabled work", async () => {
    current = snapshot({ workerRunning: false, pausedClasses: ["video-transcripts"] }, {
      previews: { state: "queued", queued: 12, failed: 2 },
      "audio-transcripts": { state: "disabled", reason: "Off in Settings" },
    });
    useDerivedWorkStore.setState({ snapshot: current });
    const view = render(<BackgroundWorkModal open onClose={() => {}} />);
    expect(backgroundWorkLine(current)).toBe("Preparation and enrichment stopped");
    const previews = [...view.container.querySelectorAll("li")].find((row) => row.textContent?.includes("Thumbnails, previews, and posters"))!;
    expect(previews.textContent).toContain("Stopped");
    expect(previews.querySelector("button")!.textContent).toBe("Resume");
    await act(async () => previews.querySelector("button")!.click());
    expect(invokeCalls).toContainEqual({ command: "background_work_set_paused", args: { classId: "previews", paused: false } });
    expect(current.workerRunning).toBe(true);
    expect(current.pausedClasses).toEqual(["video-transcripts"]);
    expect(current.classes[0].failed).toBe(2);
    const audio = [...view.container.querySelectorAll("li")].find((row) => row.textContent?.includes("Audio transcription"))!;
    expect(audio.querySelector("button")!.disabled).toBe(true);
  });

  it("keeps active manual work pausable when its automatic coordinator is stopped", () => {
    const state = snapshot({ workerRunning: false, activeItem: { id: "video-transcripts", hash: "manual", done: 2, total: 10, stopping: false } });
    const rows = backgroundRows(state);
    const transcription = rows.find((row) => row.id === "video-transcripts")!;
    expect(backgroundRowCanResume(state, transcription)).toBe(false);
    expect(backgroundRowCanResume(state, rows[0])).toBe(true);
    expect(backgroundWorkLine(state)).toBe("Video transcription 2/10");
  });

  it("projects coordinator pulses as one class lifecycle until authoritative quiet", () => {
    const projection = new BackgroundActivityProjection();
    const running = {
      workerRunning: true, pausedClasses: [],
      active: {
        id: "previews" as const,
        hash: "private-hash",
        done: null,
        total: null,
        stopping: false,
      },
    };

    const started = projection.observe(running, "priority:7");
    expect(started).toMatchObject([
      {
        kind: "started",
        subject: "previews",
        causeId: "priority:7",
        current: "running",
      },
    ]);
    const operationId = started[0]!.operationId;
    expect(operationId).toMatch(/^backgroundWork:/);
    expect(projection.observe({ ...running, active: null }, "priority:7")).toEqual([]);
    expect(projection.observe(running, "priority:7")).toEqual([]);
    expect(projection.quiet("priority:7")).toMatchObject([
      {
        kind: "completed",
        subject: "previews",
        operationId,
        causeId: "priority:7",
        current: "idle",
      },
    ]);
    expect(projection.quiet("priority:7")).toEqual([]);
  });

  it("keeps the status segment meaningful for running, queued, and settled work", () => {
    expect(backgroundWorkLine(current)).toBe("Thumbnails, previews, and posters: 12 queued");
    expect(
      backgroundWorkLine(
        snapshot(
          {},
          {
            "video-transcripts": {
              state: "running",
              queued: 1,
              done: 42,
              total: 100,
            },
          },
        ),
      ),
    ).toBe("Video transcription 42/100");
    expect(backgroundWorkLine(snapshot())).toBe("Background work: no work running");
  });

  it("patches runtime progress without re-reading output debt", () => {
    const merged = mergeBackgroundRuntime(current, {
      workerRunning: true, pausedClasses: [],
      active: {
        id: "previews",
        hash: "photo-hash",
        done: 4,
        total: 12,
        stopping: false,
      },
    });

    expect(backgroundRows(merged!)[0]).toMatchObject({
      state: "running",
      done: 4,
      total: 12,
    });
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
      workerRunning: true, pausedClasses: [],
      active: {
        id: "previews",
        hash: "photo-hash",
        done: 5,
        total: 12,
        stopping: false,
      },
    });

    expect(backgroundRows(useDerivedWorkStore.getState().snapshot!)[0]).toMatchObject({
      state: "running",
      done: 5,
      total: 12,
    });
    expect(invokeCalls.some((call) => call.command === "background_work_snapshot")).toBe(false);
  });

  it("shows every fixed class and sends a class-specific pause", async () => {
    render(<BackgroundWorkModal open onClose={() => {}} />);

    for (const label of [
      "Thumbnails, previews, and posters",
      "Video snapshots",
      "Similar photos",
      "Face scoring",
      "Video transcription",
      "Audio transcription",
    ]) {
      expect(document.body.textContent).toContain(label);
    }

    const previews = [...document.querySelectorAll("li")].find((row) =>
      row.textContent?.includes("Thumbnails, previews, and posters"),
    );
    const pause = [...(previews?.querySelectorAll("button") ?? [])].find((button) => button.textContent === "Pause");
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

  it("pauses every pausable row without installing a master lock", async () => {
    render(<BackgroundWorkModal open onClose={() => {}} />);

    const pauseAll = [...document.querySelectorAll("button")].find(
      (button) => button.textContent === "Pause all",
    );
    await act(async () => pauseAll!.click());

    expect(
      invokeCalls.some(
        (call) =>
          call.command === "background_work_set_paused" && call.args.classId === null && call.args.paused === true,
      ),
    ).toBe(true);
    expect(useSectionsStore.getState().fileInformation.paused).toBe(true);
    const previews = [...document.querySelectorAll("li")].find((row) => row.textContent?.includes("Thumbnails, previews, and posters"))!;
    const resume = [...previews.querySelectorAll("button")].find((button) => button.textContent === "Resume")!;
    expect(resume.disabled).toBe(false);
    await act(async () => resume.click());
    expect(current.pausedClasses).not.toContain("previews");
    expect(current.pausedClasses).toContain("video-transcripts");
    expect(useSectionsStore.getState().fileInformation.paused).toBe(true);
    expect(invokeCalls.some((call) => call.command === "stop_source_check")).toBe(false);
  });

  it("retains failed and unavailable debt across running, stopping, pause, and resume overlays", () => {
    for (const state of ["failed", "unavailable", "disabled"] as const) {
      const base = snapshot({}, { previews: { state, failed: 2, reason: "Inspect Issues" } });
      const running = mergeBackgroundRuntime(base, { workerRunning: true, pausedClasses: [], active: { id: "previews", hash: "x", done: 1, total: 2, stopping: false } })!;
      expect(backgroundRows(running)[0].state).toBe("running");
      const paused = mergeBackgroundRuntime(running, { workerRunning: true, pausedClasses: ["previews"], active: null })!;
      const resumed = mergeBackgroundRuntime(paused, { workerRunning: true, pausedClasses: [], active: null })!;
      expect(backgroundRows(resumed)[0]).toEqual(base.classes[0]);
    }
  });

  it("keeps failed output facts without duplicating the Issues surface", async () => {
    current = snapshot({}, { previews: { state: "failed", failed: 2 } });
    useDerivedWorkStore.setState({ snapshot: current });
    render(<BackgroundWorkModal open onClose={() => {}} />);
    expect(backgroundWorkLine(current)).toBe("Background work: no work running");
    expect(document.body.textContent).not.toMatch(/failed|Issues|up to date|could not be prepared/i);
    expect(document.body.textContent).toContain("No work running");
    expect(useDerivedWorkStore.getState().snapshot?.classes[0].failed).toBe(2);
  });

  it("distinguishes a completed source pass from a stopped or failed pass", () => {
    useSectionsStore.setState((state) => ({
      sourceCheck: { ...state.sourceCheck, lastResult: "completed" },
    }));
    const view = render(<BackgroundWorkModal open onClose={() => {}} />);
    expect(document.body.textContent).toContain("Completed");

    act(() => {
      useSectionsStore.setState((state) => ({
        sourceCheck: { ...state.sourceCheck, lastResult: "failed" },
      }));
    });
    expect(document.body.textContent).toContain("Could not complete check");
    act(() => {
      useSectionsStore.setState((state) => ({ sourceCheck: { ...state.sourceCheck, lastResult: "completed-with-issues" } }));
    });
    expect(document.body.textContent).toContain("Finished with incomplete checks");
    expect([...document.querySelectorAll("button")].some((button) => button.textContent === "Open Issues")).toBe(false);
    view.unmount();
  });

  it("does not allow resume to race a class that is still stopping", () => {
    useDerivedWorkStore.setState({
      snapshot: snapshot({ workerRunning: true, pausedClasses: ids, activeItem: { id: "video-transcripts", hash: "video", done: null, total: null, stopping: true } }, { "video-transcripts": { state: "queued", queued: 3 } }),
    });
    render(<BackgroundWorkModal open onClose={() => {}} />);

    const transcript = [...document.querySelectorAll("li")].find((row) =>
      row.textContent?.includes("Video transcription"),
    );
    const resume = [...(transcript?.querySelectorAll("button") ?? [])].find(
      (button) => button.textContent === "Resume",
    );
    expect(resume?.disabled).toBe(true);
    expect(document.body.textContent).toContain("Stopping and releasing resources…");
  });

  it("keeps Settings-disabled features disabled after Pause all while enabled rows can resume", async () => {
    current = snapshot({}, { "audio-transcripts": { state: "disabled", reason: "Off in Settings" } });
    useDerivedWorkStore.setState({ snapshot: current });
    const view = render(<BackgroundWorkModal open onClose={() => {}} />);
    await act(async () => view.getByRole("button", { name: "Pause all" }).click());
    const audio = [...view.container.querySelectorAll("li")].find((row) => row.textContent?.includes("Audio transcription"))!;
    expect(audio.textContent).toContain("Off in Settings");
    expect(audio.querySelector("button")!.disabled).toBe(true);
    const video = [...view.container.querySelectorAll("li")].find((row) => row.textContent?.includes("Video transcription"))!;
    await act(async () => video.querySelector("button")!.click());
    expect(current.pausedClasses).not.toContain("video-transcripts");
    expect(current.classes.find((row) => row.id === "audio-transcripts")!.state).toBe("disabled");
    expect(view.container.textContent).toContain("Settings decides which optional features are enabled");
  });
});
