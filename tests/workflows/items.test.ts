// @vitest-environment happy-dom
//
// Failures are recorded through the notifications store, which renders the
// sentence in the language the document declares, so this spec needs a document
// even though the subject is not the interface.

import { beforeEach, describe, expect, it } from "vitest";
import { rescanCurrentSection } from "../../src/workflows/items";
import { useItemsStore } from "../../src/state/items-store";
import { currentMainFeedback, useMainFeedbackStore } from "../../src/state/main-feedback-store";
import { mockCommands, resetTauriMocks } from "../mocks/tauri";
import { inEnglish } from "../helpers/i18n";

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

    const feedback = currentMainFeedback(useMainFeedbackStore.getState());
    expect(feedback?.tone).toBe("danger");
    expect(inEnglish(feedback?.text)).toBe("This section could not be refreshed. Try again.");
    expect(inEnglish(feedback?.text)).not.toContain("HOSTILE-SENTINEL");
  });
});
