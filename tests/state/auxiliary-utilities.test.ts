// @vitest-environment happy-dom
import { beforeEach, expect, it } from "vitest";
import { openBackgroundWorkFromViewing } from "../../src/workflows/auxiliary-utilities";
import { useAppShellStore } from "../../src/state/app-shell-store";
import { usePreviewStore } from "../../src/state/preview-store";
import { useFullscreenViewStore } from "../../src/state/fullscreen-view-store";
import { WebviewWindow, invokeCalls, mockCommands, resetTauriMocks, setFocus } from "../mocks/tauri";

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  useAppShellStore.setState({ utilitySurface: null });
  usePreviewStore.setState({ follow: true, placement: "window" });
  useFullscreenViewStore.setState({ session: null });
  mockCommands({ viewer_sequence_close: () => null, set_window_fullscreen: () => null });
});

it("opens management in Main while keeping separate Preview open", async () => {
  const preview = new WebviewWindow("preview");
  await openBackgroundWorkFromViewing();
  expect(useAppShellStore.getState().utilitySurface).toBe("backgroundWork");
  expect(usePreviewStore.getState().follow).toBe(true);
  expect(preview.hide).not.toHaveBeenCalled();
  expect(preview.destroy).not.toHaveBeenCalled();
  expect(setFocus).toHaveBeenCalled();
});

it("leaves fullscreen before bringing Main's management surface forward", async () => {
  const viewer = new WebviewWindow("fullscreen-view");
  useFullscreenViewStore.setState({ session: { token: "session" } as never });
  await openBackgroundWorkFromViewing();
  expect(useFullscreenViewStore.getState().session).toBeNull();
  expect(viewer.hide).toHaveBeenCalled();
  expect(useAppShellStore.getState().utilitySurface).toBe("backgroundWork");
  const commands = invokeCalls.map((call) => call.command);
  expect(commands.indexOf("set_window_fullscreen")).toBeLessThan(commands.indexOf("focus_window_while_active"));
});
