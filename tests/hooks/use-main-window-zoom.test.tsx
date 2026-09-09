// @vitest-environment happy-dom
import { act, cleanup, fireEvent, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { useMainWindowLifecycle } from "../../src/hooks/useMainWindowLifecycle";
import { useAppStore } from "../../src/state/app-store";
import { pushModal, resetModalStack } from "../../src/utils/modalStack";
import { mockCommands, resetTauriMocks, setZoom } from "../mocks/tauri";

vi.hoisted(() => Object.defineProperty(navigator, "platform", { value: "MacIntel", configurable: true }));

const appData = { config: {}, state: {}, dataRoot: "/test", debugEnabled: false, quarantines: [] };
beforeEach(() => {
  resetTauriMocks();
  resetModalStack();
  mockCommands({ patch_state: () => ({}), note_user_activity: () => null });
  useAppStore.setState({ appData });
});
afterEach(() => { cleanup(); resetModalStack(); });

it("keeps macOS Ctrl editing local while Cmd zoom remains available", async () => {
  renderHook(() => useMainWindowLifecycle({ appData, splitOpen: false }));
  await act(async () => {});
  const field = document.createElement("textarea");
  document.body.appendChild(field);
  field.focus();
  fireEvent.keyDown(field, { key: "=", ctrlKey: true });
  expect(setZoom).not.toHaveBeenCalled();
  fireEvent.keyDown(field, { key: "=", metaKey: true });
  expect(setZoom).toHaveBeenLastCalledWith(1.2);
  field.remove();
});

it("does not zoom after another owner consumes the event, during composition, or behind a dialog", async () => {
  renderHook(() => useMainWindowLifecycle({ appData, splitOpen: false }));
  await act(async () => {});
  const consumed = new KeyboardEvent("keydown", { key: "=", metaKey: true, cancelable: true });
  consumed.preventDefault();
  act(() => window.dispatchEvent(consumed));
  fireEvent.keyDown(window, { key: "=", metaKey: true, isComposing: true });
  fireEvent.keyDown(window, { key: "=", ctrlKey: true, altKey: true });
  pushModal({});
  fireEvent.keyDown(window, { key: "=", metaKey: true });
  expect(setZoom).not.toHaveBeenCalled();
});
