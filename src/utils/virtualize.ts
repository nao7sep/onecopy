// Windowed rendering for the grid (and the other-files list, which is the
// same composite at one column). At the developer's scale a month reaches
// 20,000–30,000 items; native lazy loading defers the IMAGE BYTES but still
// mounts every tile's DOM node, and tens of thousands of nodes make scroll
// and selection crawl. Only the rows near the viewport exist in the DOM; two
// spacer blocks keep the scrollbar honest.
//
// Pure, because the row arithmetic is exactly the kind of decision that hides
// in a component and rots: the keyboard's PageUp maths, the anchor-recovery
// scroll and this window must all agree on what a "row" is.

export interface VisibleWindow {
  /** First rendered row (inclusive). */
  startRow: number;
  /** One past the last rendered row. */
  endRow: number;
  /** Spacer heights that stand in for everything not rendered. */
  topPad: number;
  bottomPad: number;
}

export function visibleWindow(
  scrollTop: number,
  viewportHeight: number,
  rowHeight: number,
  totalRows: number,
  overscan = 2,
): VisibleWindow {
  if (totalRows <= 0 || rowHeight <= 0) {
    return { startRow: 0, endRow: 0, topPad: 0, bottomPad: 0 };
  }
  const first = Math.floor(scrollTop / rowHeight);
  const last = Math.ceil((scrollTop + Math.max(0, viewportHeight)) / rowHeight);
  const startRow = Math.max(0, first - overscan);
  const endRow = Math.min(totalRows, last + overscan);
  return {
    startRow,
    endRow,
    topPad: startRow * rowHeight,
    bottomPad: Math.max(0, (totalRows - endRow) * rowHeight),
  };
}

/** The scroll offset that centres `row` in the viewport — the anchor-recovery
 * fallback when the anchor's node is virtualized out and cannot be asked to
 * scrollIntoView. */
export function scrollTopForRow(
  row: number,
  viewportHeight: number,
  rowHeight: number,
): number {
  return Math.max(0, row * rowHeight - Math.max(0, viewportHeight - rowHeight) / 2);
}
