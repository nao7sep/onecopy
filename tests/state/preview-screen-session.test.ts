import { afterEach, expect, it, vi } from "vitest";
import { usePreviewStore } from "../../src/state/preview-store";
import { flushPreviewWindowPlacement } from "../../src/state/preview-store";
import { createdWindows, invokeCalls, mockCommands, outerPosition, resetTauriMocks, setFocus, setMonitors, setWindowCreatedHook, WebviewWindow } from "../mocks/tauri";

afterEach(() => { setWindowCreatedHook(null); vi.useRealTimers(); });

it("allocates before showing, retains only session placement, and never raises Main over Preview", async () => {
  vi.useFakeTimers();
  resetTauriMocks();
  mockCommands({ set_window_fullscreen: () => null, log_event: () => null, record_recent_notification: () => null });
  const monitors = [0, 1, 2].map((index) => ({
    name: `Screen ${index}`, position: { x: index * 1920, y: 0 }, size: { width: 1920, height: 1080 },
    workArea: { position: { x: index * 1920, y: 30 }, size: { width: 1920, height: 1050 } }, scaleFactor: 1,
  }));
  setMonitors(monitors);
  usePreviewStore.setState({ follow: false, placement: null, placementPreference: "window", fullscreen: false });
  const geometries = new Map<WebviewWindow, { x: number; y: number; width: number; height: number; maximized: boolean }>();
  setWindowCreatedHook((label) => {
    void WebviewWindow.getByLabel(label).then((window) => {
      if (!window) throw new Error("Missing constructed window");
      const geometry = { x: 0, y: 30, width: 1280, height: 800, maximized: false };
      geometries.set(window, geometry);
      window.outerPosition.mockImplementation(async () => ({ x: geometry.x, y: geometry.y }));
      window.outerSize.mockImplementation(async () => ({ width: geometry.width, height: geometry.height }));
      window.setPosition.mockImplementation(async (position) => { Object.assign(geometry, position); });
      window.setSize.mockImplementation(async (size) => { Object.assign(geometry, size); });
      window.maximize.mockImplementation(async () => { geometry.maximized = true; });
      window.unmaximize.mockImplementation(async () => { geometry.maximized = false; });
      window.isMaximized.mockImplementation(async () => geometry.maximized);
      const created = (window.once.mock.calls as unknown as Array<[string, () => void]>).find(([name]) => name === "tauri://created");
      if (!created) throw new Error("Missing creation registration");
      created[1]();
    });
  });
  const settle = async (operation: Promise<void>) => {
    await vi.advanceTimersByTimeAsync(1_000);
    await operation;
  };
  const open = () => usePreviewStore.getState().open({ hash: "h1", pathId: null }, null, {
    previewWindowBounds: { x: 100, y: 100, width: 1000, height: 700 }, previewWindowMaximized: false,
  });
  await settle(open());
  const first = (await WebviewWindow.getByLabel("preview"))!;
  const firstGeometry = geometries.get(first)!;
  expect(firstGeometry).toMatchObject({ maximized: true });
  expect(firstGeometry.x).toBeGreaterThanOrEqual(1920);
  expect(firstGeometry.x + firstGeometry.width).toBeLessThanOrEqual(3840);
  expect(createdWindows[0].options).toMatchObject({ visible: false, focus: false });
  expect(first.setPosition.mock.invocationCallOrder[0]).toBeLessThan(first.maximize.mock.invocationCallOrder[0]);
  expect(first.maximize.mock.invocationCallOrder[0]).toBeLessThan(first.show.mock.invocationCallOrder[0]);
  expect(first.setAlwaysOnTop.mock.calls).toEqual([[true], [false]]);
  expect(setFocus).not.toHaveBeenCalled();
  expect(first.setFocus).not.toHaveBeenCalled();

  // The user unmaximizes and moves Preview onto a third screen.
  Object.assign(firstGeometry, { x: 4000, y: 100, width: 900, height: 700, maximized: false });
  await settle(usePreviewStore.getState().setPlacementPreference("split"));
  await settle(usePreviewStore.getState().setPlacementPreference("window"));
  const second = (await WebviewWindow.getByLabel("preview"))!;
  expect(geometries.get(second)).toEqual({ x: 4000, y: 100, width: 900, height: 700, maximized: false });

  // Main moving onto that screen makes the next opening allocate elsewhere.
  outerPosition.mockResolvedValue({ x: 4000, y: 100 });
  await settle(usePreviewStore.getState().setPlacementPreference("split"));
  await settle(usePreviewStore.getState().setPlacementPreference("window"));
  const third = (await WebviewWindow.getByLabel("preview"))!;
  expect(geometries.get(third)!.x).toBeLessThan(1920);
  expect(geometries.get(third)!.maximized).toBe(true);

  // Fullscreen cannot replace ordinary session bounds with its large rectangle.
  await usePreviewStore.getState().setFullscreen(true);
  Object.assign(geometries.get(third)!, { x: 0, y: 0, width: 1920, height: 1080 });
  await flushPreviewWindowPlacement();
  await settle(usePreviewStore.getState().setPlacementPreference("split"));
  await settle(usePreviewStore.getState().setPlacementPreference("window"));
  const fourth = (await WebviewWindow.getByLabel("preview"))!;
  expect(geometries.get(fourth)!.width).toBeLessThan(1920);
  expect(geometries.get(fourth)!.maximized).toBe(true);

  // After unplugging the extra screens, reopening floats on the remaining one.
  setMonitors([monitors[0]]);
  outerPosition.mockResolvedValue({ x: 100, y: 100 });
  await settle(usePreviewStore.getState().setPlacementPreference("split"));
  await settle(usePreviewStore.getState().setPlacementPreference("window"));
  const single = (await WebviewWindow.getByLabel("preview"))!;
  expect(geometries.get(single)!.maximized).toBe(false);
  expect(geometries.get(single)!.width).toBeLessThan(1920);
  expect(single.setAlwaysOnTop.mock.calls).toEqual([[true], [false]]);
  expect(setFocus).not.toHaveBeenCalled();
  expect(invokeCalls.some(({ command }) => command === "patch_state")).toBe(false);
  await settle(usePreviewStore.getState().setPlacementPreference("split"));
});
