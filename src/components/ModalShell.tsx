// The one shared modal shell (modal-dialog conventions): every app-controlled
// modal renders through this so dialog semantics, focus, stacking, scroll
// lock, and the footer dismiss exist in exactly one place.
//
// - role="dialog" + aria-modal + aria-labelledby on the surface.
// - Focus moves inside on open (first useful control, skipping the header ✕)
//   and returns to the opener on close; Tab is trapped to the surface.
// - Escape and the Tab trap act only on the TOPMOST modal (modalStack), so
//   stacked surfaces unwind one at a time, and Escape mid-IME-composition is
//   the IME's to cancel, never the modal's to close.
// - Background scroll is locked while any modal is open (reference-counted).
// - The body is the sole scroller inside a bounded height, so a 500%-zoomed
//   window still shows the header and footer.
// - The footer carries the labelled dismiss beside the primary action; the
//   header ✕ is the supplementary affordance.

import { useId, useRef } from "react";
import { useModalLayer } from "../hooks/useModalLayer";
import { X } from "lucide-react";
import Button from "./ui/Button";

export default function ModalShell({
  title,
  onClose,
  widthClass = "w-[480px]",
  closeLabel = "Close",
  closeDisabled = false,
  footerStart,
  primaryAction,
  children,
}: {
  title: string;
  onClose: () => void;
  widthClass?: string;
  closeLabel?: string;
  /** True only while leaving would interrupt an operation at an unsafe edge. */
  closeDisabled?: boolean;
  /** Left-aligned footer content (status/error text). */
  footerStart?: React.ReactNode;
  /** The primary action button(s), rendered to the right of the dismiss. */
  primaryAction?: React.ReactNode;
  children: React.ReactNode;
}) {
  const titleId = useId();
  const surfaceRef = useRef<HTMLDivElement>(null);
  useModalLayer(surfaceRef, onClose, closeDisabled);

  return (
    <div className="fixed inset-0 z-30 flex items-center justify-center bg-background/80">
      <div
        ref={surfaceRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
        className={`flex max-h-[90vh] ${widthClass} max-w-[90vw] flex-col rounded-2xl border border-border bg-surface shadow-xl`}
      >
        <div className="flex shrink-0 items-center justify-between gap-4 px-5 pb-3 pt-4">
          <h1 id={titleId} className="text-base font-semibold tracking-tight text-ink-strong">
            {title}
          </h1>
          <button
            data-modal-close
            aria-label="Close"
            disabled={closeDisabled}
            className="flex h-7 w-7 shrink-0 items-center justify-center rounded-md text-ink-muted transition-colors hover:bg-surface-muted hover:text-ink disabled:cursor-default disabled:opacity-40"
            onClick={onClose}
          >
            <X size={15} />
          </button>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto px-5 pb-1">{children}</div>
        <div className="flex shrink-0 items-center gap-2 px-5 pb-4 pt-4">
          <span className="min-w-0 flex-1 truncate text-xs text-danger">{footerStart}</span>
          <Button data-modal-close disabled={closeDisabled} onClick={onClose}>
            {closeLabel}
          </Button>
          {primaryAction}
        </div>
      </div>
    </div>
  );
}
