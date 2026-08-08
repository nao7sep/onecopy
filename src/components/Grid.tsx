import {
  extLabel,
  sortItems,
  thumbUrl,
  type SectionItem,
  type SortOrder,
} from "../models/items";
import { itemKey, useItemsStore } from "../state/items-store";

// The center-pane thumbnail grid. Native lazy loading keeps a large month
// cheap; every pixel comes from the mediacache protocol, never original files.
// Click selects; Delete/Backspace trash-deletes the selection (every copy),
// Shift makes it permanent — the keydown lives in App.

function Tile({
  item,
  isSelected,
  onSelect,
}: {
  item: SectionItem;
  isSelected: boolean;
  onSelect: () => void;
}) {
  return (
    <figure className="relative w-40 cursor-default" onClick={onSelect}>
      <div
        className={`flex h-32 w-40 items-center justify-center overflow-hidden rounded border ${
          isSelected ? "border-primary-ring ring-2 ring-primary-ring" : "border-border"
        } bg-surface`}
      >
        {item.hash !== null && item.hasThumb ? (
          <img
            src={thumbUrl(item.hash)}
            alt={item.fileName}
            loading="lazy"
            className="max-h-full max-w-full object-contain"
          />
        ) : (
          <span className="text-lg font-semibold text-ink-muted">
            {extLabel(item.fileName)}
          </span>
        )}
      </div>
      {item.copyCount > 1 ? (
        <span className="absolute right-1 top-1 rounded bg-primary-surface px-1 text-xs text-primary">
          ×{item.copyCount}
        </span>
      ) : null}
      {item.similarGroupId !== null ? (
        <span
          className="absolute left-1 top-1 rounded bg-surface-muted px-1 text-xs text-ink"
          title="Has similar photos — press Enter to compare"
        >
          ≈
        </span>
      ) : null}
      {item.hasCompanions ? (
        <span
          className="absolute bottom-7 right-1 rounded bg-surface-muted px-1 text-xs text-ink-muted"
          title="Has a paired companion file (RAW/sidecar) — every action includes it"
        >
          pair
        </span>
      ) : null}
      <figcaption
        className="mt-0.5 w-40 truncate text-xs text-ink-muted"
        title={item.fileName}
      >
        {item.fileName}
      </figcaption>
    </figure>
  );
}

const SORT_LABELS: Record<SortOrder, string> = {
  time: "Time taken",
  name: "Name",
  size: "Size",
  resolution: "Resolution",
};

export default function Grid({
  items,
  loading,
}: {
  items: SectionItem[];
  loading: boolean;
}) {
  const selectedItem = useItemsStore((s) => s.selectedItem);
  const selectItem = useItemsStore((s) => s.selectItem);
  const sortOrder = useItemsStore((s) => s.sortOrder);
  const setSortOrder = useItemsStore((s) => s.setSortOrder);
  if (loading) {
    return <p className="m-auto text-ink-muted">Loading…</p>;
  }
  if (items.length === 0) {
    return <p className="m-auto text-ink-muted">Nothing in this section</p>;
  }
  const sorted = sortItems(items, sortOrder);
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 items-center justify-end gap-1 border-b border-border px-3 py-1 text-xs text-ink-muted">
        <label htmlFor="grid-sort">Sort</label>
        <select
          id="grid-sort"
          className="rounded border border-border bg-surface px-1 py-0.5 text-ink"
          value={sortOrder}
          onChange={(e) => setSortOrder(e.target.value as SortOrder)}
        >
          {(Object.keys(SORT_LABELS) as SortOrder[]).map((order) => (
            <option key={order} value={order}>
              {SORT_LABELS[order]}
            </option>
          ))}
        </select>
      </div>
      <div className="flex min-h-0 flex-1 flex-wrap content-start gap-3 overflow-y-auto p-3">
        {sorted.map((item) => {
          const key = itemKey(item);
          return (
            <Tile
              key={key}
              item={item}
              isSelected={selectedItem === key}
              onSelect={() => selectItem(key)}
            />
          );
        })}
      </div>
    </div>
  );
}
