import { X } from "lucide-react";
import type { ReactNode } from "react";

export type OperationResultLevel = "error" | "warning" | "info";

export default function OperationResult({
  level,
  children,
  actions,
  onDismiss,
  dismissLabel = "Dismiss result",
  className = "",
}: {
  level: OperationResultLevel;
  children: ReactNode;
  actions?: ReactNode;
  onDismiss?: () => void;
  dismissLabel?: string;
  className?: string;
}) {
  const tone =
    level === "error"
      ? "border-danger/40 bg-danger-surface text-danger"
      : level === "warning"
        ? "border-warning/40 bg-warning-surface text-warning"
        : "border-border bg-surface-muted text-ink";

  return (
    <div
      role={level === "error" ? "alert" : "status"}
      aria-atomic="true"
      className={`flex min-w-0 items-start gap-2 rounded-lg border px-2.5 py-2 text-xs ${tone} ${className}`}
    >
      <div className="min-w-0 flex-1 break-words">
        {children}
      </div>
      {actions !== undefined ? (
        <div className="flex shrink-0 items-center gap-2">{actions}</div>
      ) : null}
      {onDismiss !== undefined ? (
        <button
          type="button"
          aria-label={dismissLabel}
          title="Dismiss"
          className="flex h-6 w-6 shrink-0 items-center justify-center rounded-md text-current opacity-70 hover:bg-ink/10 hover:opacity-100 focus-visible:bg-ink/10 focus-visible:opacity-100"
          onClick={onDismiss}
        >
          <X aria-hidden="true" size={14} />
        </button>
      ) : null}
    </div>
  );
}
