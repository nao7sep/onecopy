import { useEffect, useRef, useState } from "react";
import {
  extLabel,
  formatDuration,
  sortItems,
  thumbUrl,
  type SectionItem,
  type SortOrder,
} from "../models/items";
import { itemKey, useItemsStore } from "../state/items-store";

// Tile geometry used for column measurement (w-40 = 160px, gap-3 = 12px).
const TILE_WIDTH = 160;
const TILE_GAP = 12;

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
      className="relative w-40 cursor-grab"
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
        // Window-wide closed hand for the drag's duration (App.css rule) —
        // the pointer roams over elements with their own cursors otherwise.
        document.body.classList.add("dragging");
      }}
      onDragEnd={() => document.body.classList.remove("dragging")}
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
  const selectedItem = useItemsStore((s) => s.selectedItem);
  const selectItem = useItemsStore((s) => s.selectItem);
  const toggleItem = useItemsStore((s) => s.toggleItem);
  const rangeSelect = useItemsStore((s) => s.rangeSelect);
  const sortOrder = useItemsStore((s) => s.sortOrder);
  const setSortOrder = useItemsStore((s) => s.setSortOrder);

  // The grid is ONE composite control: the scroll container is the single tab
  // stop (active-descendant style — selection state lives in the store, never
  // in DOM focus), arrows move the selection, Shift+arrows extend it. The
  // command layer (Delete/Enter in App) reads the same source of truth.
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [columns, setColumns] = useState(1);
  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const measure = () =>
      setColumns(
        Math.max(1, Math.floor((container.clientWidth - TILE_GAP) / (TILE_WIDTH + TILE_GAP))),
      );
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(container);
    return () => observer.disconnect();
  }, [loading, items.length]);

  // The anchor stays in view across deletes and refreshes — the recovery
  // selection lands off-screen otherwise ("nearest" makes it a no-op when
  // already visible, so arrow navigation double-scrolls harmlessly).
  useEffect(() => {
    if (selectedItem === null) return;
    containerRef.current
      ?.querySelector(`[data-item-key="${CSS.escape(selectedItem)}"]`)
      ?.scrollIntoView({ block: "nearest" });
  }, [selectedItem]);

  // During a same-section refresh the stale items keep rendering (the store
  // keeps them), so the scroll container never unmounts and its position
  // survives the reload; only a genuinely empty grid shows the text states.
  if (items.length === 0) {
    return (
      <p className="m-auto text-ink-muted">
        {loading ? "Loading…" : "Nothing in this section"}
      </p>
    );
  }
  const sorted = sortItems(items, sortOrder);
  const sortedKeys = sorted.map(itemKey);

  const onGridKeyDown = (event: React.KeyboardEvent) => {
    const step =
      event.key === "ArrowRight"
        ? 1
        : event.key === "ArrowLeft"
          ? -1
          : event.key === "ArrowDown"
            ? columns
            : event.key === "ArrowUp"
              ? -columns
              : event.key === "Home"
                ? Number.NEGATIVE_INFINITY
                : event.key === "End"
                  ? Number.POSITIVE_INFINITY
                  : null;
    if (step === null) return;
    event.preventDefault();
    const current = selectedItem !== null ? sortedKeys.indexOf(selectedItem) : -1;
    const target = Number.isFinite(step)
      ? Math.min(Math.max(current < 0 ? 0 : current + (step as number), 0), sortedKeys.length - 1)
      : step === Number.NEGATIVE_INFINITY
        ? 0
        : sortedKeys.length - 1;
    const key = sortedKeys[target];
    if (key === undefined) return;
    if (event.shiftKey) {
      rangeSelect(sortedKeys, key);
      useItemsStore.setState({ selectedItem: key });
    } else {
      selectItem(key);
    }
    // Minimal scroll-into-view for the active tile.
    containerRef.current
      ?.querySelector(`[data-item-key="${CSS.escape(key)}"]`)
      ?.scrollIntoView({ block: "nearest" });
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 items-center justify-end gap-2 border-b border-border px-3 py-1 text-xs text-ink-muted">
        <button
          className="rounded border border-border px-1 py-0.5 text-ink hover:bg-surface-muted"
          title="Re-check only the directories this section's files came from"
          onClick={() => void useItemsStore.getState().rescanSection()}
        >
          Rescan section
        </button>
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
      <div
        ref={containerRef}
        tabIndex={0}
        role="listbox"
        aria-label="Section items"
        aria-multiselectable
        className="flex min-h-0 flex-1 flex-wrap content-start gap-3 overflow-y-auto p-3 outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-primary-ring"
        onKeyDown={onGridKeyDown}
      >
        {sorted.map((item) => {
          const key = itemKey(item);
          return (
            <div key={key} data-item-key={key} role="option" aria-selected={selectedKeys.has(key)}>
              <Tile
                item={item}
                isSelected={selectedKeys.has(key)}
                onSelect={(event) => {
                  containerRef.current?.focus();
                  if (event.metaKey || event.ctrlKey) {
                    toggleItem(key);
                  } else if (event.shiftKey) {
                    rangeSelect(sortedKeys, key);
                  } else {
                    selectItem(key);
                  }
                }}
              />
            </div>
          );
        })}
      </div>
    </div>
  );
}
