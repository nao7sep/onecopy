import { useEffect, useState } from "react";
import "./App.css";
import { loadAppData, log, toErrorFields } from "./repositories";
import type { LoadedAppData } from "./repositories";

// The Phase-1 shell: loads app data from the core and renders the main-window
// skeleton with the plan's honest empty states. The wizard, scanner, and real
// section lists replace the placeholders in their own phases.

interface Section {
  title: string;
  emptyLabel: string;
}

const SECTIONS: Section[] = [
  { title: "Images", emptyLabel: "No images" },
  { title: "Videos", emptyLabel: "No videos" },
  { title: "Other files", emptyLabel: "No other files" },
];

export default function App() {
  const [appData, setAppData] = useState<LoadedAppData | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);

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
  }, []);

  return (
    <div className="flex h-screen flex-col bg-background text-ink">
      <div className="flex min-h-0 flex-1">
        <aside className="w-64 shrink-0 overflow-y-auto border-r border-border bg-surface p-3">
          {SECTIONS.map((section) => (
            <section key={section.title} className="mb-4">
              <h2 className="mb-1 text-sm font-semibold text-ink-strong">
                {section.title}
              </h2>
              <p className="text-sm text-ink-muted">{section.emptyLabel}</p>
            </section>
          ))}
        </aside>
        <main className="flex min-w-0 flex-1 items-center justify-center">
          {loadError !== null ? (
            <p className="text-danger">{loadError}</p>
          ) : (
            <p className="text-ink-muted">Nothing to handle</p>
          )}
        </main>
      </div>
      <footer className="flex shrink-0 items-center justify-between border-t border-border bg-surface px-3 py-1 text-xs text-ink-muted">
        <span>OneCopy {__APP_VERSION__}</span>
        <span>{appData?.dataRoot ?? ""}</span>
      </footer>
    </div>
  );
}
