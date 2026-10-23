// @vitest-environment happy-dom
import { act, cleanup, fireEvent, render, renderHook, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { Menu, MenuItem } from "../../src/components/Menu";
import { comparisonLayerToken } from "../../src/workflows/comparison";
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

it("keeps Comparison zoom available until a nested dialog opens", async () => {
  renderHook(() => useMainWindowLifecycle({ appData, splitOpen: false }));
  await act(async () => {});
  pushModal(comparisonLayerToken);
  fireEvent.keyDown(window, { key: "=", metaKey: true });
  expect(setZoom).toHaveBeenLastCalledWith(1.2);
  pushModal({});
  fireEvent.keyDown(window, { key: "=", metaKey: true });
  expect(setZoom).toHaveBeenCalledTimes(1);
});

it("keeps global zoom shortcuts available from an open menu", async () => {
  function Harness() {
    const zoom = useMainWindowLifecycle({ appData, splitOpen: false });
    return <Menu ariaLabel="Commands" trigger={(props) => <button {...props}>Menu</button>}>
      <MenuItem onSelect={() => undefined}>Settings</MenuItem>
      <div><button onClick={zoom.zoomOut}>Zoom out</button><button onClick={zoom.zoomIn}>Zoom in</button></div>
    </Menu>;
  }
  render(<Harness />);
  await act(async () => {});
  fireEvent.click(screen.getByText("Menu"));
  const command = screen.getByRole("menuitem");
  fireEvent.keyDown(command, { key: "=", metaKey: true });
  expect(setZoom).toHaveBeenLastCalledWith(1.2);
  fireEvent.keyDown(command, { key: "-", metaKey: true });
  expect(setZoom).toHaveBeenLastCalledWith(1);
  expect(screen.getByRole("menu")).toBeTruthy();
});
