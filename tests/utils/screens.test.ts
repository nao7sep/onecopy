import { describe, expect, it } from "vitest";
import {
  describePosition,
  monitorKey,
  orderMonitors,
  priorityFromConfig,
  swappedPriority,
} from "../../src/utils/screens";
import { inEnglish } from "../helpers/i18n";

const m = (name: string | null, x: number, y = 0) => ({ name, position: { x, y } });

describe("screen priority ordering", () => {
  it("orders by the priority list, unlisted appending last in native order", () => {
    const monitors = [m("C", 2), m("A", 0), m("B", 1)];
    const ordered = orderMonitors(monitors, [monitorKey(m("B", 1)), monitorKey(m("A", 0))]);
    expect(ordered.map((x) => x.name)).toEqual(["B", "A", "C"]);
  });

  // R5.5 C9: an entry for a display that is not currently connected is
  // simply skipped -- it neither crashes the sort nor produces a phantom
  // row, and the remaining, connected entries still rank correctly.
  it("ignores a priority entry for a display that is not connected", () => {
    const monitors = [m("C", 2), m("A", 0), m("B", 1)];
    const ordered = orderMonitors(monitors, [
      monitorKey(m("Absent", 99)),
      monitorKey(m("B", 1)),
      monitorKey(m("A", 0)),
    ]);
    expect(ordered.map((x) => x.name)).toEqual(["B", "A", "C"]);
    expect(ordered).toHaveLength(3);
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

  it("reads only string lists out of the settings", () => {
    expect(priorityFromConfig({ screenPriority: ["A", 1, "B"] })).toEqual(["A", "B"]);
    expect(priorityFromConfig({})).toEqual([]);
    expect(priorityFromConfig(null)).toEqual([]);
  });
});

describe("describing where a monitor sits", () => {
  it("names the sides of a side-by-side pair", () => {
    const all = [m("#1287", 0), m("#1287", 2560)];
    expect(inEnglish(describePosition(all[0], all))).toBe("left");
    expect(inEnglish(describePosition(all[1], all))).toBe("right");
  });

  it("names rows when displays are stacked", () => {
    const all = [m("#1287", 0, 0), m("#1287", 0, 1440)];
    expect(inEnglish(describePosition(all[0], all))).toBe("top");
    expect(inEnglish(describePosition(all[1], all))).toBe("bottom");
  });

  it("combines both axes on a grid", () => {
    const all = [m("a", 0, 0), m("b", 2560, 0), m("c", 0, 1440)];
    expect(inEnglish(describePosition(all[1], all))).toBe("top right");
    expect(inEnglish(describePosition(all[2], all))).toBe("bottom left");
  });

  it("says nothing when there is only one screen", () => {
    const all = [m("#1287", 0)];
    expect(describePosition(all[0], all)).toBeNull();
  });

  it("gives every display in a row of four or more a distinct ordinal position (D-S9)", () => {
    // "left/centre/right" collapses to two indistinguishable "centre" rows
    // once there are two inner displays; an ordinal keeps every row unique.
    const all = [m("a", 0), m("b", 1920), m("c", 3840), m("d", 5760)];
    expect(inEnglish(describePosition(all[0], all))).toBe("position 1 of 4");
    expect(inEnglish(describePosition(all[1], all))).toBe("position 2 of 4");
    expect(inEnglish(describePosition(all[2], all))).toBe("position 3 of 4");
    expect(inEnglish(describePosition(all[3], all))).toBe("position 4 of 4");
  });
});

describe("swappedPriority", () => {
  const at = (name: string, x: number) => ({ name, position: { x, y: 0 } });
  const left = at("L", 0);
  const right = at("R", 100);
  const office = "Office@200,0";

  it("swaps two connected displays and keeps a disconnected one's place", () => {
    const saved = [monitorKey(left), office, monitorKey(right)];
    expect(swappedPriority(saved, [left, right], monitorKey(left), monitorKey(right)))
      .toEqual([monitorKey(right), office, monitorKey(left)]);
  });

  it("adds connected displays not yet saved after the saved ones", () => {
    expect(swappedPriority([office], [left, right], monitorKey(left), monitorKey(right)))
      .toEqual([office, monitorKey(right), monitorKey(left)]);
  });

  it("builds each swap on the order as saved, so quick moves each count", () => {
    const third = at("T", 300);
    const once = swappedPriority([], [left, right, third], monitorKey(left), monitorKey(right))!;
    const twice = swappedPriority(once, [left, right, third], monitorKey(left), monitorKey(third))!;
    expect(twice).toEqual([monitorKey(right), monitorKey(third), monitorKey(left)]);
  });
});
