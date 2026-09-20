// The app's button primitive.
//
// Every surface used to spell its own `rounded border border-border px-2 py-1
// text-sm …` string, which is why controls drifted into a cramped, boxy look:
// each new one copied whichever neighbour was nearest, and the paddings never
// agreed. One primitive with named variants and sizes is the root fix — a
// control's appearance becomes a choice from a small set rather than a string
// re-typed per call site.
//
// Sizes carry real touch targets (the `sm` height is 32px, `md` 36px), which
// is most of the difference between this and what was here before.

import type { ButtonHTMLAttributes } from "react";

type Variant = "primary" | "secondary" | "ghost" | "danger";
type Size = "sm" | "md";

// Every variant carries a PRESSED state distinct from its hover state. With a
// mouse, hover is already showing before the click lands, so a press with no
// separate feedback reads as "the button did nothing" until whatever it
// triggered finishes — which is exactly how a fast-but-silent action gets
// reported as laggy (developer, 2026-08-17).
//
// Off, a variant is its resting self faded — same fill, outline, ink, padding and
// footprint — so it stays recognisably the control that will come back and the
// variants stay told apart while they are off. Primary and danger used to swap
// their surfaces for neutral greys instead, which landed both on the SAME fill
// and the SAME ink: a disabled Copy and a disabled Delete became one control,
// the destructive one having lost its red and its outline at once. Ghost already
// faded, because it has no surface to swap; that is now the app's one answer.
const VARIANTS: Record<Variant, string> = {
  primary:
    "bg-primary text-ink-inverted shadow-sm hover:brightness-110 active:brightness-90 disabled:opacity-40",
  secondary:
    "border border-border bg-surface text-ink hover:bg-surface-muted hover:border-border-strong active:bg-border active:border-border-strong disabled:opacity-40",
  ghost:
    "text-ink-muted hover:bg-surface-muted hover:text-ink active:bg-border active:text-ink disabled:opacity-40",
  danger:
    "border border-danger/50 bg-danger-surface text-danger hover:border-danger hover:brightness-95 active:brightness-90 disabled:opacity-40",
};

const SIZES: Record<Size, string> = {
  sm: "h-8 rounded-lg px-3 text-sm",
  md: "h-9 rounded-lg px-4 text-sm",
};

export default function Button({
  variant = "secondary",
  size = "sm",
  className = "",
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: Variant;
  size?: Size;
}) {
  return (
    <button
      {...props}
      // `transition-colors` at 75ms, never `transition-all`: the press must
      // land immediately, and a blanket transition also animates properties
      // nobody asked for (a label change that resizes the button among them).
      className={`inline-flex shrink-0 items-center justify-center gap-1.5 font-medium transition-colors duration-75 outline-none focus-visible:ring-2 focus-visible:ring-primary-ring ${SIZES[size]} ${VARIANTS[variant]} ${className}`}
    />
  );
}
