import { describe, expect, it } from "vitest";
import {
  invalidIgnoredFileName,
  isFileName,
  trimIgnoredFileName,
} from "../../src/models/ignoredFileNames";
// The core's rule is held to the same cases
// (visibility_tests::ignored_file_names_follow_the_shared_cases).
import cases from "../fixtures/ignored-file-names.json";

describe("an ignored file name row", () => {
  it("follows the core's rule for every shared case", () => {
    for (const name of cases.fileNames) {
      expect(isFileName(name), JSON.stringify(name)).toBe(true);
      expect(invalidIgnoredFileName(name)).toBeNull();
    }
    for (const name of cases.notFileNames) {
      expect(isFileName(name), JSON.stringify(name)).toBe(false);
      expect(invalidIgnoredFileName(name)).toBe(trimIgnoredFileName(name));
    }
    for (const [row, name] of Object.entries(cases.trimmed)) {
      expect(trimIgnoredFileName(row), JSON.stringify(row)).toBe(name);
    }
    for (const row of cases.blank) {
      expect(trimIgnoredFileName(row), JSON.stringify(row)).toBe("");
      expect(invalidIgnoredFileName(row)).toBeNull();
    }
  });

  it("names the trimmed item it marks", () => {
    expect(invalidIgnoredFileName("  photos/raw.jpg ")).toBe("photos/raw.jpg");
  });
});
