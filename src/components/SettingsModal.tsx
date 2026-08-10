import { useState } from "react";
import { useSettingsStore } from "../state/settings-store";
import ModalShell from "./ModalShell";

// The named Settings modal over the config tunables. Field-level checks only —
// the store never validates semantics (config-seeding conventions); Save
// persists, re-resolves the index from evidence, and refreshes the views.
// Save requires a dirty AND valid draft; closing with unsaved edits stacks a
// discard confirmation instead of silently dropping them.

// The field never reformats mid-edit (select-all + retype must not snap to
// the minimum under the caret); parsing and clamping happen on blur only.
function NumberField({
  label,
  value,
  min,
  onChange,
}: {
  label: string;
  value: number;
  min: number;
  onChange: (value: number) => void;
}) {
  const [text, setText] = useState<string | null>(null);
  return (
    <label className="flex items-center justify-between gap-2 py-0.5 text-sm text-ink">
      <span>{label}</span>
      <input
        type="number"
        className="w-24 rounded border border-border bg-background px-2 py-0.5 text-right text-sm"
        value={text ?? String(value)}
        min={min}
        onFocus={() => setText(String(value))}
        onChange={(e) => setText(e.target.value)}
        onBlur={() => {
          const parsed = Number.parseInt(text ?? "", 10);
          onChange(Number.isFinite(parsed) ? Math.max(min, parsed) : value);
          setText(null);
        }}
      />
    </label>
  );
}

function CheckField({
  label,
  checked,
  onChange,
}: {
  label: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
}) {
  return (
    <label className="flex items-center justify-between gap-2 py-0.5 text-sm text-ink">
      <span>{label}</span>
      <input
        type="checkbox"
        checked={checked}
        onChange={(e) => onChange(e.target.checked)}
      />
    </label>
  );
}

export default function SettingsModal() {
  const open = useSettingsStore((s) => s.open);
  const draft = useSettingsStore((s) => s.draft);
  const opened = useSettingsStore((s) => s.opened);
  const timezoneValid = useSettingsStore((s) => s.timezoneValid);
  const saving = useSettingsStore((s) => s.saving);
  const message = useSettingsStore((s) => s.message);
  const close = useSettingsStore((s) => s.close);
  const update = useSettingsStore((s) => s.update);
  const validateTimezone = useSettingsStore((s) => s.validateTimezone);
  const addSourceDir = useSettingsStore((s) => s.addSourceDir);
  const removeSourceDir = useSettingsStore((s) => s.removeSourceDir);
  const pickCacheDir = useSettingsStore((s) => s.pickCacheDir);
  const clearCacheDir = useSettingsStore((s) => s.clearCacheDir);
  const save = useSettingsStore((s) => s.save);
  const [confirmDiscard, setConfirmDiscard] = useState(false);

  if (!open || draft === null) return null;

  const dirty = JSON.stringify(draft) !== JSON.stringify(opened);
  const requestClose = () => {
    if (dirty) setConfirmDiscard(true);
    else close();
  };

  return (
    <ModalShell
      title="Settings"
      onClose={requestClose}
      widthClass="w-[520px]"
      footerStart={message}
      primaryAction={
        <button
          className="rounded bg-primary px-3 py-1 text-sm text-ink-inverted disabled:bg-surface-muted disabled:text-ink-muted"
          disabled={saving || !dirty || !timezoneValid}
          onClick={() => void save()}
        >
          {saving ? "Saving…" : "Save"}
        </button>
      }
    >
      {confirmDiscard ? (
        <ModalShell
          title="Discard changes?"
          onClose={() => setConfirmDiscard(false)}
          widthClass="w-[360px]"
          closeLabel="Keep editing"
          primaryAction={
            <button
              className="rounded bg-danger px-3 py-1 text-sm text-ink-inverted"
              onClick={() => {
                setConfirmDiscard(false);
                close();
              }}
            >
              Discard
            </button>
          }
        >
          <p className="text-sm text-ink">
            Settings has unsaved edits. Discard them?
          </p>
        </ModalShell>
      ) : null}
          <h2 className="mb-1 mt-2 text-xs font-semibold uppercase text-ink-muted">
            Directories
          </h2>
          <ul className="mb-1">
            {draft.sourceDirs.map((dir) => (
              <li
                key={dir}
                className="mb-0.5 flex items-center justify-between gap-2 rounded border border-border px-2 py-0.5 text-xs"
              >
                <span className="min-w-0 flex-1 truncate text-ink" title={dir}>
                  {dir}
                </span>
                <button className="shrink-0 text-danger" onClick={() => removeSourceDir(dir)}>
                  Remove
                </button>
              </li>
            ))}
          </ul>
          <button
            className="rounded border border-border px-2 py-0.5 text-xs text-primary hover:bg-primary-surface"
            onClick={() => void addSourceDir()}
          >
            Add directory…
          </button>

          <h2 className="mb-1 mt-3 text-xs font-semibold uppercase text-ink-muted">
            Timestamps
          </h2>
          <label className="flex items-center justify-between gap-2 py-0.5 text-sm text-ink">
            <span>Default timezone</span>
            <input
              className={`w-48 rounded border px-2 py-0.5 text-sm ${
                timezoneValid ? "border-border" : "border-danger"
              } bg-background`}
              value={draft.defaultTimezone}
              onChange={(e) => void validateTimezone(e.target.value)}
            />
          </label>
          <NumberField
            label="Good range starts (year)"
            value={draft.goodRangeStartYear}
            min={1900}
            onChange={(v) => update({ goodRangeStartYear: v })}
          />

          <h2 className="mb-1 mt-3 text-xs font-semibold uppercase text-ink-muted">
            Similar photos
          </h2>
          <NumberField
            label="Max gap between spares (seconds)"
            value={draft.similarityMaxGapSeconds}
            min={1}
            onChange={(v) => update({ similarityMaxGapSeconds: v })}
          />
          <NumberField
            label="Visual distance limit (0–64)"
            value={draft.similarityPhashMaxDistance}
            min={0}
            onChange={(v) => update({ similarityPhashMaxDistance: v })}
          />

          <h2 className="mb-1 mt-3 text-xs font-semibold uppercase text-ink-muted">
            Previews
          </h2>
          <NumberField
            label="Preview long edge (px)"
            value={draft.previewLongEdgePx}
            min={480}
            onChange={(v) => update({ previewLongEdgePx: v })}
          />
          <NumberField
            label="Thumbnail edge (px)"
            value={draft.thumbnailEdgePx}
            min={96}
            onChange={(v) => update({ thumbnailEdgePx: v })}
          />
          <div className="flex items-center justify-between gap-2 py-0.5 text-sm text-ink">
            <span>Cache location</span>
            <span className="flex items-center gap-1">
              <span
                className="max-w-48 truncate text-xs text-ink-muted"
                title={draft.cacheDir ?? "default"}
              >
                {draft.cacheDir ?? "default"}
              </span>
              <button
                className="rounded border border-border px-1 text-xs text-primary"
                onClick={() => void pickCacheDir()}
              >
                Change…
              </button>
              {draft.cacheDir !== null ? (
                <button
                  className="rounded border border-border px-1 text-xs text-ink"
                  onClick={clearCacheDir}
                >
                  Default
                </button>
              ) : null}
            </span>
          </div>

          <h2 className="mb-1 mt-3 text-xs font-semibold uppercase text-ink-muted">
            Videos
          </h2>
          <NumberField
            label="Seconds per snapshot frame"
            value={draft.videoStripSecondsPerFrame}
            min={1}
            onChange={(v) => update({ videoStripSecondsPerFrame: v })}
          />
          <NumberField
            label="Snapshot frames (min)"
            value={draft.videoStripMinFrames}
            min={1}
            onChange={(v) => update({ videoStripMinFrames: v })}
          />
          <NumberField
            label="Snapshot frames (max)"
            value={draft.videoStripMaxFrames}
            min={1}
            onChange={(v) => update({ videoStripMaxFrames: v })}
          />

          <h2 className="mb-1 mt-3 text-xs font-semibold uppercase text-ink-muted">
            Behavior
          </h2>
          <CheckField
            label="Pair companion files (RAW, sidecars)"
            checked={draft.pairingEnabled}
            onChange={(v) => update({ pairingEnabled: v })}
          />
          <CheckField
            label="Keep the system awake while indexing"
            checked={draft.keepAwakeDuringIndexing}
            onChange={(v) => update({ keepAwakeDuringIndexing: v })}
          />
          <CheckField
            label="Read-back verify every copy out"
            checked={draft.verifyAfterCopy}
            onChange={(v) => update({ verifyAfterCopy: v })}
          />
    </ModalShell>
  );
}
