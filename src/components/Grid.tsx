import { useEffect, useRef, useState } from "react";
import {
  extLabel,
  factsLine,
  formatDuration,
  sortItems,
  thumbUrl,
  type SectionItem,
  type SortOrder,
} from "../models/items";
import { itemKey, useItemsStore } from "../state/items-store";
import { usePreviewStore } from "../state/preview-store";
import PreviewControl from "./PreviewControl";

// Tile geometry used for column measurement (w-40 = 160px, gap-3 = 12px).
const TILE_WIDTH = 160;
const TILE_GAP = 12;

// The center-pane thumbnail grid. Native lazy loading keeps a large month
// cheap; every pixel comes from the mediacache protocol, never original files.
// Click selects; Delete/Backspace trash-deletes the selection (every copy),
// Shift makes it permanent — the keydown lives in App.

/** Starts a drag carrying the whole selection. Shared by the tile and the list
 * row so both layouts drop into the destination tree identically. */
function dragProps(item: SectionItem) {
  return {
    draggable: true,
    onDragStart: (event: React.DragEvent) => {
      // Dragging an unselected item re-anchors the selection onto it, so the
      // drag always carries exactly what looks selected.
      const { selectedKeys, selectItem } = useItemsStore.getState();
      const key = itemKey(item);
      if (!selectedKeys.has(key)) selectItem(key);
      event.dataTransfer.setData("application/x-onecopy-drag", "selection");
      event.dataTransfer.effectAllowed = "copyMove";
      // Window-wide closed hand for the drag's duration (App.css rule) — the
      // pointer roams over elements with their own cursors otherwise.
      document.body.classList.add("dragging");
    },
    onDragEnd: () => document.body.classList.remove("dragging"),
  };
}

function Thumb({ item }: { item: SectionItem }) {
  const [thumbFailed, setThumbFailed] = useState(false);
  if (item.hash === null || !item.hasThumb || thumbFailed) {
    return (
      <span className="text-sm font-semibold text-ink-muted">{extLabel(item.fileName)}</span>
    );
  }
  return (
    <img
      src={thumbUrl(item.hash)}
      alt={item.fileName}
      loading="lazy"
      // h/w-full rather than max-h/max-w: an image SMALLER than the tile is
      // scaled UP to fill it, so its softness is the signal that it is a small
      // file. Left at its native size it sat neatly in the middle and looked
      // like a deliberately small thumbnail of a large photo. The derive
      // pipeline still never upscales what it stores — this is display only.
      className="h-full w-full object-contain"
      // A cache entry can be absent even when the row claims one (a
      // hand-deleted cache file, a move interrupted mid-flight). Falling back
      // to the extension label keeps the grid readable instead of showing the
      // webview's broken-image glyph.
      onError={() => setThumbFailed(true)}
    />
  );
}

function Tile({
  item,
  isSelected,
  onSelect,
}: {
  item: SectionItem;
  isSelected: boolean;
  onSelect: (event: React.MouseEvent) => void;
}) {
  const facts = factsLine(item);
  return (
    <figure className="relative w-40 cursor-grab" onClick={onSelect} {...dragProps(item)}>
      <div
        className={`flex h-32 w-40 items-center justify-center overflow-hidden rounded-lg border transition-colors ${
          isSelected ? "border-primary-ring ring-2 ring-primary-ring" : "border-border"
        } bg-surface`}
      >
        <Thumb item={item} />
      </div>
      {item.copyCount > 1 ? (
        <span className="absolute right-1 top-1 rounded-md bg-primary-surface px-1.5 py-0.5 text-[11px] font-medium text-primary">
          ×{item.copyCount}
        </span>
      ) : null}
      {item.similarGroupId !== null ? (
        <span
          className="absolute left-1 top-1 rounded-md bg-surface-muted px-1.5 py-0.5 text-[11px] text-ink"
          title="Has similar photos — press Enter to compare"
        >
          ≈
        </span>
      ) : null}
      {item.durationMs !== null ? (
        <span className="absolute bottom-9 left-1 rounded-md bg-surface-muted px-1.5 py-0.5 text-[11px] text-ink">
          {formatDuration(item.durationMs)}
        </span>
      ) : null}
      {item.hasCompanions ? (
        <span
          className="absolute bottom-9 right-1 rounded-md bg-surface-muted px-1.5 py-0.5 text-[11px] text-ink-muted"
          title="Has a paired companion file (RAW/sidecar) — every action includes it"
        >
          pair
        </span>
      ) : null}
      <figcaption className="mt-1 w-40" title={item.fileName}>
        <span className="block truncate text-xs text-ink">{item.fileName}</span>
        {/* Pixels and bytes, quietly. Without them a section of the same shot
            at three qualities is undecidable: the original and the for-web
            copy are the same picture at tile size. */}
        {facts !== "" ? (
          <span className="block truncate text-[11px] tabular-nums text-ink-muted">
            {facts}
          </span>
        ) : null}
      </figcaption>
    </figure>
  );
}

/** Other-files render as ROWS, not tiles.
 *
 * A document has nothing to look at, so a thumbnail grid spent a 160×128 box
 * per file to show an extension in the middle of it — a handful per screen
 * where a list shows dozens, and none of the facts that actually distinguish
 * two files. The keyboard contract is unchanged: the same composite, one
 * column instead of several. */
function ListRow({
  item,
  isSelected,
  onSelect,
}: {
  item: SectionItem;
  isSelected: boolean;
  onSelect: (event: React.MouseEvent) => void;
}) {
  const facts = factsLine(item);
  return (
    <div
      className={`flex w-full cursor-grab items-center gap-3 rounded-md border px-3 py-1.5 text-sm transition-colors ${
        isSelected
          ? "border-primary-ring bg-primary-surface"
          : "border-transparent hover:bg-surface-muted"
      }`}
      onClick={onSelect}
      {...dragProps(item)}
    >
      <span className="w-12 shrink-0 truncate text-[11px] font-semibold text-ink-muted">
        {extLabel(item.fileName)}
      </span>
      <span className="min-w-0 flex-1 truncate text-ink" title={item.fileName}>
        {item.fileName}
      </span>
      {item.hasCompanions ? (
        <span
          className="shrink-0 rounded-md bg-surface-muted px-1.5 py-0.5 text-[11px] text-ink-muted"
          title="Has a paired companion file (RAW/sidecar) — every action includes it"
        >
          pair
        </span>
      ) : null}
      {item.copyCount > 1 ? (
        <span className="shrink-0 rounded-md bg-primary-surface px-1.5 py-0.5 text-[11px] font-medium text-primary">
          ×{item.copyCount}
        </span>
      ) : null}
      <span className="shrink-0 tabular-nums text-xs text-ink-muted">{facts}</span>
    </div>
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
  layout,
}: {
  items: SectionItem[];
  loading: boolean;
  /** Thumbnails for images and videos; rows for other-files, which have
   * nothing to show in a tile. */
  layout: "tiles" | "list";
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
    // A list is one column by definition, so Down moves one row — measuring
    // would compute a tile count the layout does not have.
    if (layout === "list") {
      setColumns(1);
      return;
    }
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
  }, [loading, items.length, layout]);

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
  // survives the reload. An empty grid keeps its focusable container too —
  // an empty composite is still reachable by Tab (composite-control rules).
  const emptyState =
    items.length === 0 ? (loading ? "Loading…" : "Nothing in this section") : null;
  const sorted = emptyState === null ? sortItems(items, sortOrder) : [];
  const sortedKeys = sorted.map(itemKey);

  const onGridKeyDown = (event: React.KeyboardEvent) => {
    // Space is QUICK LOOK: it opens the preview on the anchor and closes it
    // again, the gesture every Finder user already has. It previously toggled
    // the anchor's membership in the multi-selection — technically the listbox
    // idiom, but nobody found it, and Cmd-click and Shift-click already cover
    // multi-select. Nothing else in the app claims Space.
    if (event.key === " ") {
      event.preventDefault();
      void usePreviewStore.getState().toggleFollow();
      return;
    }
    // PageUp/PageDown jump by roughly a viewport of rows.
    const pageRows = Math.max(
      2,
      Math.floor(
        (containerRef.current?.clientHeight ?? 600) / (layout === "list" ? 34 : 190),
      ),
    );
    const step =
      event.key === "ArrowRight"
        ? 1
        : event.key === "ArrowLeft"
          ? -1
          : event.key === "ArrowDown"
            ? columns
            : event.key === "ArrowUp"
              ? -columns
              : event.key === "PageDown"
                ? columns * pageRows
                : event.key === "PageUp"
                  ? -columns * pageRows
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
      // Through the anchor path (not a raw setState) so persistence and the
      // preview follow see Shift+arrow moves too.
      useItemsStore.getState().setAnchor(key);
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
      <div className="flex shrink-0 items-center gap-2 border-b border-border px-2 py-1 text-xs text-ink-muted">
        <PreviewControl />
        <span className="flex-1" />
        <button
          className="h-7 rounded-md px-2 text-ink-muted transition-colors hover:bg-surface-muted hover:text-ink"
          title="Re-check only the directories this section's files came from"
          onClick={() => void useItemsStore.getState().rescanSection()}
        >
          Rescan section
        </button>
        <label htmlFor="grid-sort">Sort</label>
        <select
          id="grid-sort"
          className="h-7 rounded-md border border-border bg-surface px-2 text-ink"
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
        aria-activedescendant={selectedItem !== null ? `grid-opt-${selectedItem}` : undefined}
        aria-multiselectable
        className={`min-h-0 flex-1 overflow-y-auto outline-none ${
          layout === "list"
            ? "flex flex-col gap-0.5 p-2"
            : "flex flex-wrap content-start gap-3 p-3"
        }`}
        onKeyDown={onGridKeyDown}
      >
        {emptyState !== null ? (
          <p className="m-auto text-ink-muted">{emptyState}</p>
        ) : null}
        {sorted.map((item) => {
          const key = itemKey(item);
          const isSelected = selectedKeys.has(key);
          const onSelect = (event: React.MouseEvent) => {
            containerRef.current?.focus();
            if (event.metaKey || event.ctrlKey) {
              toggleItem(key);
            } else if (event.shiftKey) {
              rangeSelect(sortedKeys, key);
            } else {
              selectItem(key);
            }
          };
          return (
            <div
              key={key}
              id={`grid-opt-${key}`}
              data-item-key={key}
              role="option"
              aria-selected={isSelected}
              className={layout === "list" ? "w-full" : undefined}
            >
              {layout === "list" ? (
                <ListRow item={item} isSelected={isSelected} onSelect={onSelect} />
              ) : (
                <Tile item={item} isSelected={isSelected} onSelect={onSelect} />
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
