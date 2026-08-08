import { useEffect, useState } from "react";
import "./App.css";
import { loadAppData, log, toErrorFields } from "./repositories";
import type { LoadedAppData } from "./repositories";
import { useSectionsStore } from "./state/sections-store";
import { monthLabel, type MonthSection } from "./models/sections";

// The main-window shell: live left-pane sections over the index, the plan's
// honest empty states, and the scan lifecycle in the status bar. The wizard,
// grid, and metadata panes land in their own Phase 3 steps.

function SectionList({
  title,
  sections,
  emptyLabel,
}: {
  title: string;
  sections: MonthSection[];
  emptyLabel: string;
}) {
  return (
    <section className="mb-4">
      <h2 className="mb-1 text-sm font-semibold text-ink-strong">{title}</h2>
      {sections.length === 0 ? (
        <p className="text-sm text-ink-muted">{emptyLabel}</p>
      ) : (
        <ul>
          {sections.map((section) => (
            <li
              key={section.month}
              className="flex justify-between rounded px-1 py-0.5 text-sm text-ink hover:bg-surface-muted"
            >
              <span>{monthLabel(section.month)}</span>
              <span className="text-ink-muted">{section.count}</span>
            </li>
          ))}
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
            sections={counts?.images ?? []}
            emptyLabel="No images"
          />
          <SectionList
            title="Videos"
            sections={counts?.videos ?? []}
            emptyLabel="No videos"
          />
          <SectionList
            title="Other files"
            sections={counts?.others ?? []}
            emptyLabel="No other files"
          />
        </aside>
        <main className="flex min-w-0 flex-1 items-center justify-center">
          {loadError !== null ? (
            <p className="text-danger">{loadError}</p>
          ) : allEmpty ? (
            <p className="text-ink-muted">Nothing to handle</p>
          ) : null}
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
