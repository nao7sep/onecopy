// Screen priority (the design's rule: 1 = main window, 2 = preview, 3+ join
// the comparison spread). The persisted order is app STATE — screen
// identifiers are machine-specific — as a list of monitor keys; monitors not
// in the list append last in native order, so a newly attached screen simply
// joins the tail.

export interface MonitorLike {
  name: string | null;
  position: { x: number; y: number };
}

/** A monitor's identity for the priority list.
 *
 * The POSITION is always part of it, never a fallback for a missing name. Two
 * displays of the same model report the same name — "#1287" twice is the
 * ordinary case for a matched pair — and a name-only key made them one entry:
 * reordering moved whichever the lookup found first, and React saw duplicate
 * keys in the list. What genuinely distinguishes two identical displays is
 * where they sit, so that is what identifies them.
 *
 * Consequence, accepted pre-release: a priority list persisted under the old
 * name-only keys no longer matches, so those monitors fall to the tail in
 * native order and the user reorders once. */
export function monitorKey(monitor: MonitorLike): string {
  return `${monitor.name ?? "display"}@${monitor.position.x},${monitor.position.y}`;
}

/** Where each monitor sits, in words, so a matched pair can be told apart.
 *
 * Two "#1287"s are indistinguishable by name and by resolution; their
 * arrangement is the only thing the user can map onto the desk in front of
 * them. Ordered left-to-right, then top-to-bottom for stacked displays. */
export function describePosition(
  monitor: MonitorLike,
  all: MonitorLike[],
): string {
  if (all.length < 2) return "";
  const xs = [...new Set(all.map((m) => m.position.x))].sort((a, b) => a - b);
  const ys = [...new Set(all.map((m) => m.position.y))].sort((a, b) => a - b);
  const column =
    xs.length < 2
      ? ""
      : monitor.position.x === xs[0]
        ? "left"
        : monitor.position.x === xs[xs.length - 1]
          ? "right"
          : "middle";
  const row =
    ys.length < 2
      ? ""
      : monitor.position.y === ys[0]
        ? "top"
        : monitor.position.y === ys[ys.length - 1]
          ? "bottom"
          : "centre";
  return [row, column].filter(Boolean).join(" ");
}

/** Stable-sorts monitors by their key's position in `priority`; unlisted
 * monitors keep native order after every listed one. */
export function orderMonitors<T extends MonitorLike>(monitors: T[], priority: string[]): T[] {
  const rank = (m: T) => {
    const index = priority.indexOf(monitorKey(m));
    return index === -1 ? priority.length : index;
  };
  return [...monitors].sort((a, b) => rank(a) - rank(b));
}

/** Reads the persisted priority list out of the app state document. */
export function priorityFromState(state: Record<string, unknown> | null | undefined): string[] {
  const value = state?.screenPriority;
  return Array.isArray(value) ? value.filter((v): v is string => typeof v === "string") : [];
}
