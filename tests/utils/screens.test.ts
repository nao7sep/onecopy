import { describe, expect, it } from "vitest";
import {
  describePosition,
  monitorKey,
  orderMonitors,
  priorityFromState,
} from "../../src/utils/screens";

const m = (name: string | null, x: number, y = 0) => ({ name, position: { x, y } });

describe("screen priority ordering", () => {
  it("orders by the priority list, unlisted appending last in native order", () => {
    const monitors = [m("C", 2), m("A", 0), m("B", 1)];
    const ordered = orderMonitors(monitors, [monitorKey(m("B", 1)), monitorKey(m("A", 0))]);
    expect(ordered.map((x) => x.name)).toEqual(["B", "A", "C"]);
  });

  it("keeps native order entirely when no priority is set", () => {
    const monitors = [m("C", 2), m("A", 0)];
    expect(orderMonitors(monitors, []).map((x) => x.name)).toEqual(["C", "A"]);
  });

  it("gives two displays of the SAME model distinct keys", () => {
    // The matched-pair case, and the reason position is always part of the
    // key. A name-only key made both "#1287"s one entry: reordering moved
    // whichever the lookup hit first, and the list rendered duplicate keys.
    const left = m("#1287", 0);
    const right = m("#1287", 2560);
    expect(monitorKey(left)).not.toBe(monitorKey(right));

    // And the priority list can then actually address one of them.
    const ordered = orderMonitors([left, right], [monitorKey(right)]);
    expect(ordered[0].position.x).toBe(2560);
  });

  it("reads only string lists out of state", () => {
    expect(priorityFromState({ screenPriority: ["A", 1, "B"] })).toEqual(["A", "B"]);
    expect(priorityFromState({})).toEqual([]);
    expect(priorityFromState(null)).toEqual([]);
  });
});

describe("describing where a monitor sits", () => {
  it("names the sides of a side-by-side pair", () => {
    const all = [m("#1287", 0), m("#1287", 2560)];
    expect(describePosition(all[0], all)).toBe("left");
    expect(describePosition(all[1], all)).toBe("right");
  });

  it("names rows when displays are stacked", () => {
    const all = [m("#1287", 0, 0), m("#1287", 0, 1440)];
    expect(describePosition(all[0], all)).toBe("top");
    expect(describePosition(all[1], all)).toBe("bottom");
  });

  it("combines both axes on a grid", () => {
    const all = [m("a", 0, 0), m("b", 2560, 0), m("c", 0, 1440)];
    expect(describePosition(all[1], all)).toBe("top right");
    expect(describePosition(all[2], all)).toBe("bottom left");
  });

  it("says nothing when there is only one screen", () => {
    const all = [m("#1287", 0)];
    expect(describePosition(all[0], all)).toBe("");
  });
});
