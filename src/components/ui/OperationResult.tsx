import { CircleAlert, Info, TriangleAlert } from "lucide-react";
import type { ReactNode } from "react";

export type OperationResultLevel = "error" | "warning" | "info";

export default function OperationResult({
  level,
  children,
  actions,
  className = "",
}: {
  level: OperationResultLevel;
  children: ReactNode;
  actions?: ReactNode;
  className?: string;
}) {
  const Icon =
    level === "error" ? CircleAlert : level === "warning" ? TriangleAlert : Info;
  const label =
    level === "error" ? "Error" : level === "warning" ? "Warning" : "Status";
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
      <Icon aria-hidden="true" className="mt-0.5 h-3.5 w-3.5 shrink-0" />
      <div className="min-w-0 flex-1 break-words">
        <strong>{label}:</strong> {children}
      </div>
      {actions !== undefined ? (
        <div className="flex shrink-0 items-center gap-2">{actions}</div>
      ) : null}
    </div>
  );
}
