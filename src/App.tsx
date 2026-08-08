import { useEffect, useState } from "react";
import "./App.css";
import { loadAppData, log, toErrorFields } from "./repositories";
import type { LoadedAppData } from "./repositories";
import { useSectionsStore } from "./state/sections-store";
import { useItemsStore, type SelectedSection } from "./state/items-store";
import { monthLabel, type MonthSection } from "./models/sections";
import Grid from "./components/Grid";

// The main-window shell: live left-pane sections, the thumbnail grid for the
// selected section, and the scan lifecycle in the status bar. The wizard,
// metadata pane, and preview window land in their own Phase 3 steps.

function SectionList({
  title,
  kind,
  sections,
  emptyLabel,
}: {
  title: string;
  kind: SelectedSection["kind"];
  sections: MonthSection[];
  emptyLabel: string;
}) {
  const selected = useItemsStore((s) => s.selected);
  const select = useItemsStore((s) => s.select);
  return (
    <section className="mb-4">
      <h2 className="mb-1 text-sm font-semibold text-ink-strong">{title}</h2>
      {sections.length === 0 ? (
        <p className="text-sm text-ink-muted">{emptyLabel}</p>
      ) : (
        <ul>
          {sections.map((section) => {
            const isSelected =
              selected?.kind === kind && selected.month === section.month;
            return (
              <li key={section.month}>
                <button
                  className={`flex w-full justify-between rounded px-1 py-0.5 text-left text-sm ${
                    isSelected
                      ? "bg-primary-surface text-primary"
                      : "text-ink hover:bg-surface-muted"
                  }`}
                  onClick={() => void select({ kind, month: section.month })}
                >
                  <span>{monthLabel(section.month)}</span>
                  <span className="text-ink-muted">{section.count}</span>
                </button>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}

export default function App() {
  const [appData, setAppData] = useState<LoadedAppData | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const counts = useSectionsStore((s) => s.counts);
  const scanning = useSectionsStore((s) => s.scanning);
  const progress = useSectionsStore((s) => s.progress);
  const loadCounts = useSectionsStore((s) => s.loadCounts);
  const startScan = useSectionsStore((s) => s.startScan);
  const selected = useItemsStore((s) => s.selected);
  const items = useItemsStore((s) => s.items);
  const itemsLoading = useItemsStore((s) => s.loading);

  // Delete/Backspace trash-deletes the selected logical item (every copy);
  // Shift makes it permanent. Ignored while typing in a form control.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Delete" && event.key !== "Backspace") return;
      const target = event.target as HTMLElement | null;
      if (
        target instanceof HTMLInputElement ||
        target instanceof HTMLTextAreaElement ||
        target?.isContentEditable
      ) {
        return;
      }
      event.preventDefault();
      void useItemsStore.getState().deleteSelected(event.shiftKey);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  useEffect(() => {
    loadAppData()
      .then((data) => {
        setAppData(data);
        log.info("app data loaded", {
          dataRoot: data.dataRoot,
          hasConfig: data.config !== null,
          hasState: data.state !== null,
        });
      })
      .catch((error) => {
        setLoadError(String(error));
        log.error("app data load failed", toErrorFields(error));
      });
    void loadCounts();
  }, [loadCounts]);

  const allEmpty =
    counts !== null &&
    counts.images.length === 0 &&
    counts.videos.length === 0 &&
    counts.others.length === 0;

  return (
    <div className="flex h-screen flex-col bg-background text-ink">
      <div className="flex min-h-0 flex-1">
        <aside className="w-64 shrink-0 overflow-y-auto border-r border-border bg-surface p-3">
          <SectionList
            title="Images"
            kind="image"
            sections={counts?.images ?? []}
            emptyLabel="No images"
          />
          <SectionList
            title="Videos"
            kind="video"
            sections={counts?.videos ?? []}
            emptyLabel="No videos"
          />
          <SectionList
            title="Other files"
            kind="other"
            sections={counts?.others ?? []}
            emptyLabel="No other files"
          />
        </aside>
        <main className="flex min-w-0 flex-1 flex-col overflow-hidden">
          {loadError !== null ? (
            <p className="m-auto text-danger">{loadError}</p>
          ) : selected !== null ? (
            <Grid items={items} loading={itemsLoading} />
          ) : allEmpty ? (
            <p className="m-auto text-ink-muted">Nothing to handle</p>
          ) : (
            <p className="m-auto text-ink-muted">Select a month</p>
          )}
        </main>
      </div>
      <footer className="flex shrink-0 items-center justify-between border-t border-border bg-surface px-3 py-1 text-xs text-ink-muted">
        <span>OneCopy {__APP_VERSION__}</span>
        <span>{scanning ? progress : ""}</span>
        <span className="flex items-center gap-3">
          <button
            className="text-primary hover:text-primary-hover disabled:text-ink-muted"
            disabled={scanning}
            onClick={() => void startScan()}
          >
            {scanning ? "Scanning…" : "Scan"}
          </button>
          <span>{appData?.dataRoot ?? ""}</span>
        </span>
      </footer>
    </div>
  );
}
