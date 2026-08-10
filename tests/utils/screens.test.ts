import { describe, expect, it } from "vitest";
import { monitorKey, orderMonitors, priorityFromState } from "../../src/utils/screens";

const m = (name: string | null, x: number) => ({ name, position: { x, y: 0 } });

describe("screen priority ordering", () => {
  it("orders by the priority list, unlisted appending last in native order", () => {
    const monitors = [m("C", 2), m("A", 0), m("B", 1)];
    const ordered = orderMonitors(monitors, ["B", "A"]);
    expect(ordered.map((x) => x.name)).toEqual(["B", "A", "C"]);
  });

  it("keeps native order entirely when no priority is set", () => {
    const monitors = [m("C", 2), m("A", 0)];
    expect(orderMonitors(monitors, []).map((x) => x.name)).toEqual(["C", "A"]);
  });

  it("falls back to a position key for unnamed monitors", () => {
    expect(monitorKey(m(null, 1920))).toBe("at-1920x0");
  });

  it("reads only string lists out of state", () => {
    expect(priorityFromState({ screenPriority: ["A", 1, "B"] })).toEqual(["A", "B"]);
    expect(priorityFromState({})).toEqual([]);
    expect(priorityFromState(null)).toEqual([]);
  });
});
