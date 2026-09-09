// @vitest-environment happy-dom

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, it } from "vitest";
import ViewerWindow from "../../src/windows/ViewerWindow";
import { EMPTY_ITEM_WORK } from "../../src/models/items";
import type { ViewerBroadcast } from "../../src/workflows/quick-view";
import { emitCalls, fireEvent as deliver, resetTauriMocks } from "../mocks/tauri";

const state: ViewerBroadcast = {
  item: { hash: "photo", pathId: 1, fileName: "photo.jpg", resolvedUtcMs: null,
    copyCount: 1, width: 10, height: 10, hasThumb: true, similarGroupId: null,
    sharpness: null, faceScore: null, byteSize: 10, hasCompanions: false,
    durationMs: null, dirPaths: [], derivedWork: EMPTY_ITEM_WORK },
  detail: { fileName: "photo.jpg", kind: "image", byteSize: 10, width: 10, height: 10,
    durationMs: null, dateState: "undated", resolvedUtcMs: null, resolvedSource: null,
    dateOnly: false, copyPaths: [], companionPaths: [], stripFrames: null },
  index: 1, length: 3, pendingDelete: null, sectionKind: "image", failure: null,
};

beforeEach(() => resetTauriMocks());
afterEach(cleanup);

it("focuses the fullscreen command surface on entry and native-window reactivation", async () => {
  const user = userEvent.setup();
  render(<ViewerWindow />);
  await act(async () => deliver("viewer://state", state));
  const surface = screen.getByLabelText("Fullscreen viewer");
  expect(document.activeElement).toBe(surface);
  await user.keyboard("{Enter}");
  expect(emitCalls.filter((call) => call.event === "viewer://key")).toEqual([]);

  screen.getByRole("button", { name: "Previous item" }).focus();
  await user.keyboard("{Enter}");
  expect(emitCalls).toContainEqual({ event: "viewer://key", payload: { key: "ArrowLeft", shiftKey: false } });
  fireEvent(window, new Event("focus"));
  expect(document.activeElement).toBe(surface);
  await user.keyboard(" ");
  expect(emitCalls).toContainEqual({ event: "viewer://key", payload: expect.objectContaining({ key: " " }) });
});

it("does not steal confirmation focus on reactivation or forward its decision keys", async () => {
  const user = userEvent.setup();
  render(<ViewerWindow />);
  await act(async () => deliver("viewer://state", { ...state, pendingDelete: "permanent" }));
  const cancel = screen.getByRole("button", { name: "Cancel" });
  expect(document.activeElement).toBe(cancel);
  fireEvent(window, new Event("focus"));
  expect(document.activeElement).toBe(cancel);
  await user.keyboard("{ArrowRight}{Enter}");
  expect(emitCalls).toContainEqual({ event: "viewer://confirm-delete", payload: {} });
  expect(emitCalls.filter((call) => call.event === "viewer://key")).toEqual([]);
});
