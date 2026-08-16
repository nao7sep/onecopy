import { useEffect, useRef, useState } from "react";
import { ChevronRight } from "lucide-react";
import { monthLabel, type SectionCounts } from "../models/sections";
import {
  branchesFor,
  buildSectionTree,
  defaultExpanded,
  visibleRows,
  type ItemKind,
  type Row,
} from "../models/sectionTree";
import { useItemsStore } from "../state/items-store";

// The left pane as ONE tree composite (the composite-control conventions):
// the container is the single tab stop, Up/Down walk the VISIBLE rows,
// Right expands or descends, Left collapses or climbs to the parent, Home/End
// jump the ends. Activating a month row opens that section; kind and year rows
// only expand, because there is no such thing as "all of 2016" to show.
//
// SECTIONS only, deliberately: Issues is a status-bar count opening a modal
// (issues are diagnostics, not something to handle), so nothing here competes
// with the months.

function rowLabel(row: Row): string {
  switch (row.type) {
    case "kind":
      return row.node.title;
    case "year":
      return row.node.year;
    case "month":
      return monthLabel(row.month);
  }
}

function rowCount(row: Row): number {
  switch (row.type) {
    case "kind":
      return row.node.count;
    case "year":
      return row.node.count;
    case "month":
      return row.count;
  }
}

export default function Sidebar({ counts }: { counts: SectionCounts | null }) {
  const selected = useItemsStore((s) => s.selected);
  const select = useItemsStore((s) => s.select);
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [expanded, setExpanded] = useState<Set<string>>(defaultExpanded);

  const tree = buildSectionTree(counts);

  // The restored (or newly chosen) section's branches open with it — a
  // selection the user cannot see would be worse than no restore at all.
  const selectedKey = selected ? `month:${selected.kind}:${selected.month}` : null;
  useEffect(() => {
    const needed = branchesFor(selected);
    if (needed.length === 0) return;
    setExpanded((current) => {
      if (needed.every((key) => current.has(key))) return current;
      const next = new Set(current);
      for (const key of needed) next.add(key);
      return next;
    });
  }, [selected]);

  const rows = visibleRows(tree, expanded);
  const keys = rows.map((r) => r.key);
  const activeIndex = selectedKey === null ? -1 : keys.indexOf(selectedKey);

  const toggle = (key: string, open?: boolean) =>
    setExpanded((current) => {
      const next = new Set(current);
      const shouldOpen = open ?? !next.has(key);
      if (shouldOpen) next.add(key);
      else next.delete(key);
      return next;
    });

  const openSection = (kind: ItemKind, month: string) => {
    void select({ kind, month });
  };

  const focusRow = (index: number) => {
    const key = keys[index];
    if (key === undefined) return;
    containerRef.current
      ?.querySelector(`[data-row-key="${CSS.escape(key)}"]`)
      ?.scrollIntoView({ block: "nearest" });
  };

  /** Activation follows focus, which is what makes Up/Down browse the library
   * rather than merely move a highlight. Expandable rows have nothing to
   * activate, so arrowing onto one only moves — it never blanks the grid. */
  const activate = (index: number) => {
    const row = rows[index];
    if (row === undefined) return;
    if (row.type === "month") openSection(row.kind, row.month);
    else toggle(row.key);
    focusRow(index);
  };

  /** Up/Down move and, on a month, open it. */
  const step = (index: number) => {
    const row = rows[index];
    if (row === undefined) return;
    if (row.type === "month") openSection(row.kind, row.month);
    focusRow(index);
  };

  const onKeyDown = (event: React.KeyboardEvent) => {
    if (keys.length === 0) return;
    const row = activeIndex >= 0 ? rows[activeIndex] : undefined;
    const isBranch = row !== undefined && row.type !== "month";
    const isOpen = row !== undefined && expanded.has(row.key);

    switch (event.key) {
      case "ArrowDown":
        event.preventDefault();
        step(Math.min(activeIndex + 1, keys.length - 1));
        return;
      case "ArrowUp":
        event.preventDefault();
        step(Math.max(activeIndex - 1, 0));
        return;
      case "PageDown":
        event.preventDefault();
        step(Math.min(activeIndex + 10, keys.length - 1));
        return;
      case "PageUp":
        event.preventDefault();
        step(Math.max(activeIndex - 10, 0));
        return;
      case "Home":
        event.preventDefault();
        step(0);
        return;
      case "End":
        event.preventDefault();
        step(keys.length - 1);
        return;
      case "ArrowRight":
        event.preventDefault();
        // Open a closed branch; on an open one, descend to its first child.
        if (isBranch && !isOpen) toggle(row.key, true);
        else if (isBranch) step(Math.min(activeIndex + 1, keys.length - 1));
        return;
      case "ArrowLeft": {
        event.preventDefault();
        // Close an open branch; otherwise climb to the enclosing one.
        if (isBranch && isOpen) {
          toggle(row.key, false);
          return;
        }
        for (let i = activeIndex - 1; i >= 0; i -= 1) {
          const candidate = rows[i];
          if (candidate !== undefined && candidate.depth < (row?.depth ?? 3)) {
            step(i);
            return;
          }
        }
        return;
      }
      case "Enter":
      case " ":
        event.preventDefault();
        activate(activeIndex >= 0 ? activeIndex : 0);
        return;
      default:
    }
  };

  const allEmpty = tree.every((node) => node.count === 0);

  return (
    <div
      ref={containerRef}
      tabIndex={0}
      role="tree"
      aria-label="Sections"
      aria-activedescendant={activeIndex >= 0 ? `section-row-${activeIndex}` : undefined}
      className="outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-primary-ring"
      onKeyDown={onKeyDown}
    >
      {rows.map((row, index) => {
        const isBranch = row.type !== "month";
        const isOpen = expanded.has(row.key);
        const isSelected = row.key === selectedKey;
        const empty = row.type === "kind" && row.node.count === 0;
        return [
          <div
            key={row.key}
            id={`section-row-${index}`}
            role="treeitem"
            aria-selected={isSelected}
            aria-expanded={isBranch ? isOpen : undefined}
            aria-level={row.depth + 1}
            data-row-key={row.key}
            style={{ paddingLeft: 6 + row.depth * 14 }}
            className={`flex cursor-pointer items-center gap-1 rounded-md py-1 pr-2 text-sm transition-colors ${
              isSelected
                ? "bg-primary-surface font-medium text-primary"
                : row.type === "kind"
                  ? "font-semibold text-ink-strong hover:bg-surface-muted"
                  : "text-ink hover:bg-surface-muted"
            }`}
            onClick={() => {
              containerRef.current?.focus();
              activate(index);
            }}
          >
            <ChevronRight
              size={13}
              aria-hidden
              className={`shrink-0 transition-transform ${
                isBranch ? (isOpen ? "rotate-90 text-ink-muted" : "text-ink-muted") : "invisible"
              }`}
            />
            <span className="min-w-0 flex-1 truncate">{rowLabel(row)}</span>
            <span className="shrink-0 text-xs tabular-nums text-ink-muted">
              {empty ? "" : rowCount(row)}
            </span>
          </div>,
          // The design's per-kind empty states, shown where the branch would
          // have been rather than as a row (there is nothing to select).
          empty && isOpen ? (
            <p
              key={`${row.key}-empty`}
              className="py-1 pr-2 text-sm text-ink-muted"
              style={{ paddingLeft: 6 + 14 + 17 }}
            >
              {row.node.emptyLabel}
            </p>
          ) : null,
        ];
      })}

      {allEmpty ? (
        <p className="mt-2 px-2 text-sm text-ink-muted">Nothing to handle</p>
      ) : null}

    </div>
  );
}
