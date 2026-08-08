import { useRef } from "react";
import { monthLabel, type MonthSection, type SectionCounts } from "../models/sections";
import { useItemsStore, type SelectedSection } from "../state/items-store";

// The left pane's month navigation as ONE composite listbox (the
// composite-control conventions): the container is the single tab stop,
// Up/Down flow continuously across the three groups (headers are
// non-interactive labels), Home/End jump the ends, and activation follows
// focus — selecting a month is a cheap indexed query. Click mirrors the
// keyboard through the same source of truth (items-store.selected).

interface FlatEntry {
  kind: SelectedSection["kind"];
  month: string;
  count: number;
}

function flatten(counts: SectionCounts | null): {
  entries: FlatEntry[];
  groups: { title: string; kind: SelectedSection["kind"]; sections: MonthSection[]; emptyLabel: string }[];
} {
  const groups = [
    { title: "Images", kind: "image" as const, sections: counts?.images ?? [], emptyLabel: "No images" },
    { title: "Videos", kind: "video" as const, sections: counts?.videos ?? [], emptyLabel: "No videos" },
    { title: "Other files", kind: "other" as const, sections: counts?.others ?? [], emptyLabel: "No other files" },
  ];
  const entries: FlatEntry[] = groups.flatMap((group) =>
    group.sections.map((section) => ({
      kind: group.kind,
      month: section.month,
      count: section.count,
    })),
  );
  return { entries, groups };
}

export default function Sidebar({ counts }: { counts: SectionCounts | null }) {
  const selected = useItemsStore((s) => s.selected);
  const select = useItemsStore((s) => s.select);
  const containerRef = useRef<HTMLDivElement | null>(null);
  const { entries, groups } = flatten(counts);

  const activeIndex = selected
    ? entries.findIndex((e) => e.kind === selected.kind && e.month === selected.month)
    : -1;

  const activate = (index: number) => {
    const entry = entries[index];
    if (!entry) return;
    void select({ kind: entry.kind, month: entry.month });
    containerRef.current
      ?.querySelector(`[data-entry-index="${index}"]`)
      ?.scrollIntoView({ block: "nearest" });
  };

  const onKeyDown = (event: React.KeyboardEvent) => {
    if (entries.length === 0) return;
    const target =
      event.key === "ArrowDown"
        ? Math.min(activeIndex + 1, entries.length - 1)
        : event.key === "ArrowUp"
          ? Math.max(activeIndex - 1, 0)
          : event.key === "Home"
            ? 0
            : event.key === "End"
              ? entries.length - 1
              : null;
    if (target === null) return;
    event.preventDefault();
    activate(target);
  };

  let flatIndex = 0;
  return (
    <div
      ref={containerRef}
      tabIndex={0}
      role="listbox"
      aria-label="Sections"
      className="outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-primary-ring"
      onKeyDown={onKeyDown}
    >
      {groups.map((group) => (
        <section key={group.kind} className="mb-4" role="group" aria-label={group.title}>
          <h2 className="mb-1 text-sm font-semibold text-ink-strong">{group.title}</h2>
          {group.sections.length === 0 ? (
            <p className="text-sm text-ink-muted">{group.emptyLabel}</p>
          ) : (
            <ul>
              {group.sections.map((section) => {
                const index = flatIndex;
                flatIndex += 1;
                const isSelected =
                  selected?.kind === group.kind && selected.month === section.month;
                return (
                  <li key={section.month}>
                    <div
                      role="option"
                      aria-selected={isSelected}
                      data-entry-index={index}
                      className={`flex w-full cursor-default justify-between rounded px-1 py-0.5 text-sm ${
                        isSelected
                          ? "bg-primary-surface text-primary"
                          : "text-ink hover:bg-surface-muted"
                      }`}
                      onClick={() => {
                        containerRef.current?.focus();
                        activate(index);
                      }}
                    >
                      <span>{monthLabel(section.month)}</span>
                      <span className="text-ink-muted">{section.count}</span>
                    </div>
                  </li>
                );
              })}
            </ul>
          )}
        </section>
      ))}
    </div>
  );
}
