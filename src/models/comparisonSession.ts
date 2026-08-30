export interface ComparisonMember {
  hash: string;
  fileName: string;
  width: number | null;
  height: number | null;
  byteSize: number | null;
  sharpness: number | null;
  faceScore: number | null;
  copyCount: number;
  hasThumb: boolean;
}

export const COMPARISON_DIRECT_KEYS = [
  "0",
  "1",
  "2",
  "3",
  "4",
  "5",
  "6",
  "7",
  "8",
  "9",
  ..."abcdefghijklmnopqrstuvwxyz",
] as const;

export interface ComparisonPage {
  members: ComparisonMember[];
  portraitDominant: boolean;
  perDisplay: number;
}

export interface ComparisonSelection {
  selected: Set<string>;
  anchors: Set<string>;
  anchor: string | null;
  rangeOrigin: string | null;
  rangeBase: Set<string>;
}

export interface ComparisonGrid {
  count: number;
  columns: number;
  rows: number;
}

export type ComparisonSelectionMode = "exclusive" | "toggle" | "range";

export function directKeyIndex(event: {
  key: string;
  repeat?: boolean;
  shiftKey?: boolean;
  metaKey?: boolean;
  ctrlKey?: boolean;
  altKey?: boolean;
}): number {
  if (
    event.repeat === true ||
    event.shiftKey === true ||
    event.metaKey === true ||
    event.ctrlKey === true ||
    event.altKey === true
  ) {
    return -1;
  }
  return (COMPARISON_DIRECT_KEYS as readonly string[]).indexOf(
    event.key.toLowerCase(),
  );
}

export function dominantPortrait(members: ComparisonMember[]): boolean {
  const portrait = members.filter(
    (member) =>
      member.width !== null &&
      member.height !== null &&
      member.height > member.width,
  ).length;
  const landscape = members.filter(
    (member) =>
      member.width !== null &&
      member.height !== null &&
      member.width >= member.height,
  ).length;
  return portrait > landscape;
}

/**
 * Builds variable-size pages without limiting the group itself. A page first
 * considers everything the displays could show in landscape layout; its own
 * dominant orientation then chooses the ordinary three- or four-card display
 * capacity. This makes every boundary deterministic without a circular
 * "page shape decides page size decides page shape" dependency.
 */
export function comparisonPages(
  members: ComparisonMember[],
  configuredMaximum: number,
  displayCount: number,
): ComparisonPage[] {
  const maximum = Math.max(2, Math.floor(configuredMaximum));
  const displays = Math.max(1, Math.floor(displayCount));
  const landscapeCapacity = Math.min(maximum, displays * 4);
  const pages: ComparisonPage[] = [];
  let offset = 0;
  while (offset < members.length) {
    const candidates = members.slice(offset, offset + landscapeCapacity);
    const portraitDominant = dominantPortrait(candidates);
    const perDisplay = portraitDominant ? 3 : 4;
    const pageSize = Math.min(maximum, displays * perDisplay);
    pages.push({
      members: members.slice(offset, offset + pageSize),
      portraitDominant,
      perDisplay,
    });
    offset += pageSize;
  }
  return pages;
}

export function activePage(
  members: ComparisonMember[],
  page: number,
  configuredMaximum: number,
  displayCount: number,
): ComparisonPage {
  const pages = comparisonPages(members, configuredMaximum, displayCount);
  return (
    pages[Math.min(Math.max(0, page), Math.max(0, pages.length - 1))] ?? {
      members: [],
      portraitDominant: false,
      perDisplay: 4,
    }
  );
}

export function activeSelection(
  selected: Set<string>,
  members: ComparisonMember[],
): Set<string> {
  const visible = new Set(members.map((member) => member.hash));
  return new Set([...selected].filter((hash) => visible.has(hash)));
}

export function activateSelection(
  selection: ComparisonSelection,
  members: ComparisonMember[],
  preferredSelected: Iterable<string> = [],
  preferredAnchor: string | null = null,
): ComparisonSelection {
  const hashes = members.map((member) => member.hash);
  const visible = new Set(hashes);
  const selected = new Set(
    [...selection.selected].filter((hash) => visible.has(hash)),
  );
  if (selected.size === 0) {
    for (const hash of preferredSelected) {
      if (visible.has(hash)) selected.add(hash);
    }
  }
  if (selected.size === 0) {
    if (preferredAnchor !== null && visible.has(preferredAnchor)) {
      selected.add(preferredAnchor);
    }
  }

  let anchor =
    preferredAnchor !== null && selected.has(preferredAnchor)
      ? preferredAnchor
      : (hashes.find(
          (hash) => selection.anchors.has(hash) && selected.has(hash),
        ) ??
        hashes.find((hash) => selected.has(hash)) ??
        null);
  const allSelected = new Set(selection.selected);
  for (const hash of hashes) allSelected.delete(hash);
  for (const hash of selected) allSelected.add(hash);

  const anchors = new Set(selection.anchors);
  for (const hash of hashes) anchors.delete(hash);
  if (anchor !== null) anchors.add(anchor);

  return {
    selected: allSelected,
    anchors,
    anchor,
    rangeOrigin: anchor,
    rangeBase: selected,
  };
}

export function updateSelection(
  selection: ComparisonSelection,
  members: ComparisonMember[],
  target: string,
  mode: ComparisonSelectionMode,
): ComparisonSelection {
  const hashes = members.map((member) => member.hash);
  const targetIndex = hashes.indexOf(target);
  if (targetIndex < 0) return selection;

  const current = activeSelection(selection.selected, members);
  let selected: Set<string>;
  let anchor = target;
  let rangeOrigin = target;
  let rangeBase: Set<string>;

  if (mode === "toggle") {
    selected = new Set(current);
    if (selected.has(target)) {
      selected.delete(target);
      if (selection.anchor === target) {
        anchor = [...selected].pop() ?? "";
      } else {
        anchor = selection.anchor ?? "";
      }
    } else {
      selected.add(target);
    }
    rangeOrigin = anchor || target;
    rangeBase = new Set(selected);
  } else if (mode === "range") {
    const origin = selection.rangeOrigin ?? selection.anchor;
    const originIndex = origin === null ? -1 : hashes.indexOf(origin);
    if (originIndex < 0) {
      selected = new Set([target]);
      rangeOrigin = target;
      rangeBase = new Set([target]);
    } else {
      const [start, end] =
        originIndex <= targetIndex
          ? [originIndex, targetIndex]
          : [targetIndex, originIndex];
      selected = new Set(selection.rangeBase);
      for (const hash of hashes.slice(start, end + 1)) selected.add(hash);
      rangeOrigin = origin ?? target;
      rangeBase = new Set(selection.rangeBase);
    }
  } else {
    selected = new Set([target]);
    rangeBase = new Set([target]);
  }

  if (selected.size === 0) anchor = "";
  const allSelected = new Set(selection.selected);
  for (const hash of hashes) allSelected.delete(hash);
  for (const hash of selected) allSelected.add(hash);
  const anchors = new Set(selection.anchors);
  for (const hash of hashes) anchors.delete(hash);
  if (anchor !== "") anchors.add(anchor);

  return {
    selected: allSelected,
    anchors,
    anchor: anchor || null,
    rangeOrigin: selected.size === 0 ? null : rangeOrigin,
    rangeBase: selected.size === 0 ? new Set() : rangeBase,
  };
}

export function displayCapacities(
  memberCount: number,
  perDisplay: number,
  displayCount: number,
): number[] {
  if (memberCount <= 0) return [perDisplay];
  const used = Math.min(
    Math.max(1, displayCount),
    Math.ceil(memberCount / Math.max(1, perDisplay)),
  );
  return Array.from({ length: used }, () => perDisplay);
}

export function chunkMembers<T>(members: T[], capacities: number[]): T[][] {
  const chunks: T[][] = [];
  let offset = 0;
  for (const capacity of capacities) {
    chunks.push(members.slice(offset, offset + capacity));
    offset += capacity;
  }
  return chunks;
}

/** Number of columns while DOM order flows top-to-bottom, then left-to-right. */
export function gridFor(
  count: number,
  portraitDominant: boolean,
  containerAspect = 16 / 9,
): ComparisonGrid {
  if (count <= 1) return { count, columns: 1, rows: 1 };
  const safeAspect =
    Number.isFinite(containerAspect) && containerAspect > 0
      ? containerAspect
      : 16 / 9;
  const imageAspect = portraitDominant ? 2 / 3 : 3 / 2;
  let best = { columns: 1, rows: count, score: Number.POSITIVE_INFINITY };
  for (let columns = 1; columns <= count; columns += 1) {
    const rows = Math.ceil(count / columns);
    const cellAspect = (safeAspect * rows) / columns;
    const emptyCells = columns * rows - count;
    const score = Math.abs(Math.log(cellAspect / imageAspect)) + emptyCells * 0.15;
    if (score < best.score) best = { columns, rows, score };
  }
  return { count, columns: best.columns, rows: best.rows };
}

export function spatialTarget(
  currentIndex: number,
  direction: "left" | "right" | "up" | "down",
  chunkSizes: number[],
  portraitDominant: boolean,
  containerAspects: number[] = [],
): number {
  if (currentIndex < 0) return -1;
  let offset = 0;
  const grids = chunkSizes.map((count, index) => {
    const grid = gridFor(count, portraitDominant, containerAspects[index]);
    const start = offset;
    offset += count;
    return { ...grid, start };
  });
  const displayIndex = grids.findIndex(
    (grid) =>
      currentIndex >= grid.start && currentIndex < grid.start + grid.count,
  );
  if (displayIndex < 0) return currentIndex;
  const grid = grids[displayIndex];
  const local = currentIndex - grid.start;
  const column = Math.floor(local / grid.rows);
  const row = local % grid.rows;

  if (direction === "up") {
    return row > 0 ? currentIndex - 1 : currentIndex;
  }
  if (direction === "down") {
    return row + 1 < grid.rows && local + 1 < grid.count
      ? currentIndex + 1
      : currentIndex;
  }
  if (direction === "left") {
    if (column > 0) {
      const candidate = local - grid.rows;
      return grid.start + Math.min(candidate, grid.count - 1);
    }
    const previous = grids[displayIndex - 1];
    if (previous === undefined) return currentIndex;
    const previousColumn = previous.columns - 1;
    return (
      previous.start +
      Math.min(previous.count - 1, previousColumn * previous.rows + row)
    );
  }
  if (column + 1 < grid.columns) {
    const candidate = local + grid.rows;
    return candidate < grid.count ? grid.start + candidate : currentIndex;
  }
  const next = grids[displayIndex + 1];
  if (next === undefined) return currentIndex;
  return next.start + Math.min(row, next.count - 1);
}
