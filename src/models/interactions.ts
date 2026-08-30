import type { SectionItem } from "./items";

/** Comparison admission is selection-wide. Looking only at the anchor would
 * silently discard the meaning of a mixed multi-selection. */
export function comparisonHashForSelection(
  items: readonly Pick<SectionItem, "hash" | "similarGroupId">[],
  selectedKeys: ReadonlySet<string>,
  anchor: string | null,
): string | null {
  if (anchor === null || selectedKeys.size === 0) return null;
  const selected = items.filter(
    (item) => item.hash !== null && selectedKeys.has(item.hash),
  );
  if (selected.length !== selectedKeys.size) return null;
  const group = selected[0]?.similarGroupId ?? null;
  if (group === null || selected.some((item) => item.similarGroupId !== group)) return null;
  return selected.some((item) => item.hash === anchor) ? anchor : null;
}
