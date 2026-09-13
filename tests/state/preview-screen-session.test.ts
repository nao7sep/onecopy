import { afterEach, expect, it, vi } from "vitest";
import { usePreviewStore } from "../../src/state/preview-store";
import {
  createdWindows,
  invokeCalls,
  mockCommands,
  outerPosition,
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

it("allocates before showing and persists Preview placement across openings", async () => {
  vi.useFakeTimers();
  resetTauriMocks();
  mockCommands({
    set_window_fullscreen: () => null,
    log_event: () => null,
    record_recent_notification: () => null,
    patch_state: ({ patch }) => patch,
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
  const geometries = new Map<
    WebviewWindow,
    { x: number; y: number; width: number; height: number; maximized: boolean }
  >();
  setWindowCreatedHook((label) => {
    void WebviewWindow.getByLabel(label).then((window) => {
      if (!window) throw new Error("Missing constructed window");
      const geometry = { x: 0, y: 30, width: 1280, height: 800, maximized: false };
      geometries.set(window, geometry);
      // Real Tauri geometry instances carry this enumerable discriminator.
      window.outerPosition.mockImplementation(async () => ({
        x: geometry.x,
        y: geometry.y,
        type: "Physical",
      }));
      window.outerSize.mockImplementation(async () => ({
        width: geometry.width,
        height: geometry.height,
        type: "Physical",
      }));
      window.setPosition.mockImplementation(async (position) => {
        geometry.x = position.x;
        geometry.y = position.y;
      });
      window.setSize.mockImplementation(async (size) => {
        geometry.width = size.width;
        geometry.height = size.height;
      });
      window.maximize.mockImplementation(async () => { geometry.maximized = true; });
      window.unmaximize.mockImplementation(async () => { geometry.maximized = false; });
      window.isMaximized.mockImplementation(async () => geometry.maximized);
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
  const firstGeometry = geometries.get(first)!;
  expect(firstGeometry.maximized).toBe(true);
  expect(firstGeometry.x).toBeGreaterThanOrEqual(1920);
  expect(first.setPosition.mock.invocationCallOrder[0])
    .toBeLessThan(first.maximize.mock.invocationCallOrder[0]);
  expect(first.maximize.mock.invocationCallOrder[0])
    .toBeLessThan(first.show.mock.invocationCallOrder[0]);
  expect(createdWindows[0].options).toMatchObject({ visible: false, focus: false });
  expect(setFocus).not.toHaveBeenCalled();

  // A Mac edge-tiled outer frame survives Preview off/on, fitted to the work area.
  Object.assign(firstGeometry, { x: 1919, y: 0, width: 961, height: 1080, maximized: false });
  usePreviewStore.getState().close();
  await settle(open());
  const second = (await WebviewWindow.getByLabel("preview"))!;
  expect(geometries.get(second)).toEqual({
    x: 1920,
    y: 30,
    width: 961,
    height: 1050,
    maximized: false,
  });

  // Work-area-sized geometry is maximized even when the Mac backend reports false;
  // reopening restores that mode while retaining the normal half-screen rectangle.
  Object.assign(geometries.get(second)!, {
    x: 1920, y: 30, width: 1920, height: 1050, maximized: false,
  });
  usePreviewStore.getState().close();
  await settle(open());
  const maximized = (await WebviewWindow.getByLabel("preview"))!;
  expect(geometries.get(maximized)).toEqual({
    x: 1920,
    y: 30,
    width: 961,
    height: 1050,
    maximized: true,
  });

  // Moving Main onto that screen does not override the user's Preview choice.
  outerPosition.mockResolvedValue({ x: 2000, y: 100 });
  await settle(usePreviewStore.getState().setPlacementPreference("split"));
  await settle(usePreviewStore.getState().setPlacementPreference("window"));
  const third = (await WebviewWindow.getByLabel("preview"))!;
  expect(geometries.get(third)!.x).toBeGreaterThanOrEqual(1920);
  expect(geometries.get(third)!.maximized).toBe(true);

  await vi.advanceTimersByTimeAsync(1_000);
  const writes = invokeCalls.filter(({ command }) => command === "patch_state");
  expect(writes.length).toBeGreaterThan(0);
  expect(writes.at(-1)?.args.patch).toMatchObject({
    previewWindowPlacement: {
      screen: `Screen 1@1920,0`,
      normalBounds: { x: 1920, y: 30, width: 961, height: 1050 },
      mode: "maximized",
    },
  });
  expect(writes.at(-1)?.args.patch)
    .not.toHaveProperty("previewWindowPlacement.normalBounds.type");
  await settle(usePreviewStore.getState().setPlacementPreference("split"));
});
