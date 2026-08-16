import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useSettingsStore } from "../state/settings-store";
import { log, toErrorFields } from "../repositories";
import { useAppStore } from "../state/app-store";
import {
  describePosition,
  monitorKey,
  orderMonitors,
  priorityFromState,
} from "../utils/screens";
import ModalShell from "./ModalShell";
import DirectoryRow from "./DirectoryRow";
import Button from "./ui/Button";
import { Row, Select, TextInput, Toggle } from "./ui/Field";
import { Plus } from "lucide-react";

/** Screen priority: the ordered monitor list (1 = main, 2 = preview, 3+ =
 * comparison). Persisted as app STATE, not part of the config draft — screen
 * identifiers are machine-specific and reordering applies immediately, like
 * a pane width. Meaningful only with two or more monitors. */
function ScreensSection() {
  const [monitors, setMonitors] = useState<
    {
      name: string | null;
      position: { x: number; y: number };
      size: { width: number; height: number };
      scaleFactor?: number;
    }[]
  >([]);
  const priority = priorityFromState(useAppStore((s) => s.appData?.state) ?? null);
  useEffect(() => {
    void import("@tauri-apps/api/window").then(async ({ availableMonitors }) => {
      setMonitors(await availableMonitors().catch(() => []));
    });
  }, []);
  if (monitors.length < 2) return null;

  const ordered = orderMonitors(monitors, priority);
  const move = (index: number, delta: number) => {
    const target = index + delta;
    if (target < 0 || target >= ordered.length) return;
    const keys = ordered.map(monitorKey);
    [keys[index], keys[target]] = [keys[target], keys[index]];
    void useAppStore.getState().patchState({ screenPriority: keys });
  };
  const role = (index: number) =>
    index === 0 ? "main" : index === 1 ? "preview" : "comparison";

  return (
    <>
      <h2 className="mb-2 mt-6 text-xs font-semibold uppercase tracking-wide text-ink-muted">Screens</h2>
      <p className="mb-1 text-xs text-ink-muted">
        Order decides the role: 1 = main window, 2 = preview, the rest join
        the comparison spread. Applies immediately.
      </p>
      <Button
        className="mb-2"
        onClick={() => {
          // One self-closing flash per monitor, showing its ordinal — the
          // only way to tell a matched pair apart beyond "left"/"right".
          void import("@tauri-apps/api/webviewWindow").then(({ WebviewWindow }) => {
            ordered.forEach((monitor, index) => {
              const scale = monitor.scaleFactor || 1;
              new WebviewWindow(`identify-${index + 1}`, {
                url: `index.html?view=identify&slice=${index + 1}`,
                title: "OneCopy",
                x: monitor.position.x / scale + monitor.size.width / scale / 2 - 110,
                y: monitor.position.y / scale + monitor.size.height / scale / 2 - 110,
                width: 220,
                height: 220,
                decorations: false,
                alwaysOnTop: true,
                skipTaskbar: true,
                resizable: false,
                focus: false,
              });
            });
          });
        }}
      >
        Identify screens
      </Button>
      {ordered.map((monitor, index) => (
        <div
          key={monitorKey(monitor)}
          className="mb-1 flex items-center justify-between gap-2 rounded-lg border border-border px-3 py-2 text-sm"
        >
          <span className="min-w-0 flex-1">
            {/* The POSITION leads, because a matched pair reports the same
                name and the same resolution — where it sits is the only fact
                that maps onto the desk. */}
            <span className="text-ink">
              {index + 1}. {describePosition(monitor, ordered) || "Display"}
            </span>
            <span className="block truncate text-xs text-ink-muted">
              {monitor.name ?? "Display"} · {monitor.size.width}×{monitor.size.height} ·{" "}
              {role(index)}
            </span>
          </span>
          <span className="flex gap-1">
            <Button
              variant="ghost"
              aria-label="Move up"
              disabled={index === 0}
              onClick={() => move(index, -1)}
            >
              ↑
            </Button>
            <Button
              variant="ghost"
              aria-label="Move down"
              disabled={index === ordered.length - 1}
              onClick={() => move(index, 1)}
            >
              ↓
            </Button>
          </span>
        </div>
      ))}
    </>
  );
}

// The named Settings modal over the config tunables. Field-level checks only —
// the store never validates semantics (config-seeding conventions); Save
// persists, re-resolves the index from evidence, and refreshes the views.
// Save requires a dirty AND valid draft; closing with unsaved edits stacks a
// discard confirmation instead of silently dropping them.

// The field never reformats mid-edit (select-all + retype must not snap to
// the minimum under the caret); parsing and clamping happen on blur only.
function NumberField({
  label,
  hint,
  value,
  min,
  onChange,
}: {
  label: string;
  /** What the number MEANS, for the knobs whose effect is not obvious from
   * their name — a similarity threshold is a judgement call, so the row says
   * which direction is stricter. */
  hint?: string;
  value: number;
  min: number;
  onChange: (value: number) => void;
}) {
  const [text, setText] = useState<string | null>(null);
  return (
    <Row label={label} hint={hint}>
      <TextInput
        type="number"
        className="w-24 text-right"
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
    </Row>
  );
}

/** The unlink store's one surface: how many "not the same subject" verdicts
 * exist, and the way to take them all back. Without the count the exclusions
 * would be an invisible permanent store; with only the count there would be
 * no recovery from an accidental unlink. */
function UnlinkedPairsRow() {
  const [count, setCount] = useState<number | null>(null);
  useEffect(() => {
    void invoke<number>("similar_exclusions_count")
      .then(setCount)
      .catch((error) => log.warn("exclusions count failed", toErrorFields(error)));
  }, []);
  if (count === null || count === 0) return null;
  return (
    <Row
      label={`Unlinked pairs (${count})`}
      hint="Photos you marked as not similar. Forgetting lets them group again on the next scan."
    >
      <Button
        onClick={() => {
          void invoke("similar_exclusions_clear")
            .then(() => setCount(0))
            .catch((error) => log.warn("exclusions clear failed", toErrorFields(error)));
        }}
      >
        Forget all
      </Button>
    </Row>
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
    <Row label={label}>
      <Toggle checked={checked} onChange={onChange} />
    </Row>
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
  const movingCache = useSettingsStore((s) => s.movingCache);
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
        <Button
          variant="primary"
          disabled={saving || !dirty || !timezoneValid}
          onClick={() => void save()}
        >
          {saving ? "Saving…" : "Save"}
        </Button>
      }
    >
      {movingCache !== null ? (
        // "Moving cache" — a BLOCKING progress surface by design (the one
        // deliberate no-dismiss case: interrupting a tree move mid-copy is
        // what the modal exists to prevent); completable only.
        <div
          role="dialog"
          aria-modal="true"
          aria-label="Moving cache"
          className="fixed inset-0 z-40 flex items-center justify-center bg-background/80"
        >
          <div className="w-[380px] rounded border border-border bg-surface p-4">
            <p className="mb-2 text-sm font-semibold text-ink-strong">Moving cache…</p>
            <div className="h-2 w-full overflow-hidden rounded bg-surface-muted">
              <div
                className="h-full bg-primary transition-[width]"
                style={{
                  width: `${
                    movingCache.totalBytes > 0
                      ? Math.round((movingCache.copiedBytes / movingCache.totalBytes) * 100)
                      : 0
                  }%`,
                }}
              />
            </div>
            <p className="mt-2 text-xs text-ink-muted">
              {(movingCache.copiedBytes / 1_048_576).toFixed(0)} MB of{" "}
              {(movingCache.totalBytes / 1_048_576).toFixed(0)} MB — the old location is
              kept until every file is verified.
            </p>
          </div>
        </div>
      ) : null}
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
          <h2 className="mb-2 mt-1 text-xs font-semibold uppercase tracking-wide text-ink-muted">
            Directories
          </h2>
          {/* The same rows the wizard shows — one shared component, so the two
              lists cannot drift apart. */}
          <ul className="mb-3 space-y-1.5">
            {draft.sourceDirs.map((dir) => (
              <li key={dir}>
                <DirectoryRow path={dir} onRemove={() => removeSourceDir(dir)} />
              </li>
            ))}
          </ul>
          <Button onClick={() => void addSourceDir()}>
            <Plus size={14} />
            Add directory
          </Button>

          <h2 className="mb-2 mt-6 text-xs font-semibold uppercase tracking-wide text-ink-muted">
            Timestamps
          </h2>
          <Row label="Default timezone" hint="IANA name, e.g. Asia/Tokyo">
            <TextInput
              className="w-48"
              invalid={!timezoneValid}
              value={draft.defaultTimezone}
              onChange={(e) => void validateTimezone(e.target.value)}
            />
          </Row>
          <NumberField
            label="Good range starts (year)"
            value={draft.goodRangeStartYear}
            min={1900}
            onChange={(v) => update({ goodRangeStartYear: v })}
          />

          <h2 className="mb-2 mt-6 text-xs font-semibold uppercase tracking-wide text-ink-muted">
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
            hint="How different two photos may look and still pair. Lower is stricter; flat graphics crowd together, so a corpus of icons wants a lower number than photos do."
            value={draft.similarityPhashMaxDistance}
            min={0}
            onChange={(v) => update({ similarityPhashMaxDistance: v })}
          />
          <NumberField
            label="Family width (× the limits above)"
            hint="How far one family may spread. 1 means every photo must resemble the family's first member directly; 2 lets a burst whose ends differ meet through its middle. Higher risks unrelated subjects chaining into one family."
            value={draft.similarityDiameterMultiplier}
            min={1}
            onChange={(v) => update({ similarityDiameterMultiplier: Math.min(4, v) })}
          />
          <Row
            label="Match across devices"
            hint="Pairs the same scene from different cameras (needs the similarity model)"
          >
            <Toggle
              checked={draft.similarityEmbeddingEnabled}
              onChange={(checked) => update({ similarityEmbeddingEnabled: checked })}
            />
          </Row>
          <UnlinkedPairsRow />
          <NumberField
            label="Cross-device match strictness (%)"
            hint="Higher pairs less, and wrongly pairs less. Measured on the icon corpus, 95 admits about half the false pairs 90 does."
            value={draft.similarityEmbeddingThresholdPercent}
            min={50}
            onChange={(v) =>
              update({ similarityEmbeddingThresholdPercent: Math.min(100, v) })
            }
          />

          <h2 className="mb-2 mt-6 text-xs font-semibold uppercase tracking-wide text-ink-muted">
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

          <h2 className="mb-2 mt-6 text-xs font-semibold uppercase tracking-wide text-ink-muted">
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
          <NumberField
            label="Scenes grid columns"
            value={draft.scenesGridColumns}
            min={1}
            onChange={(v) => update({ scenesGridColumns: v })}
          />
          <NumberField
            label="Scenes grid rows"
            value={draft.scenesGridRows}
            min={1}
            onChange={(v) => update({ scenesGridRows: v })}
          />

          <h2 className="mb-2 mt-6 text-xs font-semibold uppercase tracking-wide text-ink-muted">
            Appearance
          </h2>
          <Row
            label="UI font"
            hint="A CSS font-family list, resolved by the webview"
          >
            <TextInput
              className="w-64"
              value={draft.uiFontFamily}
              onChange={(e) => update({ uiFontFamily: e.target.value })}
            />
          </Row>
          <Row label="Theme">
            <Select
              value={draft.theme}
              onChange={(e) =>
                update({ theme: e.target.value as "system" | "light" | "dark" })
              }
            >
              <option value="system">Follow the system</option>
              <option value="light">Light</option>
              <option value="dark">Dark</option>
            </Select>
          </Row>

          <ScreensSection />

          <h2 className="mb-2 mt-6 text-xs font-semibold uppercase tracking-wide text-ink-muted">
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
