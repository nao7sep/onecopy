import { afterEach, describe, expect, it, vi } from "vitest";
import { formatLocalMinute } from "../../src/utils/displayTime";
import { formatActivityTime } from "../../src/models/activity-history";

afterEach(() => vi.unstubAllEnvs());

describe.each([
  ["UTC", "2026-01-31 20:00"],
  ["Asia/Tokyo", "2026-02-01 05:00"],
  ["Asia/Kathmandu", "2026-02-01 01:45"],
  ["America/New_York", "2026-01-31 15:00"],
])("local display in %s", (zone, expected) => {
  it("renders serialized and epoch instants in the same local calendar", () => {
    vi.stubEnv("TZ", zone);
    const instant = "2026-01-31T20:00:12.345Z";
    expect(formatLocalMinute(instant)).toBe(expected);
    expect(formatLocalMinute(Date.parse(instant))).toBe(expected);
    expect(formatActivityTime(instant)).toBe(`${expected}:12.345`);
  });
});

it("uses the offset at the displayed instant, not today's offset", () => {
  vi.stubEnv("TZ", "America/New_York");
  expect(formatLocalMinute("2024-03-10T06:59:00.000Z")).toBe("2024-03-10 01:59");
  expect(formatLocalMinute("2024-03-10T07:00:00.000Z")).toBe("2024-03-10 03:00");
  expect(formatLocalMinute("2024-11-03T05:30:00.000Z")).toBe("2024-11-03 01:30");
  expect(formatLocalMinute("2024-11-03T06:30:00.000Z")).toBe("2024-11-03 01:30");
});
