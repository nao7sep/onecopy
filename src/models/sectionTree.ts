// The left pane's structure: kind → year → month.
//
// A flat month list does not survive a real library. At the developer's scale
// images and videos alone run to 100 months each, so the sidebar opened as 200+
// rows before anything was chosen — a list nobody can navigate and one that
// buries the three kinds it is meant to separate. Years collapse that to about
// a dozen rows, and each level carries its own total so a collapsed branch
// still says how much is inside it.
//
// Pure, and deliberately so: the tree is derived entirely from the counts the
// core already returns, so nothing here needs a query, and every rule below —
// which rows exist, what each total is, where Undated sits — is unit-testable
// without a running app.

import type { MonthSection, SectionCounts } from "./sections";

export type ItemKind = "image" | "video" | "other";

export interface YearNode {
  year: string;
  count: number;
  months: MonthSection[];
}

export interface KindNode {
  kind: ItemKind;
  title: string;
  emptyLabel: string;
  /** Every item of this kind, including undated ones. */
  count: number;
  years: YearNode[];
  /** Undated is not a year and never nests under one; it trails the kind. */
  undated: MonthSection | null;
}

/** One rendered line. `selectable` rows are sections the grid can open;
 * the rest expand and collapse. */
export type Row =
  | { type: "kind"; key: string; depth: 0; node: KindNode }
  | { type: "year"; key: string; depth: 1; kind: ItemKind; node: YearNode }
  | {
      type: "month";
      key: string;
      depth: 1 | 2;
      kind: ItemKind;
      month: string;
      count: number;
    };

const KINDS: { kind: ItemKind; title: string; emptyLabel: string }[] = [
  { kind: "image", title: "Images", emptyLabel: "No images" },
  { kind: "video", title: "Videos", emptyLabel: "No videos" },
  { kind: "other", title: "Other files", emptyLabel: "No other files" },
];

export function kindKey(kind: ItemKind): string {
  return `kind:${kind}`;
}

export function yearKey(kind: ItemKind, year: string): string {
  return `year:${kind}:${year}`;
}

export function monthKey(kind: ItemKind, month: string): string {
  return `month:${kind}:${month}`;
}

/** Groups each kind's months by their leading year, preserving the core's
 * oldest-first order and lifting Undated out to the end. */
export function buildSectionTree(counts: SectionCounts | null): KindNode[] {
  const byKind: Record<ItemKind, MonthSection[]> = {
    image: counts?.images ?? [],
    video: counts?.videos ?? [],
    other: counts?.others ?? [],
  };
  return KINDS.map(({ kind, title, emptyLabel }) => {
    const sections = byKind[kind];
    const undated = sections.find((s) => s.month === "undated") ?? null;
    const years: YearNode[] = [];
    for (const section of sections) {
      if (section.month === "undated") continue;
      const year = section.month.slice(0, 4);
      let node = years.find((y) => y.year === year);
      if (!node) {
        node = { year, count: 0, months: [] };
        years.push(node);
      }
      node.months.push(section);
      node.count += section.count;
    }
    return {
      kind,
      title,
      emptyLabel,
      // The kind's total covers undated items too — a header that excluded
      // them would disagree with the rows beneath it.
      count: years.reduce((sum, y) => sum + y.count, 0) + (undated?.count ?? 0),
      years,
      undated,
    };
  });
}

/** The rows actually on screen, in order, given which branches are open.
 *
 * This is the keyboard's world: Up/Down walk exactly this list, so a collapsed
 * year's months are not merely hidden but genuinely absent from navigation. */
export function visibleRows(tree: KindNode[], expanded: Set<string>): Row[] {
  const rows: Row[] = [];
  for (const node of tree) {
    rows.push({ type: "kind", key: kindKey(node.kind), depth: 0, node });
    if (!expanded.has(kindKey(node.kind))) continue;
    for (const year of node.years) {
      rows.push({ type: "year", key: yearKey(node.kind, year.year), depth: 1, kind: node.kind, node: year });
      if (!expanded.has(yearKey(node.kind, year.year))) continue;
      for (const month of year.months) {
        rows.push({
          type: "month",
          key: monthKey(node.kind, month.month),
          depth: 2,
          kind: node.kind,
          month: month.month,
          count: month.count,
        });
      }
    }
    if (node.undated) {
      // Depth 1: Undated is a sibling of the years, not a month within one.
      rows.push({
        type: "month",
        key: monthKey(node.kind, "undated"),
        depth: 1,
        kind: node.kind,
        month: "undated",
        count: node.undated.count,
      });
    }
  }
  return rows;
}

/** The branches that must be open for `selected` to be visible.
 *
 * The startup restore reopens the last section, so its year has to come with
 * it — otherwise the app restores a selection the user cannot see. Used as the
 * seed for the expansion set rather than persisting one, which keeps the tree
 * derived from the selection instead of drifting out of step with it. */
export function branchesFor(
  selected: { kind: ItemKind; month: string } | null,
): string[] {
  if (selected === null) return [];
  const keys = [kindKey(selected.kind)];
  if (selected.month !== "undated") {
    keys.push(yearKey(selected.kind, selected.month.slice(0, 4)));
  }
  return keys;
}

/** The kinds open by default: all three, so the app never opens looking empty.
 * Years stay CLOSED — that is the whole point of the tree. */
export function defaultExpanded(): Set<string> {
  return new Set(KINDS.map(({ kind }) => kindKey(kind)));
}
