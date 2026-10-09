import { describe, expect, it } from "vitest";
import { mutationResultLine, type MutationResult } from "../../src/models/mutation";
import { createTranslator } from "../../src/i18n/translate";

const t = createTranslator("en").t;

function deleteResult(filesCompleted: number, trashAvailable: boolean): MutationResult {
  return {
    operationId: 1,
    kind: "delete",
    cancelled: false,
    summary: {
      itemsCompleted: filesCompleted > 0 ? 2 : 0,
      itemsPartial: 0,
      itemsUnstarted: 0,
      filesCompleted,
      filesFailed: 0,
      filesUnknown: 0,
      filesUnstarted: 0,
      trashAvailable,
      error: null,
    },
  };
}

describe("delete receipt", () => {
  it("says the files can be restored from Deleted files after a recoverable delete", () => {
    expect(mutationResultLine(deleteResult(3, true), t)).toBe(
      "Deletion complete — 2 completed · recoverable from Deleted files",
    );
  });

  it("says the files are gone after a permanent delete", () => {
    expect(mutationResultLine(deleteResult(3, false), t)).toBe(
      "Deletion complete — 2 completed · permanently deleted",
    );
  });

  it("says nothing about where files went when none were removed", () => {
    expect(mutationResultLine(deleteResult(0, false), t)).toBe("Deletion complete — 0 completed");
  });
});
