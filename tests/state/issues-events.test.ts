// @vitest-environment happy-dom

import { beforeEach, describe, expect, it, vi } from "vitest";
import { useIssuesStore } from "../../src/state/issues-store";
import { installIssuesEventWiring } from "../../src/workflows/issues";
import {
  fireEvent,
  listenerCount,
  resetTauriMocks,
} from "../mocks/tauri";

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  useIssuesStore.setState({ open: false, loadRecent: vi.fn(async () => undefined) });
});

describe("issue history event ownership", () => {
  it("refreshes an open Recent view for published and history-only notices", async () => {
    await installIssuesEventWiring();
    const loadRecent = useIssuesStore.getState().loadRecent;

    fireEvent("notification://published");
    expect(loadRecent).not.toHaveBeenCalled();

    useIssuesStore.setState({ open: true });
    fireEvent("notification://published");
    fireEvent("notification://recorded");

    expect(loadRecent).toHaveBeenCalledTimes(2);
    expect(listenerCount("notification://published")).toBe(1);
    expect(listenerCount("notification://recorded")).toBe(1);
  });
});
