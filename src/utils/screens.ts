// Auxiliary display priority: a setting in config.json, kept as a list of
// monitor keys (machine-specific identifiers). Monitors not in the list append
// last in native order, so a newly attached screen simply joins the tail, and
// a display that is not connected keeps its place for when it returns. Preview
// and Comparison exclude Main's current display at use time.

import type { MessageKey } from "../i18n/catalogues";
import { message, type Message } from "../i18n/translate";

export interface MonitorLike {
  name: string | null;
  position: { x: number; y: number };
}

export interface MonitorRect extends MonitorLike {
  size: { width: number; height: number };
  scaleFactor?: number;
  workArea?: {
    position: { x: number; y: number };
    size: { width: number; height: number };
  };
}

/** A monitor's identity for the priority list.
 *
 * The POSITION is always part of it, never a fallback for a missing name. Two
 * displays of the same model report the same name — "#1287" twice is the
 * ordinary case for a matched pair — so a name alone would make them one
 * entry. What genuinely distinguishes two identical displays is where they
 * sit, so that is what identifies them. */
export function monitorKey(monitor: MonitorLike): string {
  return `${monitor.name ?? "display"}@${monitor.position.x},${monitor.position.y}`;
}

/** Where each monitor sits, in words, so a matched pair can be told apart.
 * Null when there is nothing to tell apart.
 *
 * Two "#1287"s are indistinguishable by name and by resolution; their
 * arrangement is the only thing the user can map onto the desk in front of
 * them. Ordered left-to-right, then top-to-bottom for stacked displays.
 *
 * Row and column are two independent facts, so a language that names them in
 * the other order, or joins them differently, reorders the wrapper's
 * placeholders instead of being handed a pre-joined English phrase. */
// With four or more displays on one axis, "left/centre/right" (or
// "top/middle/bottom") stops distinguishing every inner one — two displays
// side by side in the middle both read "centre" (D-S9). Past three distinct
// positions, an ordinal position replaces the word for that axis.
const ORDINAL_AXIS_THRESHOLD = 3;

function axisDescriptor(
  value: number,
  values: number[],
  low: MessageKey,
  high: MessageKey,
  mid: MessageKey,
): Message | null {
  if (values.length < 2) return null;
  if (values.length > ORDINAL_AXIS_THRESHOLD) {
    return message("settings.screenPosition", {
      index: values.indexOf(value) + 1,
      total: values.length,
    });
  }
  const key =
    value === values[0] ? low : value === values[values.length - 1] ? high : mid;
  return message(key);
}

export function describePosition(
  monitor: MonitorLike,
  all: MonitorLike[],
): Message | null {
  if (all.length < 2) return null;
  const xs = [...new Set(all.map((m) => m.position.x))].sort((a, b) => a - b);
  const ys = [...new Set(all.map((m) => m.position.y))].sort((a, b) => a - b);
  const column = axisDescriptor(
    monitor.position.x, xs, "settings.screenLeft", "settings.screenRight", "settings.screenCentre",
  );
  const row = axisDescriptor(
    monitor.position.y, ys, "settings.screenTop", "settings.screenBottom", "settings.screenMiddle",
  );
  if (row === null) return column;
  if (column === null) return row;
  return message("settings.screenRowColumn", { row, column });
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

/** The saved priority after swapping two connected displays' places. Displays
 * that are not connected keep their saved places, and connected ones not yet
 * in the list join after the saved ones. */
export function swappedPriority<T extends MonitorLike>(
  saved: string[],
  connected: T[],
  first: string,
  second: string,
): string[] | null {
  const visible = orderMonitors(connected, saved).map(monitorKey);
  const i = visible.indexOf(first);
  const j = visible.indexOf(second);
  if (i < 0 || j < 0 || i === j) return null;
  [visible[i], visible[j]] = [visible[j], visible[i]];
  const result: string[] = [];
  let next = 0;
  for (const key of saved) {
    result.push(visible.includes(key) ? visible[next++] : key);
  }
  while (next < visible.length) result.push(visible[next++]);
  return result;
}

/** Reads the saved priority list out of the effective configuration. */
export function priorityFromConfig(config: Record<string, unknown> | null | undefined): string[] {
  const value = config?.screenPriority;
  return Array.isArray(value) ? value.filter((v): v is string => typeof v === "string") : [];
}
