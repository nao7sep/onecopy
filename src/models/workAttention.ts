import type { SortChoice } from "./items";

export interface WorkAttention {
  selectedHash: string | null;
  visibleHashes: string[];
  nearbyHashes: string[];
  sectionKind: "image" | "video" | "other" | null;
  sectionMonth: string | null;
  sectionSort: SortChoice;
  sectionAnchor: number;
  sectionTotal: number;
}

export interface ViewportAttention {
  sectionKey: string;
  visibleHashes: string[];
  nearbyHashes: string[];
  anchor: number;
}

export function viewportAttention(
  hashes: (string | null)[], windowStart: number, first: number, end: number,
): Pick<ViewportAttention, "visibleHashes" | "nearbyHashes" | "anchor"> {
  const visibleHashes: string[] = [];
  const nearby: { hash: string; distance: number }[] = [];
  const radius = Math.max(8, end - first);
  hashes.forEach((hash, offset) => {
    if (hash === null) return;
    const position = windowStart + offset;
    if (position >= first && position < end) visibleHashes.push(hash);
    else {
      const distance = position < first ? first - position : position - end + 1;
      if (distance <= radius) nearby.push({ hash, distance });
    }
  });
  return {
    visibleHashes,
    nearbyHashes: nearby.sort((a, b) => a.distance - b.distance).map(({ hash }) => hash),
    anchor: first,
  };
}

/** A comparison page replaces hidden Main hints; Preview follows Main's anchor. */
export function resolveWorkAttention(
  main: WorkAttention,
  comparison: { selected: string | null; visible: string[] } | null,
  viewer: { hash: string | null; sectionIndex: number } | null,
): WorkAttention {
  if (comparison !== null) return {
    ...main, selectedHash: comparison.selected, visibleHashes: comparison.visible,
    nearbyHashes: [], sectionKind: null, sectionMonth: null, sectionTotal: 0,
  };
  if (viewer !== null) return {
    ...main, selectedHash: viewer.hash,
    visibleHashes: viewer.hash === null ? [] : [viewer.hash],
    // Main scrolls to the viewer; only use its neighborhood once it catches up.
    nearbyHashes: main.visibleHashes.includes(viewer.hash ?? "") ? main.nearbyHashes : [],
    sectionAnchor: viewer.sectionIndex,
  };
  return main;
}
