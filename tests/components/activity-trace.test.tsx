// @vitest-environment happy-dom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import ActivityTraceModal from "../../src/components/ActivityTraceModal";
import { mockCommands, resetTauriMocks } from "../mocks/tauri";

const event = {
  sessionId: "session-one",
  sequence: 7,
  eventTimeUtc: "2026-09-07T00:00:00.000Z",
  monotonicMs: 42,
  kind: "progressed" as const,
  owner: "backgroundWork" as const,
  operationId: "background:previews",
  causeId: "priority:3",
  previous: "queued" as const,
  current: "running" as const,
  reason: "priorityChange" as const,
  queued: 10,
  done: 2,
  total: 10,
};

beforeEach(() => {
  resetTauriMocks();
  mockCommands({
    activity_snapshot: () => ({
      debugEnabled: true,
      sessionId: "session-one",
      monotonicNowMs: 100,
      events: [event],
    }),
  });
});

afterEach(() => {
  vi.restoreAllMocks();
  cleanup();
});

it("renders the core-owned sequence and causal link", async () => {
  render(<ActivityTraceModal open onClose={() => {}} />);
  expect(await screen.findByText(/Background Work · Progressed/)).toBeTruthy();
  expect(screen.getByText(/operation background:previews/)).toBeTruthy();
  expect(screen.getByText(/caused by priority:3/)).toBeTruthy();
  expect(screen.getByText("+42 ms")).toBeTruthy();
  expect(screen.getByText(/last event 58 ms ago/)).toBeTruthy();
});

it("copies the canonical events as JSONL", async () => {
  const writeText = vi.fn(async () => {});
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText },
  });
  render(<ActivityTraceModal open onClose={() => {}} />);
  await screen.findByText(/Background Work · Progressed/);
  fireEvent.click(screen.getByRole("button", { name: "Copy JSONL" }));
  await waitFor(() => expect(writeText).toHaveBeenCalledWith(JSON.stringify(event)));
  expect(await screen.findByText("Copied JSONL.")).toBeTruthy();
});
