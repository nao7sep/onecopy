import { afterEach, expect, it, vi } from "vitest";
import { usePreviewStore } from "../../src/state/preview-store";
import {
  createdWindows,
  invokeCalls,
  mockCommands,
  resetTauriMocks,
  setFocus,
  setMonitors,
  setWindowCreatedHook,
  WebviewWindow,
} from "../mocks/tauri";

afterEach(() => {
  setWindowCreatedHook(null);
  vi.useRealTimers();
});

it("hands first-use placement to the native owner before showing", async () => {
  vi.useFakeTimers();
  resetTauriMocks();
  let nativePlacementApplied = false;
  mockCommands({
    set_window_fullscreen: () => null,
    capture_preview_window_placement: () => null,
    place_preview_window: () => { nativePlacementApplied = true; },
    log_event: () => null,
    record_recent_notification: () => null,
  });
  const monitors = [0, 1, 2].map((index) => ({
    name: `Screen ${index}`,
    position: { x: index * 1920, y: 0 },
    size: { width: 1920, height: 1080 },
    workArea: {
      position: { x: index * 1920, y: 30 },
      size: { width: 1920, height: 1050 },
    },
    scaleFactor: 1,
  }));
  setMonitors(monitors);
  usePreviewStore.setState({
    follow: false,
    placement: null,
    placementPreference: "window",
    fullscreen: false,
  });
  setWindowCreatedHook((label) => {
    void WebviewWindow.getByLabel(label).then((window) => {
      if (!window) throw new Error("Missing constructed window");
      window.show.mockImplementation(async () => {
        expect(nativePlacementApplied).toBe(true);
      });
      const created = (window.once.mock.calls as unknown as Array<[string, () => void]>)
        .find(([name]) => name === "tauri://created");
      if (!created) throw new Error("Missing creation registration");
      created[1]();
    });
  });
  const settle = async (operation: Promise<void>) => {
    await vi.advanceTimersByTimeAsync(1_000);
    await operation;
  };
  const open = () => usePreviewStore.getState().open(
    { hash: "h1", pathId: null },
    null,
    { screenPriority: monitors.map((monitor) => `${monitor.name}@${monitor.position.x},0`) },
  );

  await settle(open());
  const first = (await WebviewWindow.getByLabel("preview"))!;
  expect(invokeCalls.find(({ command }) => command === "place_preview_window")?.args)
    .toEqual({
      normal: { x: 2240, y: 155, width: 1280, height: 800 },
      maximized: true,
    });
  expect(first.setPosition).not.toHaveBeenCalled();
  expect(first.setSize).not.toHaveBeenCalled();
  expect(first.maximize).not.toHaveBeenCalled();
  expect(createdWindows[0].options).toMatchObject({ visible: false, focus: false });
  expect(setFocus).not.toHaveBeenCalled();

  usePreviewStore.getState().close();
  await settle(open());
  const commands = invokeCalls.map(({ command }) => command);
  expect(commands.indexOf("capture_preview_window_placement"))
    .toBeLessThan(commands.lastIndexOf("place_preview_window"));
  await settle(usePreviewStore.getState().setPlacementPreference("split"));
});
