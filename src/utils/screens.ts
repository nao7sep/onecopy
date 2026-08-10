// Screen priority (the design's rule: 1 = main window, 2 = preview, 3+ join
// the comparison spread). The persisted order is app STATE — screen
// identifiers are machine-specific — as a list of monitor keys; monitors not
// in the list append last in native order, so a newly attached screen simply
// joins the tail.

export interface MonitorLike {
  name: string | null;
  position: { x: number; y: number };
}

export function monitorKey(monitor: MonitorLike): string {
  return monitor.name ?? `at-${monitor.position.x}x${monitor.position.y}`;
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
