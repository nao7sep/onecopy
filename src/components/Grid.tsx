import { useEffect, useRef, useState } from "react";
import {
  SORT_ORDERS,
  extLabel,
  extOf,
  factsLine,
  formatBytes,
  formatDuration,
  sortItems,
  thumbUrl,
  type SectionItem,
  type SortOrder,
} from "../models/items";
import { itemKey, useItemsStore } from "../state/items-store";
import { handleSpaceLook } from "../state/preview-store";
import { scrollTopForRow, visibleWindow } from "../utils/virtualize";
import { formatLocalMinute } from "../utils/displayTime";
import { hasOpenModal } from "../utils/modalStack";
import PreviewControl from "./PreviewControl";

// Tile geometry used for column measurement (w-40 = 160px, gap-3 = 12px).
const TILE_WIDTH = 160;
const TILE_GAP = 12;
// List rows use gap-0.5 (2px).
const LIST_GAP = 2;
// Row-height fallbacks until the first real row is measured.
const TILE_ROW_ESTIMATE = 190;
const LIST_ROW_ESTIMATE = 34;

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

/** The other-files table columns, in render order. Each is a REAL sortable
 * column (Finder/Explorer): clicking its header applies the matching order,
 * and the set is exactly the model's `other` catalogue, so a column the sort
 * cannot honour can never appear. Name is the flexible column; the rest are
 * fixed so the table scans vertically. */
const OTHER_COLUMNS: { order: SortOrder; label: string; width: string }[] = [
  { order: "ext", label: "Kind", width: "w-14" },
  { order: "name", label: "Name", width: "min-w-0 flex-1" },
  { order: "size", label: "Size", width: "w-20 text-right" },
  { order: "time", label: "Date", width: "w-36" },
  { order: "folder", label: "Folder", width: "w-56" },
];

function ListHeader({
  sortOrder,
  onSort,
}: {
  sortOrder: SortOrder;
  onSort: (order: SortOrder) => void;
}) {
  return (
    <div className="flex shrink-0 items-center gap-3 border-b border-border px-5 py-1 text-[11px] font-semibold uppercase tracking-wide text-ink-muted">
      {OTHER_COLUMNS.map((column) => (
        <button
          key={column.order}
          className={`${column.width} shrink-0 truncate text-left transition-colors hover:text-ink ${
            sortOrder === column.order ? "text-ink" : ""
          } ${column.width.includes("text-right") ? "text-right" : ""}`}
          onClick={() => onSort(column.order)}
          title={`Sort by ${column.label.toLowerCase()}`}
        >
          {column.label}
          {sortOrder === column.order ? " ▾" : ""}
        </button>
      ))}
    </div>
  );
}

function ListRow({
  item,
  isSelected,
  onSelect,
}: {
  item: SectionItem;
  isSelected: boolean;
  onSelect: (event: React.MouseEvent) => void;
}) {
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
      <span className="w-14 shrink-0 truncate text-[11px] font-semibold text-ink-muted">
        {extOf(item.fileName).toUpperCase() || extLabel(item.fileName)}
      </span>
      <span className="min-w-0 flex-1 truncate text-ink" title={item.fileName}>
        {item.fileName}
        {item.hasCompanions ? (
          <span
            className="ml-2 rounded-md bg-surface-muted px-1.5 py-0.5 text-[11px] text-ink-muted"
            title="Has a paired companion file (RAW/sidecar) — every action includes it"
          >
            pair
          </span>
        ) : null}
        {item.copyCount > 1 ? (
          <span className="ml-2 rounded-md bg-primary-surface px-1.5 py-0.5 text-[11px] font-medium text-primary">
            ×{item.copyCount}
          </span>
        ) : null}
      </span>
      <span className="w-20 shrink-0 text-right tabular-nums text-xs text-ink-muted">
        {item.byteSize !== null ? formatBytes(item.byteSize) : ""}
      </span>
      <span className="w-36 shrink-0 truncate tabular-nums text-xs text-ink-muted">
        {item.resolvedUtcMs !== null ? formatLocalMinute(item.resolvedUtcMs) : "—"}
      </span>
      {/* dir="rtl" keeps the DEEP end of a long path visible — the part that
          distinguishes two folders is the tail, not the shared root. */}
      <span
        dir="rtl"
        className="w-56 shrink-0 truncate text-left text-xs text-ink-muted"
        title={item.dirPath}
      >
        {item.dirPath}
      </span>
    </div>
  );
}


export default function Grid({
  items,
  loading,
  layout,
  mayClaimFocus,
}: {
  items: SectionItem[];
  loading: boolean;
  /** Thumbnails for images and videos; rows for other-files, which have
   * nothing to show in a tile. */
  layout: "tiles" | "list";
  /** False while a boot gate (the wizard, the missing-volume gate) owns the
   * screen — those overlays are opaque but focus nothing themselves, so the
   * grid behind them must not quietly take the keyboard. */
  mayClaimFocus: boolean;
}) {
  const selectedKeys = useItemsStore((s) => s.selectedKeys);
  const selectedItem = useItemsStore((s) => s.selectedItem);
  const selectItem = useItemsStore((s) => s.selectItem);
  const toggleItem = useItemsStore((s) => s.toggleItem);
  const rangeSelect = useItemsStore((s) => s.rangeSelect);
  const lane = layout === "list" ? "other" : "media";
  const sortOrder = useItemsStore((s) =>
    lane === "other" ? s.sortOrders.other : s.sortOrders.media,
  );
  const setSortOrder = useItemsStore((s) => s.setSortOrder);
  const sortCatalogue = SORT_ORDERS[lane];

  // The grid is ONE composite control: the scroll container is the single tab
  // stop (active-descendant style — selection state lives in the store, never
  // in DOM focus), arrows move the selection, Shift+arrows extend it. The
  // command layer (Delete/Enter in App) reads the same source of truth.
  const containerRef = useRef<HTMLDivElement | null>(null);

  // A restored section arrives WITHOUT anyone having clicked: on boot the app
  // reopens the last month itself, so the items appear under a keyboard that
  // still points at <body>. Every arrow, Space and Enter then went nowhere,
  // and the app read as frozen until the user happened to click the grid.
  //
  // Claimed only when nothing else has focus (body or null). A user who
  // clicked the sidebar, opened a modal, or is typing in a field owns the
  // keyboard, and a late-arriving refresh must never pull it away mid-word —
  // which is also why this runs on ARRIVAL, not on every render.
  const claimedFor = useRef<SectionItem | null>(null);
  const first = items[0] ?? null;
  useEffect(() => {
    if (loading || first === null || !mayClaimFocus) return;
    if (claimedFor.current === first) return;
    claimedFor.current = first;
    if (hasOpenModal()) return;
    const active = document.activeElement;
    if (active !== null && active !== document.body) return;
    containerRef.current?.focus({ preventScroll: true });
  }, [loading, first, mayClaimFocus]);

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

  // Virtualization state: only rows near the viewport exist in the DOM — at
  // 20,000+ items per month, mounting every tile's node makes scroll and
  // selection crawl even though the images themselves lazy-load. The row
  // height is MEASURED off the first rendered item (a constant would drift
  // with any styling change); the estimate carries the first paint.
  const [scrollTop, setScrollTop] = useState(0);
  const [viewportHeight, setViewportHeight] = useState(600);
  const [measuredItemHeight, setMeasuredItemHeight] = useState<number | null>(null);
  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const measure = () => setViewportHeight(container.clientHeight);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(container);
    return () => observer.disconnect();
  }, []);
  const gap = layout === "list" ? LIST_GAP : TILE_GAP;
  const rowHeight =
    (measuredItemHeight ?? (layout === "list" ? LIST_ROW_ESTIMATE - gap : TILE_ROW_ESTIMATE - gap)) +
    gap;
  const measureItem = (node: HTMLDivElement | null) => {
    if (node !== null && node.offsetHeight > 0 && node.offsetHeight !== measuredItemHeight) {
      setMeasuredItemHeight(node.offsetHeight);
    }
  };

  // The anchor stays in view across deletes and refreshes — the recovery
  // selection lands off-screen otherwise ("nearest" makes it a no-op when
  // already visible, so arrow navigation double-scrolls harmlessly). A
  // virtualized-out anchor has no node to ask, so its row position is
  // computed instead.
  useEffect(() => {
    if (selectedItem === null) return;
    const container = containerRef.current;
    if (!container) return;
    const node = container.querySelector(`[data-item-key="${CSS.escape(selectedItem)}"]`);
    if (node !== null) {
      node.scrollIntoView({ block: "nearest" });
      return;
    }
    const index = sortedKeysRef.current.indexOf(selectedItem);
    if (index < 0) return;
    container.scrollTop = scrollTopForRow(
      Math.floor(index / Math.max(1, columnsRef.current)),
      container.clientHeight,
      rowHeightRef.current,
    );
  }, [selectedItem]);

  // During a same-section refresh the stale items keep rendering (the store
  // keeps them), so the scroll container never unmounts and its position
  // survives the reload. An empty grid keeps its focusable container too —
  // an empty composite is still reachable by Tab (composite-control rules).
  const emptyState =
    items.length === 0 ? (loading ? "Loading…" : "Nothing in this section") : null;
  const sorted = emptyState === null ? sortItems(items, sortOrder) : [];
  const sortedKeys = sorted.map(itemKey);
  // Refs for the anchor effect, which must read current values without
  // re-running on every scroll.
  const sortedKeysRef = useRef(sortedKeys);
  sortedKeysRef.current = sortedKeys;
  const columnsRef = useRef(columns);
  columnsRef.current = columns;
  const rowHeightRef = useRef(rowHeight);
  rowHeightRef.current = rowHeight;

  const totalRows = Math.ceil(sorted.length / Math.max(1, columns));
  const win = visibleWindow(scrollTop, viewportHeight, rowHeight, totalRows);
  const visible = sorted.slice(win.startRow * columns, win.endRow * columns);

  const onGridKeyDown = (event: React.KeyboardEvent) => {
    // Space = LOOK (the agreed model): toggle the preview, through the one
    // shared rule — with a video loaded in the preview the video surface owns
    // the key instead (play/pause), so the rule must not claim it here.
    if (event.key === " ") {
      handleSpaceLook(event);
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
          {(Object.keys(sortCatalogue.orders) as SortOrder[]).map((order) => (
            <option key={order} value={order}>
              {sortCatalogue.orders[order]}
            </option>
          ))}
        </select>
      </div>
      {layout === "list" ? (
        // The table header lives OUTSIDE the scroll container, so the
        // virtualization geometry below never has to account for it.
        <ListHeader sortOrder={sortOrder} onSort={setSortOrder} />
      ) : null}
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
        onScroll={(event) => setScrollTop((event.target as HTMLDivElement).scrollTop)}
      >
        {emptyState !== null ? (
          <p className="m-auto text-ink-muted">{emptyState}</p>
        ) : null}
        {/* The spacers stand in for the unmounted rows, keeping the
            scrollbar's geometry honest. basis-full forces each onto its own
            flex row. */}
        {win.topPad > 0 ? (
          <div aria-hidden className="w-full shrink-0 basis-full" style={{ height: win.topPad - gap }} />
        ) : null}
        {visible.map((item, i) => {
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
              ref={i === 0 ? measureItem : undefined}
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
        {win.bottomPad > 0 ? (
          <div aria-hidden className="w-full shrink-0 basis-full" style={{ height: win.bottomPad - gap }} />
        ) : null}
      </div>
    </div>
  );
}
