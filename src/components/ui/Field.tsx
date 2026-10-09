// Form primitives for the settings-style surfaces.
//
// These exist for the same reason Button does: a modal that spells its own
// `rounded border px-2 py-0.5 text-sm` lands somewhere slightly different from
// the next one, and the app reads as a pile of separately-built dialogs.
// Height, edge, radius, padding, focus and disabled treatment are decided
// once here, for every one-line field — text box and select alike — because a
// browser sizes a select and an input differently from the same padding and
// font, so the height is given outright.
//
// `Row` is the shared label-left / control-right shape. It is a real <label>,
// so clicking the text focuses (or toggles) the control.

import type { InputHTMLAttributes, SelectHTMLAttributes, ReactNode } from "react";
import { ChevronDown } from "lucide-react";
import { DISABLED_FADE } from "./Button";

export type FieldSize = "standard" | "toolbar";

// A field shows focus by recolouring its own frame rather than by a second
// ring outside it (composite-control-conventions: a container with a visible
// frame of its own may recolour that frame). `focus:` rather than
// `focus-visible:`, because a select opened by the pointer is still the field
// the keyboard now acts on.
const CONTROL = `max-w-full border bg-surface text-ink outline-none transition-colors motion-reduce:transition-none focus:border-focus-ring focus:shadow-[inset_0_0_0_1px_var(--focus-ring)] ${DISABLED_FADE}`;

// One anatomy at the two heights the app's rows use: `standard` beside the
// standard 32px buttons of a form, `toolbar` beside the 28px text actions of a
// pane's toolbar, so either row lines up.
const SIZES: Record<FieldSize, string> = {
  standard: "h-8 rounded-lg px-2.5 text-sm",
  toolbar: "h-7 rounded-md px-2 text-xs",
};

export function Row({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: ReactNode;
}) {
  return (
    <label className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2 py-1.5">
      <span className="min-w-0 flex-1 basis-48 break-words">
        <span className="block text-sm text-ink">{label}</span>
        {hint ? <span className="block text-xs text-ink-muted">{hint}</span> : null}
      </span>
      <span className="ml-auto flex min-w-0 max-w-full flex-col items-end">{children}</span>
    </label>
  );
}

export function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="mb-6 last:mb-0">
      <h2 className="mb-2 text-xs font-semibold uppercase tracking-wide text-ink-muted">
        {title}
      </h2>
      {children}
    </section>
  );
}

export function TextInput({
  invalid = false,
  size = "standard",
  className = "",
  ...props
}: Omit<InputHTMLAttributes<HTMLInputElement>, "size"> & { invalid?: boolean; size?: FieldSize }) {
  return (
    <input
      {...props}
      aria-invalid={invalid || undefined}
      className={`${CONTROL} ${SIZES[size]} ${invalid ? "border-danger" : "border-control-edge"} ${className}`}
    />
  );
}

export function Select({
  size = "standard",
  className = "",
  ...props
}: Omit<SelectHTMLAttributes<HTMLSelectElement>, "size"> & { size?: FieldSize }) {
  // A restyled control draws its own chevron rather than keeping the
  // platform's native one (interface-styling-conventions, R8-07): only the
  // closed control is the app's, and the list it opens stays the platform's.
  return (
    <span className="relative inline-flex min-w-0 max-w-full">
      <select
        {...props}
        className={`${CONTROL} ${SIZES[size]} appearance-none border-control-edge ${size === "toolbar" ? "pr-6" : "pr-7"} ${className}`}
      />
      <ChevronDown
        aria-hidden="true"
        size={size === "toolbar" ? 12 : 14}
        className="pointer-events-none absolute right-2 top-1/2 -translate-y-1/2 text-ink-muted"
      />
    </span>
  );
}

/** A real switch rather than a bare checkbox. The native control is kept as
 * the accessible element (it stays keyboard- and screen-reader-correct, and it
 * is what the surrounding <label> targets); the visible track is drawn from
 * its checked state. */
export function Toggle({
  checked,
  onChange,
  disabled = false,
  autoFocus = false,
}: {
  checked: boolean;
  onChange: (checked: boolean) => void;
  disabled?: boolean;
  autoFocus?: boolean;
}) {
  return (
    // Off, the whole switch fades as one control, knob included.
    <span className={`relative inline-flex h-5 w-9 shrink-0 items-center ${disabled ? "opacity-50" : ""}`}>
      <input
        type="checkbox"
        checked={checked}
        disabled={disabled}
        autoFocus={autoFocus}
        onChange={(e) => onChange(e.target.checked)}
        className="peer absolute inset-0 z-10 m-0 cursor-pointer opacity-0 disabled:cursor-default"
      />
      <span
        aria-hidden
        // The visible track takes the app's one focus ring for the invisible
        // checkbox that holds focus.
        className={`h-5 w-9 rounded-full transition-colors motion-reduce:transition-none peer-focus-visible:outline-2 peer-focus-visible:outline-offset-1 peer-focus-visible:outline-focus-ring ${
          // Off, the track is drawn in the control-edge colour: a switch's
          // track is its boundary, and the knob needs it to stand out.
          checked ? "bg-primary" : "bg-control-edge"
        }`}
      />
      <span
        aria-hidden
        className={`pointer-events-none absolute top-0.5 h-4 w-4 rounded-full bg-surface shadow-sm transition-all ${
          checked ? "left-[1.125rem]" : "left-0.5"
        }`}
      />
    </span>
  );
}
