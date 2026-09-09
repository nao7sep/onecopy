import { beforeEach, describe, expect, it } from "vitest";
import { rescanCurrentSection } from "../../src/workflows/items";
import { useItemsStore } from "../../src/state/items-store";
import { currentMainFeedback, useMainFeedbackStore } from "../../src/state/main-feedback-store";
import { mockCommands, resetTauriMocks } from "../mocks/tauri";

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  useMainFeedbackStore.setState({ entries: {} });
  mockCommands({
    log_event: () => null,
    get_issues: () => ({ total: 0, rows: [] }),
  });
  useItemsStore.setState({
    selected: { kind: "image", month: "2026-01" },
  });
});

describe("section repair outcome", () => {
  it("leaves intentional cancellation to the resumable-index warning", async () => {
    mockCommands({ rescan_section: () => ({ status: "cancelled" }) });

    await rescanCurrentSection();

    expect(currentMainFeedback(useMainFeedbackStore.getState())).toBeNull();
  });

  it("keeps an unexpected repair failure visible", async () => {
    mockCommands({
      rescan_section: () =>
        Promise.reject(
          new Error("Error invoking remote method: EACCES /private/tmp/HOSTILE-SENTINEL"),
        ),
    });

    await rescanCurrentSection();

    expect(currentMainFeedback(useMainFeedbackStore.getState())).toMatchObject({
      tone: "danger", text: "This section could not be refreshed. Try again.",
    });
    expect(currentMainFeedback(useMainFeedbackStore.getState())?.text).not.toContain("HOSTILE-SENTINEL");
  });
});
