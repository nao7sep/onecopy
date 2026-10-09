// The app's button primitives: a labelled Button, an icon-only IconButton and
// an inline LinkButton.
//
// One primitive with named variants and sizes is what keeps controls from
// drifting apart: a control's appearance is a choice from a small set rather
// than a class string re-typed per call site, where each new one copies
// whichever neighbour is nearest and the paddings never agree.
//
// Sizes carry real touch targets: `sm` is the standard 32px, `md` 36px,
// `toolbar` 28px for a dense bar of text actions (a pane's toolbar, a
// destination's action row), and `xs` 24px only for actions inside an
// operation result.
//
// Focus needs nothing here: App.css gives every button the app's one focus
// ring, 1px clear of the edge, which quiets while the window is inactive.

import type { ButtonHTMLAttributes } from "react";

type Variant = "primary" | "secondary" | "ghost" | "danger" | "danger-solid";
type Size = "xs" | "toolbar" | "sm" | "md";

/** How every control recedes when it is off: the resting control, faded.
 * Fields, switches and every button role use this one value. */
export const DISABLED_FADE = "disabled:opacity-50";

// Every variant carries a PRESSED state distinct from its hover state. With a
// mouse, hover is already showing before the click lands, so a press with no
// separate feedback reads as "the button did nothing" until whatever it
// triggered finishes — which is exactly how a fast-but-silent action gets
// reported as laggy (developer, 2026-08-17). Hover and press step the
// variant's own surface in one direction: the filled roles deepen in the
// light theme and lift in the dark one (App.css owns those ladders), and the
// neutral ones go surface → surface-muted → surface-pressed. Hover and press
// are `enabled:` only, because a disabled button still matches :hover.
//
// Off, a variant is its resting self faded — same fill, outline, ink, padding
// and footprint — so it stays recognisably the control that will come back,
// and a disabled Copy and a disabled Delete stay two different controls, the
// destructive one keeping its red and its outline.
const VARIANTS: Record<Variant, string> = {
  primary:
    "bg-primary text-ink-inverted shadow-sm enabled:hover:bg-primary-hover enabled:active:bg-primary-pressed disabled:opacity-50",
  secondary:
    "border border-control-edge bg-surface text-ink enabled:hover:bg-surface-muted enabled:active:bg-surface-pressed disabled:opacity-50",
  ghost:
    "text-ink-muted enabled:hover:bg-surface-muted enabled:hover:text-ink enabled:active:bg-surface-pressed enabled:active:text-ink disabled:opacity-50",
  danger:
    "border border-danger/50 bg-danger-surface text-danger enabled:hover:border-danger enabled:hover:brightness-95 enabled:active:border-danger enabled:active:brightness-90 disabled:opacity-50",
  // The filled "commit" style (interface-styling-conventions): reserved for a
  // single click that IS the irreversible action itself, never a trigger that
  // merely opens a confirming step — `danger` above stays the trigger style
  // (R8-03).
  "danger-solid":
    "bg-danger-solid text-ink-inverted shadow-sm enabled:hover:bg-danger-solid-hover enabled:active:bg-danger-solid-pressed disabled:opacity-50",
};

// `xs` is the compact role for an action inside an operation result, whose
// text is 12px: the result's own line sets that height, so the action takes a
// size of its own rather than being squeezed by the banner it sits in.
const SIZES: Record<Size, string> = {
  xs: "h-6 rounded-md px-2 text-xs",
  toolbar: "h-7 rounded-md px-2.5 text-xs",
  sm: "h-8 rounded-lg px-3 text-sm",
  md: "h-9 rounded-lg px-4 text-sm",
};

// `transition-colors` at 75ms, never `transition-all`: the press must land
// immediately, and a blanket transition also animates properties nobody asked
// for (a label change that resizes the button among them).
const MOTION = "transition-colors duration-75 motion-reduce:transition-none";

// A toggle that is on (a pressed toggle, a chosen segment) wears the
// selection fill in place of its variant's resting look, as a selected list
// row does, and still steps under the pointer.
const SELECTED =
  "bg-primary-surface text-primary enabled:hover:brightness-95 enabled:active:brightness-90";

export default function Button({
  variant = "secondary",
  size = "sm",
  selected = false,
  className = "",
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: Variant;
  size?: Size;
  /** Shows a toggle as on; the caller still states `aria-pressed`. */
  selected?: boolean;
}) {
  return (
    <button
      {...props}
      // A caption stays whole: the button fits its label rather than
      // wrapping it.
      className={`inline-flex shrink-0 items-center justify-center gap-1.5 whitespace-nowrap font-medium ${MOTION} ${SIZES[size]} ${
        selected ? `${SELECTED} ${DISABLED_FADE}` : VARIANTS[variant]
      } ${className}`}
    />
  );
}

type IconSize = "sm" | "md";
type IconTone = "neutral" | "danger" | "current" | "overlay";

// Two sizes, one corner: 24px inside a row, a result or a compact bar, 32px
// where the control stands on its own (a title band's close, the app menu).
const ICON_SIZES: Record<IconSize, string> = {
  sm: "h-6 w-6",
  md: "h-8 w-8",
};

// No chrome at rest; hover and press reveal the hit area as steps of the
// surface it sits on. `danger` is a compact remove inside a list row: neutral
// at rest, red only under the pointer, because a list dotted with red marks
// reads as a list of errors. `current` sits inside a tinted result or notice
// and keeps its ink. `overlay` sits on the fullscreen view's black bar.
const ICON_TONES: Record<IconTone, string> = {
  neutral:
    "text-ink-muted enabled:hover:bg-surface-muted enabled:hover:text-ink enabled:active:bg-surface-pressed enabled:active:text-ink",
  danger:
    "text-ink-muted enabled:hover:bg-danger-surface enabled:hover:text-danger enabled:active:bg-danger-surface enabled:active:text-danger enabled:active:brightness-90",
  current:
    "text-current opacity-70 enabled:hover:bg-ink/10 enabled:hover:opacity-100 enabled:active:bg-ink/15 enabled:active:opacity-100 focus-visible:opacity-100",
  overlay:
    "text-white enabled:hover:bg-white/15 enabled:active:bg-white/25",
};

/** An icon-only button. It has no visible words, so callers give it an
 * accessible name (`aria-label`) and usually a matching `title`. */
export function IconButton({
  size = "md",
  tone = "neutral",
  selected = false,
  className = "",
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & {
  size?: IconSize;
  tone?: IconTone;
  /** Shows a toggle or a chosen segment as on; the caller states its ARIA. */
  selected?: boolean;
}) {
  return (
    <button
      {...props}
      className={`inline-flex shrink-0 items-center justify-center rounded-md ${MOTION} ${ICON_SIZES[size]} ${
        selected ? SELECTED : ICON_TONES[tone]
      } ${DISABLED_FADE} ${className}`}
    />
  );
}

type LinkTone = "muted" | "primary" | "danger" | "warning";

const LINK_TONES: Record<LinkTone, string> = {
  muted: "text-ink-muted enabled:hover:text-ink enabled:active:text-ink-strong",
  primary: "text-primary enabled:active:text-primary-hover",
  danger: "text-danger",
  warning: "text-warning",
};

/** A button that reads as a link: a status-bar count, an inline "show" in a
 * sentence, a transcript timestamp. Hover underlines it and a press thickens
 * the underline. It fits its words and ellipsizes when its row runs out of
 * room; `wrap` lets a long value such as a path break across lines instead. */
export function LinkButton({
  tone = "muted",
  wrap = false,
  className = "",
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & {
  tone?: LinkTone;
  wrap?: boolean;
}) {
  return (
    <button
      {...props}
      className={`rounded-sm text-left underline-offset-2 enabled:hover:underline enabled:active:underline enabled:active:decoration-2 ${MOTION} ${LINK_TONES[tone]} ${
        wrap ? "break-words" : "min-w-0 max-w-full truncate"
      } ${DISABLED_FADE} ${className}`}
    />
  );
}
