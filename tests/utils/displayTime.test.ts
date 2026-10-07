import { afterEach, describe, expect, it, vi } from "vitest";
import { formatLocalMinute } from "../../src/utils/displayTime";
import { createEnglish } from "../helpers/i18n";

afterEach(() => vi.unstubAllEnvs());

// A displayed instant now carries the interface language's own date and time
// format, so the assertions read en-US. Unicode spaces are folded to plain ones
// because which space ICU puts before AM/PM is not what these specs are about.
function shown(input: string | number): string {
  return formatLocalMinute(input, createEnglish().dateTime).replace(/[  ]/g, " ");
}

describe.each([
  ["UTC", "Jan 31, 2026, 8:00 PM"],
  ["Asia/Tokyo", "Feb 1, 2026, 5:00 AM"],
  ["Asia/Kathmandu", "Feb 1, 2026, 1:45 AM"],
  ["America/New_York", "Jan 31, 2026, 3:00 PM"],
])("local display in %s", (zone, expected) => {
  it("renders serialized and epoch instants in the same local calendar", () => {
    vi.stubEnv("TZ", zone);
    const instant = "2026-01-31T20:00:12.345Z";
    expect(shown(instant)).toBe(expected);
    expect(shown(Date.parse(instant))).toBe(expected);
  });
});

it("uses the offset at the displayed instant, not today's offset", () => {
  vi.stubEnv("TZ", "America/New_York");
  expect(shown("2024-03-10T06:59:00.000Z")).toBe("Mar 10, 2024, 1:59 AM");
  expect(shown("2024-03-10T07:00:00.000Z")).toBe("Mar 10, 2024, 3:00 AM");
  expect(shown("2024-11-03T05:30:00.000Z")).toBe("Nov 3, 2024, 1:30 AM");
  expect(shown("2024-11-03T06:30:00.000Z")).toBe("Nov 3, 2024, 1:30 AM");
});

it("shows input it cannot read as it arrived", () => {
  expect(shown("not a date")).toBe("not a date");
});
