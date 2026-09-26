// Main's keyboard movement over one section, as a pure decision: which
// position a key moves the anchor to. PageUp/PageDown move by the rows one
// viewport holds, from the MEASURED row height the virtualizer lays out with,
// so a zoom level or a tile restyle cannot make a page differ from a screen.

export interface GridNavigation {
  key: string;
  layout: "tiles" | "list";
  columns: number;
  viewportHeight: number;
  rowHeight: number;
  /** The anchor's position, or -1 when nothing is selected. */
  current: number;
  total: number;
}

/** The rows one page moves: whole rows that fit the viewport, at least two. */
export function pageRows(viewportHeight: number, rowHeight: number): number {
  return Math.max(2, Math.floor(viewportHeight / Math.max(1, rowHeight)));
}

/** The target position for a navigation key (-1 in an empty section), or
 * null when the key is not a navigation key. */
export function gridNavigationTarget(navigation: GridNavigation): number | null {
  const { key, layout, columns, current, total } = navigation;
  const page = columns * pageRows(navigation.viewportHeight, navigation.rowHeight);
  const step =
    key === "ArrowRight" && layout === "tiles"
      ? 1
      : key === "ArrowLeft" && layout === "tiles"
        ? -1
        : key === "ArrowDown"
          ? columns
          : key === "ArrowUp"
            ? -columns
            : key === "PageDown"
              ? page
              : key === "PageUp"
                ? -page
                : key === "Home"
                  ? Number.NEGATIVE_INFINITY
                  : key === "End"
                    ? Number.POSITIVE_INFINITY
                    : null;
  if (step === null) return null;
  const target = Number.isFinite(step)
    ? Math.min(Math.max(current < 0 ? 0 : current + step, 0), total - 1)
    : step === Number.NEGATIVE_INFINITY
      ? 0
      : total - 1;
  return Math.max(target, -1);
}
