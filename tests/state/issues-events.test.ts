// @vitest-environment happy-dom

import { beforeEach, describe, expect, it, vi } from "vitest";
import { useIssuesStore } from "../../src/state/issues-store";
import { installIssuesEventWiring } from "../../src/workflows/issues";
import { useAppShellStore } from "../../src/state/app-shell-store";
import {
  fireEvent,
  listenerCount,
  resetTauriMocks,
} from "../mocks/tauri";

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  useIssuesStore.setState({ load: vi.fn(async () => undefined) });
  useAppShellStore.setState({ utilitySurface: null });
});

describe("issue event ownership", () => {
  it("refreshes the status count when closed and the inbox when open", async () => {
    await installIssuesEventWiring();
    const load = useIssuesStore.getState().load;

    fireEvent("notification://published");
    expect(load).toHaveBeenCalledTimes(1);

    useAppShellStore.getState().openUtility("issues");
    fireEvent("notification://published");
    fireEvent("notification://recorded");

    expect(load).toHaveBeenCalledTimes(3);
    expect(listenerCount("notification://published")).toBe(1);
    expect(listenerCount("notification://recorded")).toBe(1);
  });
});
