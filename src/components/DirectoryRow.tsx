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

export default function DirectoryRow({
  path,
  onRemove,
}: {
  path: string;
  onRemove: () => void;
}) {
  return (
    <div className="group flex items-start gap-2 rounded-lg border border-border bg-surface-muted/40 px-3 py-2 transition-colors hover:border-border-strong">
      <p className="min-w-0 flex-1 break-all text-sm leading-relaxed text-ink">{path}</p>
      <button
        aria-label={`Remove ${path}`}
        title="Remove"
        className="mt-0.5 flex h-6 w-6 shrink-0 items-center justify-center rounded-md text-ink-muted transition-colors hover:bg-danger-surface hover:text-danger"
        onClick={onRemove}
      >
        <X size={14} />
      </button>
    </div>
  );
}
