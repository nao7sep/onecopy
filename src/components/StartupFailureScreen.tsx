import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect } from "react";
import type { StartupFailure } from "../repositories";
import { reportWindowCall } from "../repositories";

interface StartupFailureScreenProps {
  failure: StartupFailure;
}

/** The application-owned terminal bootstrap state. The webview is healthy,
 * but backend work is gated off because required application data is not. */
export default function StartupFailureScreen({ failure }: StartupFailureScreenProps) {
  useEffect(() => {
    const appWindow = getCurrentWindow();
    void appWindow
      .show()
      .then(() => appWindow.setFocus())
      .catch(reportWindowCall("show blocked startup"));
  }, []);

  const quit = () => {
    void getCurrentWindow().close().catch(reportWindowCall("startup quit"));
  };

  return (
    <div
      role="alertdialog"
      aria-modal="true"
      aria-labelledby="startup-failure-title"
      aria-describedby="startup-failure-message"
      className="fixed inset-0 z-[1000] flex items-center justify-center bg-background p-8 text-ink"
    >
      <div className="w-full max-w-lg rounded-xl border border-border bg-surface p-8 shadow-xl">
        <h1 id="startup-failure-title" className="text-xl font-semibold text-ink-strong">
          {failure.title}
        </h1>
        <p id="startup-failure-message" className="mt-3 leading-relaxed text-ink-muted">
          {failure.message}
        </p>
        <div className="mt-6 flex justify-end">
          <button
            type="button"
            className="inline-flex h-9 items-center justify-center rounded-lg bg-primary px-4 text-sm font-medium text-ink-inverted outline-none hover:brightness-110 focus-visible:ring-2 focus-visible:ring-primary-ring"
            onClick={quit}
            autoFocus
          >
            Quit OneCopy
          </button>
        </div>
      </div>
    </div>
  );
}
