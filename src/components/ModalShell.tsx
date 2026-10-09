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
import { useI18n } from "../i18n/I18nContext";
import { useModalLayer } from "../hooks/useModalLayer";
import { X } from "lucide-react";
import Button, { IconButton } from "./ui/Button";

export default function ModalShell({
  title,
  onClose,
  widthClass = "w-[480px]",
  closeLabel,
  closeDisabled = false,
  initialFocus = "auto",
  footerArrowNavigation = false,
  returnFocus,
  footerStart,
  footerResult,
  primaryAction,
  hideTitle = false,
  children,
}: {
  title: string;
  onClose: () => void;
  widthClass?: string;
  /** Defaults to the shared "Close" wording of the current language. */
  closeLabel?: string;
  /** True only while leaving would interrupt an operation at an unsafe edge. */
  closeDisabled?: boolean;
  initialFocus?: "auto" | "surface" | "close";
  footerArrowNavigation?: boolean;
  /** An explicit successful navigation may return to its destination instead of the opener. */
  returnFocus?: () => HTMLElement | null;
  /** Short left-aligned metadata, sharing the actions' first text baseline. */
  footerStart?: React.ReactNode;
  /** A wrapping operation result, in its own band above footer actions. */
  footerResult?: React.ReactNode;
  /** The primary action button(s), rendered to the right of the dismiss. */
  primaryAction?: React.ReactNode;
  /** True only where the body opens by naming what the title would say
   * (About): the header keeps the title for assistive technology, drops its
   * visible text and its closing line, and keeps the close X in its corner
   * (modal-dialog-conventions, "Every modal…"). */
  hideTitle?: boolean;
  children: React.ReactNode;
}) {
  const { t } = useI18n();
  const titleId = useId();
  const surfaceRef = useRef<HTMLDivElement>(null);
  useModalLayer(surfaceRef, onClose, closeDisabled, footerArrowNavigation, returnFocus);

  return (
    <div className="fixed inset-0 z-30 flex items-center justify-center bg-background/80">
      <div
        ref={surfaceRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
        data-modal-initial-focus={initialFocus === "surface" ? true : undefined}
        className={`flex max-h-[90vh] ${widthClass} max-w-[90vw] flex-col rounded-2xl border border-border bg-surface shadow-xl`}
      >
        {/* Equal room above and below, so the title and the close X sit on
            the band's centre line. */}
        <div
          className={`flex shrink-0 items-center gap-4 px-5 py-3 ${
            hideTitle ? "justify-end" : "justify-between border-b border-control-edge"
          }`}
        >
          <h1
            id={titleId}
            className={hideTitle ? "sr-only" : "text-base font-semibold tracking-tight text-ink-strong"}
          >
            {title}
          </h1>
          <IconButton
            data-modal-close
            aria-label={t("common.close")}
            disabled={closeDisabled}
            onClick={onClose}
          >
            <X size={16} />
          </IconButton>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto px-5 py-4">{children}</div>
        <div className="shrink-0 space-y-3 border-t border-control-edge px-5 py-3">
          {footerResult === undefined ? null : <div className="min-w-0 break-words">{footerResult}</div>}
          <div className="flex flex-wrap items-baseline gap-x-4 gap-y-3">
            {footerStart === undefined ? null : (
              <div className="min-w-0 flex-1 basis-48 break-words">{footerStart}</div>
            )}
            {/* The buttons are one height and centre on each other, so an
                icon in one never shifts it against a label-only neighbour;
                the row's own baseline is the first button's label, which the
                metadata beside it shares. */}
            <div data-modal-actions className="ml-auto flex max-w-full flex-wrap items-center justify-end gap-2">
              {/* Arrow keys move focus between these programmatically, so the
                  ring follows any focus here rather than only focus-visible. */}
              <Button data-modal-close data-modal-initial-focus={initialFocus === "close" ? true : undefined}
                className={footerArrowNavigation ? "focus:outline-2 focus:outline-offset-1 focus:outline-focus-ring" : ""}
                disabled={closeDisabled} onClick={onClose}>
                {closeLabel ?? t("common.close")}
              </Button>
              {primaryAction}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
