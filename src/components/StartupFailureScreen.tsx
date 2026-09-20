import { getCurrentWindow } from "@tauri-apps/api/window";
import { useI18n } from "../i18n/I18nContext";
import { reportWindowCall } from "../repositories";

/** The application-owned terminal bootstrap state. The webview is healthy, but
 * backend work is gated off because required application data is not. There is
 * one such condition, so the screen says it in the reader's language; the
 * core's own diagnostic stays in the session log. */
export default function StartupFailureScreen() {
  const { t } = useI18n();
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
        {/* One condition, one pair of sentences: the screen says them in the
            reader's language, and the core's own words stay in the log. */}
        <h1 id="startup-failure-title" className="text-xl font-semibold text-ink-strong">
          {t("startup.blockedTitle")}
        </h1>
        <p id="startup-failure-message" className="mt-3 leading-relaxed text-ink-muted">
          {t("startup.blockedBody")}
        </p>
        <div className="mt-6 flex justify-end">
          <button
            type="button"
            className="inline-flex h-9 items-center justify-center rounded-lg bg-primary px-4 text-sm font-medium text-ink-inverted outline-none hover:brightness-110 focus-visible:ring-2 focus-visible:ring-primary-ring"
            onClick={quit}
            autoFocus
          >
            {t("startup.quit")}
          </button>
        </div>
      </div>
    </div>
  );
}
