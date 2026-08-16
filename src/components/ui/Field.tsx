// Form primitives for the settings-style surfaces.
//
// These exist for the same reason Button does: every modal was spelling its
// own `rounded border border-border px-2 py-0.5 text-sm` and each one landed
// somewhere slightly different, so the app read as a pile of separately-built
// dialogs. Height, radius, focus ring and disabled treatment are decided once
// here.
//
// `Row` is the shared label-left / control-right shape. It is a real <label>,
// so clicking the text focuses (or toggles) the control — which the hand-built
// flex rows it replaces did not do.

import type { InputHTMLAttributes, SelectHTMLAttributes, ReactNode } from "react";

const CONTROL =
  "h-8 rounded-lg border border-border bg-background px-2.5 text-sm text-ink outline-none transition-colors focus:border-border-strong focus-visible:ring-2 focus-visible:ring-primary-ring disabled:text-ink-muted";

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
    <label className="flex items-center justify-between gap-4 py-1.5">
      <span className="min-w-0">
        <span className="block text-sm text-ink">{label}</span>
        {hint ? <span className="block text-xs text-ink-muted">{hint}</span> : null}
      </span>
      <span className="shrink-0">{children}</span>
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
  className = "",
  ...props
}: InputHTMLAttributes<HTMLInputElement> & { invalid?: boolean }) {
  return (
    <input
      {...props}
      aria-invalid={invalid || undefined}
      className={`${CONTROL} ${invalid ? "border-danger" : ""} ${className}`}
    />
  );
}

export function Select({
  className = "",
  ...props
}: SelectHTMLAttributes<HTMLSelectElement>) {
  return <select {...props} className={`${CONTROL} pr-1 ${className}`} />;
}

/** A real switch rather than a bare checkbox. The native control is kept as
 * the accessible element (it stays keyboard- and screen-reader-correct, and it
 * is what the surrounding <label> targets); the visible track is drawn from
 * its checked state. */
export function Toggle({
  checked,
  onChange,
  disabled = false,
}: {
  checked: boolean;
  onChange: (checked: boolean) => void;
  disabled?: boolean;
}) {
  return (
    <span className="relative inline-flex h-5 w-9 shrink-0 items-center">
      <input
        type="checkbox"
        checked={checked}
        disabled={disabled}
        onChange={(e) => onChange(e.target.checked)}
        className="peer absolute inset-0 z-10 m-0 cursor-pointer opacity-0 disabled:cursor-default"
      />
      <span
        aria-hidden
        className={`h-5 w-9 rounded-full transition-colors peer-focus-visible:ring-2 peer-focus-visible:ring-primary-ring ${
          checked ? "bg-primary" : "bg-surface-muted"
        } ${disabled ? "opacity-50" : ""}`}
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
