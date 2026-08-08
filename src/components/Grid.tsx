import {
  extLabel,
  formatDuration,
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
  onSelect: (event: React.MouseEvent) => void;
}) {
  return (
    <figure
      className="relative w-40 cursor-default"
      onClick={onSelect}
      draggable
      onDragStart={(event) => {
        // Dragging an unselected tile re-anchors the selection onto it, so
        // the drag always carries exactly what looks selected.
        const { selectedKeys, selectItem } = useItemsStore.getState();
        const key = itemKey(item);
        if (!selectedKeys.has(key)) selectItem(key);
        event.dataTransfer.setData("application/x-onecopy-drag", "selection");
        event.dataTransfer.effectAllowed = "copyMove";
      }}
    >
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
      {item.durationMs !== null ? (
        <span className="absolute bottom-7 left-1 rounded bg-surface-muted px-1 text-xs text-ink">
          {formatDuration(item.durationMs)}
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
  const selectedKeys = useItemsStore((s) => s.selectedKeys);
  const selectItem = useItemsStore((s) => s.selectItem);
  const toggleItem = useItemsStore((s) => s.toggleItem);
  const rangeSelect = useItemsStore((s) => s.rangeSelect);
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
              isSelected={selectedKeys.has(key)}
              onSelect={(event) => {
                if (event.metaKey || event.ctrlKey) {
                  toggleItem(key);
                } else if (event.shiftKey) {
                  rangeSelect(sorted.map(itemKey), key);
                } else {
                  selectItem(key);
                }
              }}
            />
          );
        })}
      </div>
    </div>
  );
}
