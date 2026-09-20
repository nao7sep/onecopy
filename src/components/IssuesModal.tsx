import { useEffect, useRef, useState } from "react";
import { conditionKey, conditionText } from "../models/noticeConditions";
import { createTranslator } from "../i18n/translate";
import { X } from "lucide-react";
import { useIssuesStore, type IssueRow } from "../state/issues-store";
import { formatLocalMinute } from "../utils/displayTime";
import { useDisplayZone } from "../hooks/useDisplayZone";
import ModalShell from "./ModalShell";
import Button from "./ui/Button";
import OperationResult from "./ui/OperationResult";
import { revealInMain } from "../workflows/reveal-in-main";
import { useI18n } from "../i18n/I18nContext";
import type { MessageKey } from "../i18n/catalogues";

// What the core wrote down, when it is not simply the English of the sentence
// the row already shows: a system error, a count, a path it could not read.
const english = createTranslator("en");

function recordedDetail(row: IssueRow): string | null {
  if (row.message === null || row.message.trim() === "") return null;
  const key = conditionKey(row.kind);
  if (key === null) return null;
  return row.message === english.t(key) ? null : row.message;
}

export default function IssuesModal({ open, onClose }: {
  open: boolean;
  onClose: () => void;
}) {
  useDisplayZone();
  const { t, text, dateTime } = useI18n();
  const { rows, total, loading, error, load, dismiss, dismissAll } = useIssuesStore();
  const request = useRef(0);
  const revealed = useRef(false);
  // The key, not a finished sentence, so the message follows a language change.
  const [revealResult, setRevealResult] = useState<{ level: "info" | "error"; message: MessageKey } | null>(null);
  useEffect(() => {
    if (open) {
      revealed.current = false;
      setRevealResult(null);
      void load();
    }
    return () => { request.current += 1; };
  }, [open, load]);
  const close = () => {
    request.current += 1;
    onClose();
  };
  const reveal = async (path: string) => {
    const id = ++request.current;
    setRevealResult({ level: "info", message: "issues.locating" });
    const result = await revealInMain(path, () => request.current === id, () => {
      revealed.current = true;
      close();
    });
    if (request.current !== id) return;
    if (result !== "revealed") {
      setRevealResult({ level: result === "failed" ? "error" : "info", message:
        result === "failed" ? "reveal.inMainFailed"
        : result === "blocked" ? "reveal.inMainBlocked"
        : result === "superseded" ? "issues.revealSuperseded"
        : "reveal.inMainUnavailable" });
    }
  };
  if (!open) return null;
  const footer = total > rows.length
    ? t("issues.showingOldest", { count: rows.length, total }) : undefined;

  return (
    <ModalShell
      title={t("issues.title")}
      onClose={close}
      returnFocus={() => revealed.current ? document.getElementById("main-item-area") : null}
      widthClass="w-[min(820px,calc(100vw-3rem))]"
      footerStart={footer === undefined ? undefined : (
        <span className="text-xs text-ink-muted">{footer}</span>
      )}
      footerResult={revealResult !== null ? (
        <OperationResult level={revealResult.level}>{t(revealResult.message)}</OperationResult>
      ) : error !== null ? <OperationResult level="error">{text(error)}</OperationResult> : undefined}
      primaryAction={total > 0 ? (
        <Button onClick={() => void dismissAll()}>{t("issues.dismissAll")}</Button>
      ) : undefined}
    >
      <p className="mb-3 text-sm text-ink-muted">
        {t("issues.intro")}
      </p>
      {rows.length === 0 ? (
        error !== null ? null : (
          <p className="py-6 text-center text-sm text-ink-muted">
            {loading ? t("issues.loading") : t("issues.none")}
          </p>
        )
      ) : (
        <ul className="space-y-1.5">
          {rows.map((row) => (
            <li key={row.id} className="rounded-lg border border-border bg-surface p-3 text-xs">
              <div className="flex items-start justify-between gap-3">
                <span className="flex min-w-0 flex-wrap items-center gap-2 text-ink-muted">
                  <span title={t("issues.lastSeen", { time: formatLocalMinute(row.lastSeenUtc, dateTime) })}>
                    {formatLocalMinute(row.firstSeenUtc, dateTime)}
                  </span>
                  {row.occurrenceCount > 1
                    ? <span>{t("issues.occurrences", { count: row.occurrenceCount })}</span>
                    : null}
                </span>
                <button
                  aria-label={t("issues.dismiss")}
                  title={t("issues.dismiss")}
                  className="flex h-5 w-5 shrink-0 items-center justify-center rounded text-ink-muted transition-colors hover:bg-danger-surface hover:text-danger"
                  onClick={() => void dismiss(row.id)}
                ><X size={12} /></button>
              </div>
              {row.path ? <button className="mt-2 w-full select-text break-all text-left text-primary underline decoration-primary/40 underline-offset-2 hover:decoration-primary focus-visible:outline focus-visible:outline-2 focus-visible:outline-primary-ring"
                title={t("issues.revealInMain")} onClick={() => void reveal(row.path!)}>{row.path}</button> : null}
              {/* The condition in the reader's language, then what the core
                  recorded when that says something the sentence does not. */}
              <div className="mt-1.5 select-text break-words leading-relaxed text-ink-muted">
                {conditionText(row.kind, row.message, t)}
              </div>
              {recordedDetail(row) !== null ? (
                <div className="mt-1 select-text break-words text-xs leading-relaxed text-ink-muted opacity-80">
                  {recordedDetail(row)}
                </div>
              ) : null}
            </li>
          ))}
        </ul>
      )}
    </ModalShell>
  );
}
