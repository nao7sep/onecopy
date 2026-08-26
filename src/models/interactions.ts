import type { SectionItem } from "./items";

/** Enter has one main-grid meaning: a live similar family, or no action. */
export function comparisonHashForEnter(
  item: Pick<SectionItem, "hash" | "similarGroupId"> | null | undefined,
): string | null {
  if (item === null || item === undefined || item.hash === null) return null;
  return item.similarGroupId === null ? null : item.hash;
}
