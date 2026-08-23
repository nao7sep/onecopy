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
  type SortChoice,
  type SortOrder,
} from "../models/items";
import { itemKey, useItemsStore } from "../state/items-store";
import { useAppStore } from "../state/app-store";
import { handleSpaceLook } from "../state/preview-store";
import { scrollTopForRow, visibleWindow } from "../utils/virtualize";
import { formatLocalMinute } from "../utils/displayTime";
import { hasOpenModal } from "../utils/modalStack";
import PreviewControl from "./PreviewControl";
import { ChevronDown, ChevronUp } from "lucide-react";

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
    onDragEnd: clearGridDragCursor,
  };
}

export function clearGridDragCursor() {
  document.body.classList.remove("dragging");
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
      {item.namesDiffer ? (
        <span
          className="absolute bottom-9 left-1 rounded-md bg-warning-surface px-1.5 py-0.5 text-[11px] text-warning"
          title="Copies of this file carry different names — Move and Copy are disabled until the names are resolved (reveal the copies from Details)"
        >
          ≠name
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

/** The other-files table columns, in render order. Each sortable column maps
 * to the model's `other` catalogue, so a column the sort cannot honour can
 * never appear — the FOLDERS column is exactly that case: copies merge into
 * one row with many folders (Phase 33), so it displays and never sorts.
 * Kind/Name/Size/Date carry persisted, draggable widths; Folders takes the
 * rest of the pane (the developer's rule — it is usually the longest text). */
const SIZED_COLUMNS = ["kind", "name", "size", "date"] as const;
export type SizedColumn = (typeof SIZED_COLUMNS)[number];
export const DEFAULT_COLUMN_WIDTHS: Record<SizedColumn, number> = {
  kind: 56,
  name: 300,
  size: 84,
  date: 148,
};
const MIN_COLUMN_WIDTH = 40;

/** Parses persisted widths; anything malformed falls back per column. */
export function columnWidthsFrom(value: unknown): Record<SizedColumn, number> {
  const out = { ...DEFAULT_COLUMN_WIDTHS };
  if (typeof value === "object" && value !== null) {
    const rec = value as Record<string, unknown>;
    for (const key of SIZED_COLUMNS) {
      const width = rec[key];
      if (typeof width === "number" && Number.isFinite(width) && width >= MIN_COLUMN_WIDTH) {
        out[key] = Math.round(width);
      }
    }
  }
  return out;
}

const OTHER_COLUMNS: {
  key: SizedColumn | "folders";
  order: SortOrder | null;
  label: string;
}[] = [
  { key: "kind", order: "ext", label: "Kind" },
  { key: "name", order: "name", label: "Name" },
  { key: "size", order: "size", label: "Size" },
  { key: "date", order: "time", label: "Date" },
  { key: "folders", order: null, label: "Folders" },
];

function columnStyle(
  key: SizedColumn | "folders",
  widths: Record<SizedColumn, number>,
): React.CSSProperties {
  return key === "folders"
    ? { flex: "1 1 0", minWidth: 0 }
    : { width: widths[key], flex: "0 0 auto" };
}

function ListHeader({
  sort,
  onSort,
  widths,
  onWidths,
}: {
  sort: SortChoice;
  onSort: (order: SortOrder) => void;
  widths: Record<SizedColumn, number>;
  onWidths: (widths: Record<SizedColumn, number>) => void;
}) {
  const beginResize = (key: SizedColumn) => (event: React.MouseEvent) => {
    event.preventDefault();
    event.stopPropagation();
    const startX = event.clientX;
    const startWidth = widths[key];
    document.body.classList.add("col-resizing");
    const onMove = (e: MouseEvent) => {
      onWidths({
        ...widths,
        [key]: Math.max(MIN_COLUMN_WIDTH, startWidth + (e.clientX - startX)),
      });
    };
    const onUp = () => {
      document.body.classList.remove("col-resizing");
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  };
  return (
    <div className="flex shrink-0 items-center border-b border-border px-5 py-1 text-[11px] font-semibold uppercase tracking-wide text-ink-muted">
      {OTHER_COLUMNS.map((column) => (
        <span
          key={column.key}
          className="relative flex items-center"
          style={columnStyle(column.key, widths)}
        >
          {column.order !== null ? (
            <button
              className={`min-w-0 flex-1 truncate text-left transition-colors hover:text-ink ${
                sort.order === column.order ? "text-ink" : ""
              } ${column.key === "size" ? "text-right" : ""}`}
              onClick={() => onSort(column.order!)}
              title={`Sort by ${column.label.toLowerCase()} (again to reverse)`}
            >
              {column.label}
              {sort.order === column.order ? (
                sort.desc ? <ChevronDown size={12} className="ml-0.5 inline-block" /> : <ChevronUp size={12} className="ml-0.5 inline-block" />
              ) : null}
            </button>
          ) : (
            <span className="min-w-0 flex-1 truncate text-left">{column.label}</span>
          )}
          {column.key !== "folders" ? (
            <span
              className="absolute -right-2 top-0 h-full w-3 cursor-col-resize"
              onMouseDown={beginResize(column.key as SizedColumn)}
              title="Drag to resize"
            />
          ) : null}
        </span>
      ))}
    </div>
  );
}

function ListRow({
  item,
  isSelected,
  onSelect,
  widths,
}: {
  item: SectionItem;
  isSelected: boolean;
  onSelect: (event: React.MouseEvent) => void;
  widths: Record<SizedColumn, number>;
}) {
  return (
    <div
      className={`flex w-full cursor-grab items-center rounded-md border px-3 py-1.5 text-sm transition-colors ${
        isSelected
          ? "border-primary-ring bg-primary-surface"
          : "border-transparent hover:bg-surface-muted"
      }`}
      onClick={onSelect}
      {...dragProps(item)}
    >
      <span
        className="shrink-0 truncate text-[11px] font-semibold text-ink-muted"
        style={columnStyle("kind", widths)}
      >
        {extOf(item.fileName).toUpperCase() || extLabel(item.fileName)}
      </span>
      <span
        className="truncate text-ink"
        style={columnStyle("name", widths)}
        title={item.fileName}
      >
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
        {item.namesDiffer ? (
          <span
            className="ml-2 rounded-md bg-warning-surface px-1.5 py-0.5 text-[11px] text-warning"
            title="Copies carry different names — Move and Copy are disabled until resolved"
          >
            ≠name
          </span>
        ) : null}
      </span>
      <span
        className="shrink-0 text-right tabular-nums text-xs text-ink-muted"
        style={columnStyle("size", widths)}
      >
        {item.byteSize !== null ? formatBytes(item.byteSize) : ""}
      </span>
      <span
        className="shrink-0 truncate tabular-nums text-xs text-ink-muted"
        style={columnStyle("date", widths)}
      >
        {item.resolvedUtcMs !== null ? formatLocalMinute(item.resolvedUtcMs) : "—"}
      </span>
      {/* EVERY copy's folder, sorted (Phase 33) — one row per merged binary,
          so a single folder was an arbitrary pick. dir="rtl" keeps the deep
          end visible — the tail distinguishes folders, not the shared root. */}
      <span
        dir="rtl"
        className="truncate text-left text-xs text-ink-muted"
        style={columnStyle("folders", widths)}
        title={item.dirPaths.join("\n")}
      >
        {item.dirPaths.join(" · ")}
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
  useEffect(() => {
    const clearOnVisibilityLoss = () => {
      if (document.hidden) clearGridDragCursor();
    };
    window.addEventListener("blur", clearGridDragCursor);
    window.addEventListener("dragend", clearGridDragCursor);
    document.addEventListener("visibilitychange", clearOnVisibilityLoss);
    return () => {
      window.removeEventListener("blur", clearGridDragCursor);
      window.removeEventListener("dragend", clearGridDragCursor);
      document.removeEventListener("visibilitychange", clearOnVisibilityLoss);
      clearGridDragCursor();
    };
  }, []);

  const selectedKeys = useItemsStore((s) => s.selectedKeys);
  const selectedItem = useItemsStore((s) => s.selectedItem);
  const selectItem = useItemsStore((s) => s.selectItem);
  const toggleItem = useItemsStore((s) => s.toggleItem);
  const rangeSelect = useItemsStore((s) => s.rangeSelect);
  const lane = layout === "list" ? "other" : "media";
  const sortChoice = useItemsStore((s) =>
    lane === "other" ? s.sortOrders.other : s.sortOrders.media,
  );
  const setSortOrder = useItemsStore((s) => s.setSortOrder);
  const sortCatalogue = SORT_ORDERS[lane];

  // Other-files column widths: persisted intent, applied immediately, saved
  // debounced on change (the drag fires continuously).
  const [columnWidths, setColumnWidthsRaw] = useState(() =>
    columnWidthsFrom(useAppStore.getState().appData?.state?.otherColumnWidths),
  );
  const widthsSaveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const setColumnWidths = (widths: Record<SizedColumn, number>) => {
    setColumnWidthsRaw(widths);
    if (widthsSaveTimer.current !== null) clearTimeout(widthsSaveTimer.current);
    widthsSaveTimer.current = setTimeout(() => {
      void useAppStore.getState().patchState({ otherColumnWidths: widths });
    }, 500);
  };

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
  const sorted = emptyState === null ? sortItems(items, sortChoice) : [];
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
          value={sortChoice.order}
          onChange={(e) => setSortOrder(e.target.value as SortOrder)}
        >
          {(Object.keys(sortCatalogue.orders) as SortOrder[]).map((order) => (
            <option key={order} value={order}>
              {sortCatalogue.orders[order]}
            </option>
          ))}
        </select>
        <button
          className="h-7 rounded-md px-1.5 text-ink-muted transition-colors hover:bg-surface-muted hover:text-ink"
          title={sortChoice.desc ? "Descending — click for ascending" : "Ascending — click for descending"}
          // Re-picking the active order toggles direction (the store's rule).
          onClick={() => setSortOrder(sortChoice.order)}
        >
          {sortChoice.desc ? <ChevronDown size={14} /> : <ChevronUp size={14} />}
        </button>
      </div>
      {layout === "list" ? (
        // The table header lives OUTSIDE the scroll container, so the
        // virtualization geometry below never has to account for it.
        <ListHeader
          sort={sortChoice}
          onSort={setSortOrder}
          widths={columnWidths}
          onWidths={setColumnWidths}
        />
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
                <ListRow
                  item={item}
                  isSelected={isSelected}
                  onSelect={onSelect}
                  widths={columnWidths}
                />
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
