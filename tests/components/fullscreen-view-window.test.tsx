// @vitest-environment happy-dom

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, it } from "vitest";
import FullscreenViewWindow from "../../src/windows/FullscreenViewWindow";
import { EMPTY_ITEM_WORK } from "../../src/models/items";
import type { FullscreenViewBroadcast } from "../../src/workflows/fullscreen-view";
import { emitCalls, fireEvent as deliver, resetTauriMocks } from "../mocks/tauri";

const state: FullscreenViewBroadcast = {
  item: { hash: "photo", pathId: 1, fileName: "photo.jpg", resolvedUtcMs: null,
    copyCount: 1, width: 10, height: 10, hasThumb: true, similarGroupId: null,
    sharpness: null, faceScore: null, byteSize: 10, hasCompanions: false,
    durationMs: null, dirPaths: [], derivedWork: EMPTY_ITEM_WORK },
  detail: { fileName: "photo.jpg", kind: "image", byteSize: 10, width: 10, height: 10,
    durationMs: null, dateState: "undated", resolvedUtcMs: null, resolvedSource: null,
    dateOnly: false, copyPaths: [], copyCount: 0, companionPaths: [], stripFrames: null },
  index: 1, length: 3, pendingDelete: null, sectionKind: "image", failure: null,
};

beforeEach(() => resetTauriMocks());
afterEach(cleanup);

it("focuses the fullscreen command surface on entry and native-window reactivation", async () => {
  const user = userEvent.setup();
  render(<FullscreenViewWindow />);
  await act(async () => deliver("fullscreen-view://state", state));
  const surface = screen.getByLabelText("Fullscreen view");
  expect(document.activeElement).toBe(surface);
  await user.keyboard("{Enter}");
  expect(emitCalls.filter((call) => call.event === "fullscreen-view://key")).toEqual([]);

  screen.getByRole("button", { name: "Previous item" }).focus();
  await user.keyboard("{Enter}");
  expect(emitCalls).toContainEqual({ event: "fullscreen-view://key", payload: { key: "ArrowLeft", shiftKey: false } });
  fireEvent(window, new Event("focus"));
  expect(document.activeElement).toBe(surface);
  await user.keyboard(" ");
  expect(emitCalls).toContainEqual({ event: "fullscreen-view://key", payload: expect.objectContaining({ key: " " }) });
});

it("keeps the chrome hover- and focus-revealed, not permanently shown (D9)", async () => {
  render(<FullscreenViewWindow />);
  await act(async () => deliver("fullscreen-view://state", state));
  const surface = screen.getByLabelText("Fullscreen view");
  const header = screen.getByRole("button", { name: "Previous item" }).closest("header");
  expect(surface.className).toMatch(/(^|\s)group(\s|$)/);
  expect(header?.className).toContain("opacity-0");
  expect(header?.className).toContain("group-hover:opacity-100");
  expect(header?.className).toContain("focus-within:opacity-100");
  // Hidden by default does not mean unreachable: Tab still lands on its
  // controls, which is what keeps focus-within able to reveal it.
  screen.getByRole("button", { name: "Previous item" }).focus();
  expect(document.activeElement?.getAttribute("aria-label")).toBe("Previous item");
});

it("does not steal confirmation focus on reactivation or forward its decision keys", async () => {
  const user = userEvent.setup();
  render(<FullscreenViewWindow />);
  await act(async () => deliver("fullscreen-view://state", { ...state, pendingDelete: { kind: "permanent", key: "h1", fileName: "photo.jpg" } }));
  const cancel = screen.getByRole("button", { name: "Cancel" });
  expect(document.activeElement).toBe(cancel);
  fireEvent(window, new Event("focus"));
  expect(document.activeElement).toBe(cancel);
  await user.keyboard("{ArrowRight}{Enter}");
  expect(emitCalls).toContainEqual({ event: "fullscreen-view://confirm-delete", payload: {} });
  expect(emitCalls.filter((call) => call.event === "fullscreen-view://key")).toEqual([]);
});
