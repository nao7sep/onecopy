import { useEffect, useRef, useState } from "react";
import { X } from "lucide-react";
import { useIssuesStore } from "../state/issues-store";
import { formatLocalMinute } from "../utils/displayTime";
import ModalShell from "./ModalShell";
import Button from "./ui/Button";
import OperationResult from "./ui/OperationResult";
import { revealInMain } from "../workflows/reveal-in-main";

export default function IssuesModal({ open, onClose }: {
  open: boolean;
  onClose: () => void;
}) {
  const { rows, total, loading, error, load, dismiss, dismissAll } = useIssuesStore();
  const request = useRef(0);
  const revealed = useRef(false);
  const [revealResult, setRevealResult] = useState<{ level: "info" | "error"; text: string } | null>(null);
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
    setRevealResult({ level: "info", text: "Locating file…" });
    const result = await revealInMain(path, () => request.current === id, () => {
      revealed.current = true;
      close();
    });
    if (request.current !== id) return;
    if (result !== "revealed") {
      setRevealResult({ level: result === "failed" ? "error" : "info", text:
        result === "failed" ? "Couldn’t reveal this file in Main."
        : result === "blocked" ? "Finish the current file operation or confirmation before revealing this file."
        : result === "superseded" ? "Main changed. Choose the file again to reveal it."
        : "This file is not available in Main. It may have been removed or be outside the current library." });
    }
  };
  if (!open) return null;
  const footer = total > rows.length
    ? `Showing the oldest ${rows.length} of ${total}` : undefined;

  return (
    <ModalShell
      title="Issues"
      onClose={close}
      returnFocus={() => revealed.current ? document.getElementById("main-item-area") : null}
      widthClass="w-[min(820px,calc(100vw-3rem))]"
      footerStart={footer === undefined ? undefined : (
        <span className="text-xs text-ink-muted">{footer}</span>
      )}
      footerResult={revealResult !== null ? (
        <OperationResult level={revealResult.level}>{revealResult.text}</OperationResult>
      ) : error !== null ? <OperationResult level="error">{error}</OperationResult> : undefined}
      primaryAction={total > 0 ? (
        <Button variant="danger" onClick={() => void dismissAll()}>Dismiss all</Button>
      ) : undefined}
    >
      <p className="mb-3 text-sm text-ink-muted">
        Issues from this app session. Dismissed entries remain in diagnostic history.
      </p>
      {rows.length === 0 ? (
        error !== null ? null : (
          <p className="py-6 text-center text-sm text-ink-muted">
            {loading ? "Loading issues…" : "No issues"}
          </p>
        )
      ) : (
        <ul className="space-y-1.5">
          {rows.map((row) => (
            <li key={row.id} className="rounded-lg border border-border bg-surface p-3 text-xs">
              <div className="flex items-start justify-between gap-3">
                <span className="flex min-w-0 flex-wrap items-center gap-2 text-ink-muted">
                  <span title={`Last seen ${formatLocalMinute(row.lastSeenUtc)}`}>
                    {formatLocalMinute(row.firstSeenUtc)}
                  </span>
                  {row.occurrenceCount > 1 ? <span>×{row.occurrenceCount}</span> : null}
                </span>
                <button
                  aria-label="Dismiss"
                  title="Dismiss"
                  className="flex h-5 w-5 shrink-0 items-center justify-center rounded text-ink-muted transition-colors hover:bg-danger-surface hover:text-danger"
                  onClick={() => void dismiss(row.id)}
                ><X size={12} /></button>
              </div>
              {row.path ? <button className="mt-2 w-full select-text break-all text-left text-primary underline decoration-primary/40 underline-offset-2 hover:decoration-primary focus-visible:outline focus-visible:outline-2 focus-visible:outline-primary-ring"
                title="Reveal in Main" onClick={() => void reveal(row.path!)}>{row.path}</button> : null}
              {row.message ? <div className="mt-1.5 select-text break-words leading-relaxed text-ink-muted">{row.message}</div> : null}
            </li>
          ))}
        </ul>
      )}
    </ModalShell>
  );
}
