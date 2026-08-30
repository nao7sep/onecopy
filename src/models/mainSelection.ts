export interface AnchorContext {
  index: number;
  before: string[];
  after: string[];
}

export interface SectionMemory {
  anchor: string | null;
  context: AnchorContext | null;
}

const NEIGHBOR_LIMIT = 64;

/** A bounded record of the work position. It keeps exact nearby identities
 * for next/previous recovery without serializing a million-item section into
 * app state; the index remains a truthful approximate fallback if the whole
 * neighborhood disappeared while OneCopy was away. */
export function anchorContext(order: readonly string[], anchor: string | null): AnchorContext | null {
  if (anchor === null) return null;
  const index = order.indexOf(anchor);
  if (index < 0) return null;
  return {
    index,
    before: order.slice(Math.max(0, index - NEIGHBOR_LIMIT), index).reverse(),
    after: order.slice(index + 1, index + 1 + NEIGHBOR_LIMIT),
  };
}

/** Recover the former place: the nearest following survivor wins, then the
 * nearest preceding survivor. If every remembered neighbor disappeared, the
 * prior ordinal gives a useful nearby position among the current items. */
export function recoverAnchor(
  currentOrder: readonly string[],
  rememberedAnchor: string | null,
  context: AnchorContext | null,
  allowed?: ReadonlySet<string>,
): string | null {
  if (rememberedAnchor === null) return null;
  const current = new Set(currentOrder);
  const eligible = (key: string) => current.has(key) && (allowed === undefined || allowed.has(key));
  if (eligible(rememberedAnchor)) return rememberedAnchor;
  if (context !== null) {
    const next = context.after.find(eligible);
    if (next !== undefined) return next;
    const previous = context.before.find(eligible);
    if (previous !== undefined) return previous;
  }
  const candidates = allowed === undefined
    ? currentOrder
    : currentOrder.filter((key) => allowed.has(key));
  if (candidates.length === 0) return null;
  const index = Math.min(Math.max(context?.index ?? 0, 0), candidates.length - 1);
  return candidates[index] ?? null;
}

export function parseAnchorContext(value: unknown): AnchorContext | null {
  if (typeof value !== "object" || value === null) return null;
  const record = value as Record<string, unknown>;
  if (!Number.isInteger(record.index) || (record.index as number) < 0) return null;
  if (!Array.isArray(record.before) || !record.before.every((key) => typeof key === "string")) {
    return null;
  }
  if (!Array.isArray(record.after) || !record.after.every((key) => typeof key === "string")) {
    return null;
  }
  return {
    index: record.index as number,
    before: record.before.slice(0, NEIGHBOR_LIMIT) as string[],
    after: record.after.slice(0, NEIGHBOR_LIMIT) as string[],
  };
}
