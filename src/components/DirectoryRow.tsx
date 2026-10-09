// One configured directory in a list, with its removal control.
//
// The path and the control share ONE line: Remove is an alternative rendering
// of an ✕, and an ✕ belongs beside the thing it removes, not on a line of its
// own. The path takes the remaining width and WRAPS rather than truncating —
// source roots are long, and what distinguishes two of them is usually deep in
// the middle, exactly what an ellipsis eats.
//
// Shared by the setup wizard and the Settings modal so the two lists cannot
// drift apart.

import { X } from "lucide-react";
import { useI18n } from "../i18n/I18nContext";
import { IconButton } from "./ui/Button";

export default function DirectoryRow({
  path,
  onRemove,
}: {
  path: string;
  onRemove: () => void;
}) {
  const { t } = useI18n();
  return (
    // The row carries the text's size and line height, so the X measures its
    // first line from the same line box the path is set in.
    <div className="group flex items-start gap-2 rounded-lg border border-border bg-surface-muted/40 px-3 py-2 text-sm leading-relaxed transition-colors hover:border-border-strong">
      <p className="min-w-0 flex-1 break-all text-ink">{path}</p>
      <IconButton
        size="sm"
        tone="danger"
        aria-label={t("destinations.removeDirectory", { path })}
        title={t("common.remove")}
        className="oc-first-line-dismiss"
        onClick={onRemove}
      >
        <X size={14} />
      </IconButton>
    </div>
  );
}
