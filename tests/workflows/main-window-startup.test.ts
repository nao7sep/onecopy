// @vitest-environment happy-dom
import { expect, it, vi } from "vitest";
import { mockCommands, resetTauriMocks } from "../mocks/tauri";

const startup = vi.hoisted(() => ({
  quit: vi.fn<() => Promise<void>>(),
  render: vi.fn(),
}));
vi.mock("react-dom/client", () => ({ default: { createRoot: () => ({ render: startup.render }) } }));
vi.mock("../../src/workflows/quit", async (original) => ({
  ...await original<typeof import("../../src/workflows/quit")>(),
  installQuitWorkflow: startup.quit,
}));
vi.mock("../../src/media-use", async (original) => ({
  ...await original<typeof import("../../src/media-use")>(),
  installMediaUseBoundary: async () => {},
}));
vi.mock("../../src/workflows/window-appearance", () => ({ installWindowAppearance: async () => {} }));

it("does not mount Main before quit readiness settles", async () => {
  resetTauriMocks();
  mockCommands({ logging_debug_enabled: () => false, log_event: () => null });
  document.body.innerHTML = '<div id="root"></div>';
  let ready!: () => void;
  startup.quit.mockImplementation(() => new Promise((resolve) => { ready = resolve; }));
  await import("../../src/main");
  expect(startup.quit).toHaveBeenCalledOnce();
  expect(startup.render).not.toHaveBeenCalled();
  ready();
  await vi.waitFor(() => expect(startup.render).toHaveBeenCalledOnce());
});
