// Main-window bounds persistence: the pure half.
//
// The window used to open at the config's fixed size wherever the OS dropped
// it, every launch. Bounds are app-level STATE like zoom (state.json,
// never config), saved debounced on move/resize and restored at boot BEFORE
// the window is first shown — together with the hidden-at-creation window this
// removes both the white startup flash and the restore jump.
//
// Everything here is physical pixels: monitors report physical, and a
// logical round-trip through two monitors of different scale factors is
// exactly the bug class this avoids.

/** Physical outer position + physical inner size, as saved in state.json. */
export interface SavedBounds {
  x: number;
  y: number;
  width: number;
  height: number;
}

interface MonitorRect {
  position: { x: number; y: number };
  size: { width: number; height: number };
  workArea?: {
    position: { x: number; y: number };
    size: { width: number; height: number };
  };
}

function usableArea(monitor: MonitorRect) {
  return monitor.workArea ?? { position: monitor.position, size: monitor.size };
}

/** Parses the untyped state.json value. Anything malformed — missing field,
 * non-finite number, non-positive size — is a clean "nothing saved", never a
 * throw: state.json is machine-written but survives hand edits and version
 * skew, and a corrupt entry must cost the default placement, not the boot. */
export function parseSavedBounds(value: unknown): SavedBounds | null {
  if (typeof value !== "object" || value === null) return null;
  const record = value as Record<string, unknown>;
  const numbers = [record.x, record.y, record.width, record.height];
  if (!numbers.every((n) => typeof n === "number" && Number.isFinite(n))) {
    return null;
  }
  const [x, y, width, height] = numbers as number[];
  if (width < 1 || height < 1) return null;
  return { x, y, width, height };
}

/** Whether saved bounds are still usable on the CURRENT monitor set.
 *
 * The failure this exists for: the window was last used on a monitor that is
 * no longer attached (the developer's machines swap between one and three
 * screens), and restoring it verbatim puts the title bar somewhere no mouse
 * can reach. Usable means some monitor shows a grabbable piece of the
 * window's TOP strip — at least 100×50 of it, including part of the first
 * 50 rows, which is where every OS puts the drag handle. Anything less
 * returns null and the boot keeps the OS default placement. A reachable saved
 * window is fitted back inside that monitor's work area, so scale changes and
 * taskbars cannot restore an oversized or partly stranded normal window. */
export function restorableBounds(
  saved: SavedBounds | null,
  monitors: MonitorRect[],
): SavedBounds | null {
  if (saved === null) return null;
  for (const monitor of monitors) {
    const area = usableArea(monitor);
    const left = Math.max(saved.x, area.position.x);
    const top = Math.max(saved.y, area.position.y);
    const right = Math.min(saved.x + saved.width, area.position.x + area.size.width);
    const bottom = Math.min(saved.y + saved.height, area.position.y + area.size.height);
    const overlapsTopStrip = top < saved.y + 50;
    if (right - left >= 100 && bottom - top >= 50 && overlapsTopStrip) {
      const fitted = shrinkToFit(saved, area.size) ?? saved;
      return {
        x: Math.min(
          Math.max(saved.x, area.position.x),
          area.position.x + area.size.width - fitted.width,
        ),
        y: Math.min(
          Math.max(saved.y, area.position.y),
          area.position.y + area.size.height - fitted.height,
        ),
        width: fitted.width,
        height: fitted.height,
      };
    }
  }
  return null;
}

/** First-launch or restore fit: the requested inner size may overflow a small
 * laptop's work area. Returns the size to shrink to, or null when the window
 * already fits. 90% of the monitor rather than 100%: an exactly-screen-sized
 * floating window reads as a broken maximize. */
export function shrinkToFit(
  inner: { width: number; height: number },
  monitor: { width: number; height: number },
): { width: number; height: number } | null {
  if (inner.width <= monitor.width && inner.height <= monitor.height) return null;
  return {
    width: Math.min(inner.width, Math.round(monitor.width * 0.9)),
    height: Math.min(inner.height, Math.round(monitor.height * 0.9)),
  };
}
