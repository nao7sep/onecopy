import { beforeEach, describe, expect, it } from "vitest";
import { rescanCurrentSection } from "../../src/workflows/items";
import { useItemsStore } from "../../src/state/items-store";
import { mockCommands, resetTauriMocks } from "../mocks/tauri";

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  mockCommands({
    log_event: () => null,
    get_issues: () => ({ total: 0, rows: [] }),
  });
  useItemsStore.setState({
    selected: { kind: "image", month: "2026-01" },
    message: null,
  });
});

describe("section repair outcome", () => {
  it("leaves intentional cancellation to the resumable-index warning", async () => {
    mockCommands({ rescan_section: () => Promise.reject(new Error("scan cancelled")) });

    await rescanCurrentSection();

    expect(useItemsStore.getState().message).toBeNull();
  });

  it("keeps an unexpected repair failure visible", async () => {
    mockCommands({ rescan_section: () => Promise.reject(new Error("directory unreadable")) });

    await rescanCurrentSection();

    expect(useItemsStore.getState().message).toContain("directory unreadable");
  });
});
