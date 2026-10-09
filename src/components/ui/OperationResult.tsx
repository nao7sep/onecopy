import { X } from "lucide-react";
import type { ReactNode } from "react";
import { useI18n } from "../../i18n/I18nContext";
import { IconButton } from "./Button";

export type OperationResultLevel = "error" | "warning" | "info";

export default function OperationResult({
  level,
  children,
  actions,
  onDismiss,
  dismissLabel,
  className = "",
}: {
  level: OperationResultLevel;
  children: ReactNode;
  actions?: ReactNode;
  onDismiss?: () => void;
  /** Names what is being dismissed; defaults to the shared wording of the
   * current language. */
  dismissLabel?: string;
  className?: string;
}) {
  const { t } = useI18n();
  const tone =
    level === "error"
      ? "border-danger/40 bg-danger-surface text-danger"
      : level === "warning"
        ? "border-warning/40 bg-warning-surface text-warning"
        : "border-border bg-surface-muted text-ink";
  const alignment = actions !== undefined && onDismiss === undefined
    ? "items-center"
    : "items-start";

  return (
    <div
      role={level === "error" ? "alert" : "status"}
      aria-atomic="true"
      className={`flex min-w-0 ${alignment} gap-2 rounded-lg border px-2.5 py-2 text-xs ${tone} ${className}`}
    >
      <div className="min-w-0 flex-1 break-words">
        {children}
      </div>
      {actions !== undefined ? (
        <div className="-my-1 flex min-h-6 shrink-0 items-center gap-2">
          {actions}
        </div>
      ) : null}
      {onDismiss !== undefined ? (
        <IconButton
          type="button"
          size="sm"
          tone="current"
          aria-label={dismissLabel ?? t("common.dismissResult")}
          title={t("common.dismiss")}
          className="oc-first-line-dismiss"
          onClick={onDismiss}
        >
          <X aria-hidden="true" size={14} />
        </IconButton>
      ) : null}
    </div>
  );
}
