import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { accelerationModeLabel, featureLabel } from "../models/coreLabels";
import { invoke } from "@tauri-apps/api/core";
import { availableMonitors, type Monitor } from "@tauri-apps/api/window";
import { identifyScreens } from "../workflows/identify-screens";
import { useSettingsStore } from "../state/settings-store";
import { saveSettings } from "../workflows/settings";
import { useQuitDiscardStore } from "../workflows/quit";
import { log, toErrorFields } from "../repositories";
import { useAppStore } from "../state/app-store";
import { useItemsStore } from "../state/items-store";
import { useIssuesStore } from "../state/issues-store";
import { useSectionsStore } from "../state/sections-store";
import {
  describePosition,
  monitorKey,
  orderMonitors,
  priorityFromConfig,
  swappedPriority,
} from "../utils/screens";
import ModalShell from "./ModalShell";
import ConfirmModal from "./ConfirmModal";
import RebuildIndexModal from "./RebuildIndexModal";
import DirectoryRow from "./DirectoryRow";
import Button from "./ui/Button";
import { Row, Select, TextInput, Toggle } from "./ui/Field";
import { Plus } from "lucide-react";
import { message } from "../i18n/translate";
import { recordActionFailure } from "../state/notifications-store";
import OperationResult from "./ui/OperationResult";
import { computerTimeZone, SYSTEM_TIME_ZONE, timeZoneOptions } from "../utils/timezones";
import { type MessageKey } from "../i18n/catalogues";
import { useI18n } from "../i18n/I18nContext";
import { LANGUAGES, LANGUAGE_NAMES, normalizeLanguagePreference } from "../i18n/languages";

/** The core's text-preview choices (`text_preview::Options`); the limit
 * keeps the gate itself bounded (C-L2). */
interface TextPreviewOptions {
  encodings: string[];
  maxAllowedBytes: number;
}

/** Auxiliary display priority. A setting of its own, outside the draft:
 * reordering is saved the moment it happens. Meaningful only with two or more
 * monitors. */
function ScreensSection() {
  const { t } = useI18n();
  const [monitors, setMonitors] = useState<Monitor[]>([]);
  // The key, not a finished sentence, so the message follows a language change.
  const [screenError, setScreenError] = useState<MessageKey | null>(null);
  const [identifying, setIdentifying] = useState(false);
  const mounted = useRef(false);
  const priority = priorityFromConfig(
    useAppStore((s) => s.appData?.config) ?? null,
  );
  useEffect(() => {
    mounted.current = true;
    let current = true;
    void availableMonitors()
      .then((available) => {
        if (!current) return;
        setMonitors(available);
        setScreenError(null);
      })
      .catch((error) => {
        if (!current) return;
        log.warn("settings monitor query failed", toErrorFields(error));
        setScreenError("settings.screensReadFailed");
        recordActionFailure(
          "screen-list-failed",
          message("settings.screensReadFailed"),
          error,
        );
      });
    return () => { current = false; mounted.current = false; };
  }, []);
  if (monitors.length < 2) return screenError === null ? null : (
    <OperationResult level="error" className="mt-6">{t(screenError)}</OperationResult>
  );

  const ordered = orderMonitors(monitors, priority);
  const move = (index: number, delta: number) => {
    const target = index + delta;
    if (target < 0 || target >= ordered.length) return;
    const first = monitorKey(ordered[index]);
    const second = monitorKey(ordered[target]);
    const connected = monitors;
    void useAppStore
      .getState()
      // This row shows the failure itself, so the core stays quiet: one failed
      // write is one notice and one Issue. The swap is applied to the order as
      // last saved when the write runs, so quick clicks each count.
      .saveConfig(
        (config) => {
          const next = swappedPriority(priorityFromConfig(config), connected, first, second);
          return next === null ? null : { screenPriority: next };
        },
        { reportFailure: false },
      )
      .catch((error) => {
        log.error("screen priority save failed", toErrorFields(error));
        setScreenError("settings.screenOrderSaveFailed");
        recordActionFailure(
          "screen-order-save-failed",
          message("settings.screenOrderSaveFailed"),
          error,
        );
      });
  };
  return (
    <>
      <h2 className="mb-2 mt-6 text-xs font-semibold uppercase tracking-wide text-ink-muted">
        {t("settings.screens")}
      </h2>
      <p className="mb-3 text-xs text-ink-muted">
        {t("settings.screensHint")}
      </p>
      <Button
        className="mb-2"
        disabled={identifying}
        onClick={async () => {
          setIdentifying(true);
          setScreenError(null);
          try {
            await identifyScreens(ordered);
          } catch (error) {
            log.warn("screen identification failed", toErrorFields(error));
            if (mounted.current) setScreenError("settings.screenIdentifyFailed");
            recordActionFailure(
              "screen-identify-failed",
              message("settings.screenIdentifyFailed"),
              error,
            );
          } finally {
            if (mounted.current) setIdentifying(false);
          }
        }}
      >
        {identifying ? t("settings.identifyingScreens") : t("settings.identifyScreens")}
      </Button>
      {screenError !== null && (
        <OperationResult level="error" className="mb-3">{t(screenError)}</OperationResult>
      )}
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
              {t("settings.screenRank", {
                rank: index + 1,
                position:
                  describePosition(monitor, ordered) ?? t("settings.display"),
              })}
            </span>
            <span className="block truncate text-xs text-ink-muted">
              {/* The pixel counts are passed as text: a resolution carries no
                  thousands separator. */}
              {t("settings.screenDetail", {
                name: monitor.name ?? t("settings.display"),
                width: String(monitor.size.width),
                height: String(monitor.size.height),
              })}
            </span>
          </span>
          {/* The words, not a chevron: OneCopy spends that glyph on sort
              direction in the grid and on disclosure in Destinations, so on a
              row it would read as one of those rather than as a move. The
              words are also the accessible name, so nothing has to be kept in
              step with a label nobody can see. */}
          <span className="flex gap-1">
            <Button disabled={index === 0} onClick={() => move(index, -1)}>
              {t("settings.moveUp")}
            </Button>
            <Button
              disabled={index === ordered.length - 1}
              onClick={() => move(index, 1)}
            >
              {t("settings.moveDown")}
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
  max,
  onChange,
}: {
  label: string;
  /** Explain a numeric choice when its label alone is insufficient. */
  hint?: string;
  value: number;
  min: number;
  max?: number;
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
        max={max}
        onFocus={() => setText(String(value))}
        onChange={(e) => setText(e.target.value)}
        onBlur={() => {
          const parsed = Number.parseInt(text ?? "", 10);
          onChange(
            Number.isFinite(parsed)
              ? Math.min(max ?? Number.POSITIVE_INFINITY, Math.max(min, parsed))
              : value,
          );
          setText(null);
        }}
      />
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

const SETTINGS_TABS = [
  { id: "library", label: "settings.library" },
  { id: "media", label: "settings.media" },
  { id: "appearance", label: "settings.appearance" },
  { id: "behavior", label: "settings.behavior" },
] as const;

type SettingsTab = (typeof SETTINGS_TABS)[number]["id"];

function SettingsTabList({
  active,
  onChange,
}: {
  active: SettingsTab;
  onChange: (tab: SettingsTab) => void;
}) {
  const { t } = useI18n();
  const moveFocus = (event: KeyboardEvent<HTMLButtonElement>) => {
    const tabs = Array.from(
      event.currentTarget.parentElement?.querySelectorAll<HTMLButtonElement>(
        "[role='tab']",
      ) ?? [],
    );
    const current = tabs.indexOf(event.currentTarget);
    let next = current;
    // Arrows STOP at the ends — the app-wide end-of-axis choice App.tsx's
    // Details/Destinations tablist, Grid and Sidebar all already use
    // (R8-01) — rather than wrapping around.
    if (event.key === "ArrowRight") next = Math.min(current + 1, tabs.length - 1);
    else if (event.key === "ArrowLeft") next = Math.max(current - 1, 0);
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = tabs.length - 1;
    else return;
    event.preventDefault();
    const tab = SETTINGS_TABS[next]?.id;
    if (tab !== undefined) onChange(tab);
    tabs[next]?.focus();
  };

  return (
    <div
      role="tablist"
      aria-label={t("settings.categories")}
      className="sticky top-0 z-10 mb-3 flex gap-1 border-b border-border bg-surface pb-2"
    >
      {SETTINGS_TABS.map((tab) => (
        <button
          key={tab.id}
          id={`settings-tab-${tab.id}`}
          role="tab"
          aria-selected={active === tab.id}
          aria-controls={`settings-panel-${tab.id}`}
          tabIndex={active === tab.id ? 0 : -1}
          className={`rounded-lg px-3 py-1.5 text-sm font-medium ${
            active === tab.id
              ? "bg-primary-surface text-primary"
              : "text-ink-muted hover:bg-surface-muted hover:text-ink"
          }`}
          onClick={() => onChange(tab.id)}
          onKeyDown={moveFocus}
        >
          {t(tab.label)}
        </button>
      ))}
    </div>
  );
}

export default function SettingsModal({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) {
  const { t, text } = useI18n();
  const [activeTab, setActiveTab] = useState<SettingsTab>("library");
  const draft = useSettingsStore((s) => s.draft);
  const opened = useSettingsStore((s) => s.opened);
  const saving = useSettingsStore((s) => s.saving);
  const storeMessage = useSettingsStore((s) => s.message);
  const messageLevel = useSettingsStore((s) => s.messageLevel);
  const discardDraft = useSettingsStore((s) => s.discardDraft);
  const update = useSettingsStore((s) => s.update);
  const addSourceDir = useSettingsStore((s) => s.addSourceDir);
  const removeSourceDir = useSettingsStore((s) => s.removeSourceDir);
  const accelerationCapabilities = useSettingsStore(
    (s) => s.accelerationCapabilities,
  );
  const [confirmDiscard, setConfirmDiscard] = useState(false);
  const quitDiscard = useQuitDiscardStore((s) => s.choose);
  const quitDiscardPresented = useQuitDiscardStore((s) => s.presented);
  const [confirmRebuild, setConfirmRebuild] = useState(false);
  const [rebuilding, setRebuilding] = useState(false);
  const [textOptions, setTextOptions] = useState<TextPreviewOptions | null>(null);
  const [visibilityCapabilities, setVisibilityCapabilities] = useState<{
    hiddenAttributes: boolean; systemAttributes: boolean;
  } | null>(null);

  useEffect(() => {
    if (!open) return;
    let current = true;
    void invoke<{ hiddenAttributes: boolean; systemAttributes: boolean }>("visibility_capabilities")
      .then((capabilities) => { if (current) setVisibilityCapabilities(capabilities); })
      .catch((error) => {
        if (current) {
          recordActionFailure(
            "visibility-capabilities-failed",
            message("settings.visibilityOptionsReadFailed"),
            error,
          );
        }
      });
    return () => { current = false; };
  }, [open]);

  useEffect(() => {
    if (!open || textOptions !== null) return;
    void invoke<TextPreviewOptions>("text_preview_options")
      .then(setTextOptions)
      .catch((error) => {
        log.warn("text encoding list failed", toErrorFields(error));
        const failure = message("settings.textEncodingsReadFailed");
        useSettingsStore.setState({ message: failure, messageLevel: "error" });
        recordActionFailure("text-encodings-load-failed", failure, error);
      });
  }, [open, textOptions]);

  // Quitting over unsaved edits asks here, where the edits are.
  useEffect(() => {
    if (open && draft !== null) quitDiscardPresented?.();
  }, [open, draft, quitDiscardPresented]);

  if (!open || draft === null) return null;

  const dirty = JSON.stringify(draft) !== JSON.stringify(opened);
  const requestClose = () => {
    if (dirty) setConfirmDiscard(true);
    else {
      discardDraft();
      onClose();
    }
  };

  return (
    <ModalShell
      title={t("settings.title")}
      onClose={requestClose}
      closeLabel={t("common.cancel")}
      closeDisabled={saving}
      widthClass="w-[min(760px,calc(100vw-3rem))]"
      footerResult={
        storeMessage === null ? undefined : (
          <OperationResult level={messageLevel ?? "info"}>
            {text(storeMessage)}
          </OperationResult>
        )
      }
      primaryAction={
        <Button
          variant="primary"
          disabled={saving || !dirty}
          onClick={() => void saveSettings()}
        >
          {saving ? t("common.saving") : t("settings.save")}
        </Button>
      }
    >
      {quitDiscard !== null ? (
        <ConfirmModal
          title={t("settings.discardTitle")}
          message={t("settings.discardMessage")}
          confirmLabel={t("settings.discard")}
          cancelLabel={t("settings.keepEditing")}
          onConfirm={() => {
            setConfirmDiscard(false);
            quitDiscard(true);
          }}
          onCancel={() => quitDiscard(false)}
        />
      ) : confirmDiscard ? (
        <ConfirmModal
          title={t("settings.discardTitle")}
          message={t("settings.discardMessage")}
          confirmLabel={t("settings.discard")}
          cancelLabel={t("settings.keepEditing")}
          onConfirm={() => {
            setConfirmDiscard(false);
            discardDraft();
            onClose();
          }}
          onCancel={() => setConfirmDiscard(false)}
        />
      ) : null}
      {confirmRebuild ? (
        <RebuildIndexModal
          onConfirm={({ discardPreviews, discardTranscripts, discardFaces }) => {
            setConfirmRebuild(false);
            setRebuilding(true);
            useSettingsStore.setState({
              message: message("settings.rebuildingIndex"),
              messageLevel: "info",
            });
            void invoke("rebuild_library_index", { discardPreviews, discardTranscripts, discardFaces })
              .then(async () => {
                await Promise.all([
                  useSectionsStore.getState().loadCounts(),
                  useItemsStore.getState().refresh(),
                  useIssuesStore.getState().load(),
                  useSectionsStore.getState().loadIndexWork(),
                ]);
                useSettingsStore.setState({
                  message: message("settings.rebuildCleared"),
                  messageLevel: "info",
                });
              })
              .catch((error) => {
                useSettingsStore.setState({
                  message: message("settings.rebuildFailed"),
                  messageLevel: "error",
                });
                log.error("library index rebuild failed", toErrorFields(error));
                recordActionFailure(
                  "library-rebuild-failed",
                  message("settings.rebuildFailedNotice"),
                  error,
                );
              })
              .finally(() => setRebuilding(false));
          }}
          onCancel={() => setConfirmRebuild(false)}
        />
      ) : null}
      <SettingsTabList active={activeTab} onChange={setActiveTab} />
      {activeTab === "library" ? (
        <div
          id="settings-panel-library"
          role="tabpanel"
          aria-labelledby="settings-tab-library"
        >
          <h2 className="mb-2 mt-1 text-xs font-semibold uppercase tracking-wide text-ink-muted">
            {t("settings.directories")}
          </h2>
          {/* The same rows the wizard shows — one shared component, so the two
              lists cannot drift apart. */}
          <ul className="mb-3 space-y-1.5">
            {draft.sourceDirs.length === 0 ? (
              <li className="text-sm text-ink-muted">
                {t("settings.noSourceDirectories")}
              </li>
            ) : null}
            {draft.sourceDirs.map((dir) => (
              <li key={dir}>
                <DirectoryRow
                  path={dir}
                  onRemove={() => removeSourceDir(dir)}
                />
              </li>
            ))}
          </ul>
          <Button onClick={() => void addSourceDir()}>
            <Plus size={14} />
            {t("settings.addDirectory")}
          </Button>

          <h2 className="mb-2 mt-6 text-xs font-semibold uppercase tracking-wide text-ink-muted">
            {t("settings.visibility")}
          </h2>
          <p className="mb-3 text-xs text-ink-muted">
            {t("settings.visibilityHint")}
          </p>
          <CheckField label={t("settings.hideDotNames")} checked={draft.hideDotNames}
            onChange={(value) => update({ hideDotNames: value })} />
          {visibilityCapabilities?.hiddenAttributes ? (
            <CheckField label={t("settings.hideHiddenAttributes")} checked={draft.hideHiddenAttributes}
              onChange={(value) => update({ hideHiddenAttributes: value })} />
          ) : null}
          {visibilityCapabilities?.systemAttributes ? (
            <CheckField label={t("settings.hideSystemAttributes")} checked={draft.hideSystemAttributes}
              onChange={(value) => update({ hideSystemAttributes: value })} />
          ) : null}
          <p className="mb-2 mt-3 text-sm">{t("settings.ignoredFileNames")}</p>
          <p className="mb-2 text-xs text-ink-muted">{t("settings.ignoredFileNamesHint")}</p>
          <ul className="mb-2 space-y-2">
            {draft.ignoredFileNames.map((name, index) => (
              <li key={index} className="flex items-center gap-2">
                <TextInput className="min-w-0 flex-1" aria-label={t("settings.ignoredFileNameField", { number: index + 1 })} value={name}
                  onChange={(event) => update({ ignoredFileNames: draft.ignoredFileNames.map((entry, position) => position === index ? event.target.value : entry) })} />
                <Button aria-label={t("settings.removeIgnoredFileName", { number: index + 1 })}
                  onClick={() => update({ ignoredFileNames: draft.ignoredFileNames.filter((_, position) => position !== index) })}>{t("common.remove")}</Button>
              </li>
            ))}
          </ul>
          <Button onClick={() => update({ ignoredFileNames: [...draft.ignoredFileNames, ""] })}>{t("settings.addFileName")}</Button>

          <h2 className="mb-2 mt-6 text-xs font-semibold uppercase tracking-wide text-ink-muted">
            {t("settings.timestamps")}
          </h2>
          <Row label={t("settings.defaultTimezone")} hint={t("settings.defaultTimezoneHint")}>
            <Select
              className="w-64"
              value={draft.defaultTimezone}
              onChange={(e) => update({ defaultTimezone: e.target.value })}
            >
              <option value={SYSTEM_TIME_ZONE}>
                {t("settings.timezoneSystem", { zone: computerTimeZone() })}
              </option>
              {timeZoneOptions(draft.defaultTimezone).map((zone) => (
                <option key={zone} value={zone}>
                  {zone}
                </option>
              ))}
            </Select>
          </Row>

          <h2 className="mb-2 mt-6 text-xs font-semibold uppercase tracking-wide text-ink-muted">
            {t("settings.applicationUpdates")}
          </h2>
          <CheckField
            label={t("settings.checkGithubReleases")}
            checked={draft.checkGithubReleasesAtLaunch}
            onChange={(v) => update({ checkGithubReleasesAtLaunch: v })}
          />

          <h2 className="mb-2 mt-6 text-xs font-semibold uppercase tracking-wide text-ink-muted">
            {t("settings.libraryMaintenance")}
          </h2>
          <CheckField
            label={t("settings.checkSourceFolders")}
            checked={draft.checkSourceFoldersAtLaunch}
            onChange={(v) => update({ checkSourceFoldersAtLaunch: v })}
          />

          <CheckField
            label={t("settings.keepAwake")}
            checked={draft.keepAwakeDuringIndexing}
            onChange={(v) => update({ keepAwakeDuringIndexing: v })}
          />

          <Row
            label={t("settings.rebuildIndex")}
            hint={t("settings.rebuildIndexHint")}
          >
            <Button
              disabled={dirty || saving || rebuilding}
              onClick={() => setConfirmRebuild(true)}
            >
              {rebuilding ? t("settings.rebuilding") : t("settings.rebuildRequest")}
            </Button>
          </Row>
        </div>
      ) : null}

      {activeTab === "media" ? (
        <div
          id="settings-panel-media"
          role="tabpanel"
          aria-labelledby="settings-tab-media"
        >
          <h2 className="mb-2 mt-1 text-xs font-semibold uppercase tracking-wide text-ink-muted">
            {t("settings.previews")}
          </h2>

          <CheckField
            label={t("settings.enlargeSmallImages")}
            checked={draft.enlargeSmallImages}
            onChange={(v) => update({ enlargeSmallImages: v })}
          />

          <Row label={t("settings.fallbackTextEncoding")}>
            <Select
              value={draft.textFallbackEncoding}
              onChange={(event) =>
                update({ textFallbackEncoding: event.target.value })
              }
            >
              {(textOptions !== null && textOptions.encodings.length > 0
                ? textOptions.encodings
                : [draft.textFallbackEncoding]
              ).map((encoding) => (
                <option key={encoding} value={encoding}>
                  {encoding}
                </option>
              ))}
            </Select>
          </Row>
          <h2 className="mb-2 mt-6 text-xs font-semibold uppercase tracking-wide text-ink-muted">
            {t("settings.photosAndComparison")}
          </h2>
          <CheckField
            label={t("settings.findSimilarPhotos")}
            checked={draft.similarPhotoAnalysisEnabled}
            onChange={(v) => update({ similarPhotoAnalysisEnabled: v })}
          />

          <Row label={t("settings.similarPhotoGrouping")}>
            <Select value={draft.similarPhotoGrouping} onChange={(event) => update({ similarPhotoGrouping: event.target.value as typeof draft.similarPhotoGrouping })}>
              <option value="stricter">{t("settings.groupingStricter")}</option>
              <option value="normal">{t("settings.groupingNormal")}</option>
              <option value="looser">{t("settings.groupingLooser")}</option>
            </Select>
          </Row>
          <CheckField
            label={t("settings.scoreFaces")}
            checked={draft.scoreFaces}
            onChange={(v) => update({ scoreFaces: v })}
          />

          <NumberField
            label={t("settings.maximumImagesInComparison")}
            hint={t("settings.maximumImagesInComparisonHint")}
            value={draft.maximumImagesInComparison}
            min={2}
            onChange={(v) => update({ maximumImagesInComparison: v })}
          />

          <h2 className="mb-2 mt-6 text-xs font-semibold uppercase tracking-wide text-ink-muted">
            {t("settings.playback")}
          </h2>
          <CheckField label={t("settings.autoplay")} checked={draft.autoplay} onChange={(v) => update({ autoplay: v })} />
          <CheckField
            label={t("settings.sound")}
            checked={draft.soundEnabled}
            onChange={(v) => update({ soundEnabled: v })}
          />

          <NumberField
            label={t("settings.playbackVolume")}
            value={Math.round(draft.playbackVolume * 100)}
            min={1}
            onChange={(v) => update({ playbackVolume: Math.min(100, v) / 100 })}
          />

          <h2 className="mb-2 mt-6 text-xs font-semibold uppercase tracking-wide text-ink-muted">
            {t("settings.videos")}
          </h2>

          <CheckField
            label={t("settings.generateSceneSnapshots")}
            checked={draft.videoSnapshotsEnabled}
            onChange={(v) => update({ videoSnapshotsEnabled: v })}
          />

          <CheckField
            label={t("settings.transcribeVideos")}
            checked={draft.videoTranscriptionEnabled}
            onChange={(v) => update({ videoTranscriptionEnabled: v })}
          />

          <h2 className="mb-2 mt-6 text-xs font-semibold uppercase tracking-wide text-ink-muted">
            {t("settings.audio")}
          </h2>

          <CheckField
            label={t("settings.transcribeAudio")}
            checked={draft.audioTranscriptionEnabled}
            onChange={(v) => update({ audioTranscriptionEnabled: v })}
          />
          {accelerationCapabilities.length > 0 ? (
            <>
              <h2 className="mb-2 mt-6 text-xs font-semibold uppercase tracking-wide text-ink-muted">
                {t("settings.aiAcceleration")}
              </h2>
              <p className="mb-2 text-xs text-ink-muted">
                {t("settings.aiAccelerationHint")}
              </p>
              {accelerationCapabilities.map((capability) => {
                const selected =
                  draft.aiAcceleration[capability.feature] ?? capability.default;
                const supported = capability.options.some(
                  (option) => option.id === selected,
                );
                return (
                  <Row
                    key={capability.feature}
                    label={t("settings.featureAcceleration", { feature: featureLabel(capability.feature, capability.label, t) })}
                  >
                    {capability.options.length === 1 && supported ? (
                      <span className="text-sm text-ink-muted">
                        {accelerationModeLabel(capability.options[0].id, capability.options[0].label, t)}
                      </span>
                    ) : (
                      <Select
                        value={selected}
                        aria-label={t("settings.featureAcceleration", { feature: featureLabel(capability.feature, capability.label, t) })}
                        onChange={(event) =>
                          update({
                            aiAcceleration: {
                              ...draft.aiAcceleration,
                              [capability.feature]: event.target.value,
                            },
                          })
                        }
                      >
                        {!supported ? (
                          <option value={selected} disabled>
                            {t("settings.accelerationUnavailable", { option: selected })}
                          </option>
                        ) : null}
                        {capability.options.map((option) => (
                          <option key={option.id} value={option.id}>
                            {accelerationModeLabel(option.id, option.label, t)}
                          </option>
                        ))}
                      </Select>
                    )}
                  </Row>
                );
              })}
            </>
          ) : null}
        </div>
      ) : null}

      {activeTab === "appearance" ? (
        <div
          id="settings-panel-appearance"
          role="tabpanel"
          aria-labelledby="settings-tab-appearance"
        >
          <h2 className="mb-2 mt-1 text-xs font-semibold uppercase tracking-wide text-ink-muted">
            {t("settings.appearance")}
          </h2>
          <Row label={t("settings.language")}>
            <Select
              value={draft.language}
              onChange={(e) =>
                update({ language: normalizeLanguagePreference(e.target.value) })
              }
            >
              <option value="system">{t("settings.languageSystem")}</option>
              {LANGUAGES.map((language) => (
                <option key={language} value={language} lang={language}>
                  {LANGUAGE_NAMES[language]}
                </option>
              ))}
            </Select>
          </Row>
          <Row
            label={t("settings.uiFont")}
            hint={t("settings.uiFontHint")}
          >
            <TextInput
              className="w-64"
              value={draft.uiFontFamily}
              placeholder={t("settings.systemFont")}
              onChange={(e) => update({ uiFontFamily: e.target.value })}
            />
          </Row>
          <Row label={t("settings.theme")}>
            <Select
              value={draft.theme}
              onChange={(e) =>
                update({ theme: e.target.value as "system" | "light" | "dark" })
              }
            >
              <option value="system">{t("settings.themeSystem")}</option>
              <option value="light">{t("settings.themeLight")}</option>
              <option value="dark">{t("settings.themeDark")}</option>
            </Select>
          </Row>

          <ScreensSection />
        </div>
      ) : null}

      {activeTab === "behavior" ? (
        <div
          id="settings-panel-behavior"
          role="tabpanel"
          aria-labelledby="settings-tab-behavior"
        >
          <h2 className="mb-2 mt-1 text-xs font-semibold uppercase tracking-wide text-ink-muted">
            {t("settings.notifications")}
          </h2>
          <NumberField
            label={t("settings.notificationDisplayTime")}
            value={draft.notificationDisplaySeconds}
            min={1}
            max={60}
            onChange={(v) => update({ notificationDisplaySeconds: v })}
          />
          <h2 className="mb-2 mt-6 text-xs font-semibold uppercase tracking-wide text-ink-muted">
            {t("settings.fileOperations")}
          </h2>

          <CheckField
            label={t("settings.confirmTrashDelete")}
            checked={draft.confirmTrashDelete}
            onChange={(v) => update({ confirmTrashDelete: v })}
          />
        </div>
      ) : null}
    </ModalShell>
  );
}
