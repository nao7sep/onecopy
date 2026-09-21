import { useEffect, useRef, useState } from "react";
import { reasonText } from "../models/workReasons";
import type { MessageKey } from "../i18n/catalogues";
import { useI18n } from "../i18n/I18nContext";
import { message, type Message } from "../i18n/translate";
import { useBinariesStore } from "../state/binaries-store";
import { useWindowPreferencesStore } from "../state/window-preferences-store";
import { useDerivedWorkStore } from "../state/derived-work-store";
import { useTranscriptStore } from "../state/transcript-store";
import { useAppStore } from "../state/app-store";
import Button from "./ui/Button";
import type { ItemWorkState } from "../models/items";
import {
  installContentSessionClient,
  setTranscriptOpen,
  setTranscriptView,
  useContentSessionStore,
} from "../state/content-session-store";
import { usePlaybackClientStore } from "../state/playback-client-store";
import { requestPlaybackSeek } from "../workflows/playback";
import OperationResult from "./ui/OperationResult";
import type { TranscriptViewState } from "../models/contentSession";
import { log, toErrorFields } from "../repositories";
import { recordActionFailure } from "../state/notifications-store";
import { useAppShellStore } from "../state/app-shell-store";
import { transcriptOwnsScrollKey } from "../utils/viewerKeys";
import { passiveScrollKey } from "./ui/PassiveScrollRegion";

interface TranscriptSegment {
  seconds: number;
  timestamp: string;
  text: string;
}

type SessionOwner = "installation" | "position" | "visibility";

// Which pending change failed; each owner names itself in its dismiss label.
const SESSION_DISMISS_LABEL: Record<SessionOwner, MessageKey> = {
  installation: "common.closeInstallationResult",
  position: "transcript.closePositionResult",
  visibility: "transcript.closeVisibilityResult",
};

function selectionOffsets(root: HTMLElement): [number, number] | null {
  const selection = window.getSelection();
  if (selection === null || selection.rangeCount === 0 || selection.isCollapsed)
    return null;
  const range = selection.getRangeAt(0);
  if (!root.contains(range.commonAncestorContainer)) return null;
  const beforeStart = range.cloneRange();
  beforeStart.selectNodeContents(root);
  beforeStart.setEnd(range.startContainer, range.startOffset);
  const beforeEnd = range.cloneRange();
  beforeEnd.selectNodeContents(root);
  beforeEnd.setEnd(range.endContainer, range.endOffset);
  return [beforeStart.toString().length, beforeEnd.toString().length];
}

function textPoint(root: HTMLElement, offset: number): [Node, number] {
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  let remaining = Math.max(0, offset);
  let last: Text | null = null;
  while (walker.nextNode()) {
    const node = walker.currentNode as Text;
    last = node;
    if (remaining <= node.data.length) return [node, remaining];
    remaining -= node.data.length;
  }
  return last === null ? [root, 0] : [last, last.data.length];
}

function restoreSelection(
  root: HTMLElement,
  offsets: [number, number] | null,
): void {
  if (offsets === null) return;
  const selection = window.getSelection();
  if (selection === null) return;
  const range = document.createRange();
  const start = textPoint(root, offsets[0]);
  const end = textPoint(root, offsets[1]);
  range.setStart(start[0], start[1]);
  range.setEnd(end[0], end[1]);
  selection.removeAllRanges();
  selection.addRange(range);
}

export function parseTranscript(text: string): TranscriptSegment[] {
  return text
    .split(/\r?\n/)
    .filter((line) => line.trim() !== "")
    .map((line) => {
      const match = line.match(/^\[(\d+):(\d{2})\]\s?(.*)$/);
      if (match === null) return { seconds: 0, timestamp: "", text: line };
      return {
        seconds: Number(match[1]) * 60 + Number(match[2]),
        timestamp: `${match[1]}:${match[2]}`,
        text: match[3],
      };
    });
}

export default function TranscriptBlock({
  hash,
  medium,
  variant = "preview",
  work = null,
}: {
  hash: string;
  medium: "video" | "audio";
  variant?: "preview" | "details";
  /** Backend-authored item projection when the owning surface has it. The
   * transcript store still owns content and manual-action lifecycle. */
  work?: ItemWorkState | null;
}) {
  const { t, text, percent } = useI18n();
  const inDetails = variant === "details";
  const view = useTranscriptStore((state) => state.rows[hash]);
  const load = useTranscriptStore((state) => state.load);
  const start = useTranscriptStore((state) => state.start);
  const cancel = useTranscriptStore((state) => state.cancel);
  const tools = useBinariesStore((state) => state.entries);
  const paused = useDerivedWorkStore((state) => {
    const active = state.activeItem;
    return state.snapshot?.pausedClasses.includes(`${medium}-transcripts`) === true ||
      (active?.id === `${medium}-transcripts` && active.stopping);
  });
  const configuredAutomatic = useAppStore((state) => {
    const config = state.appData?.config;
    if (config === null || config === undefined) return null;
    return medium === "video"
      ? config.videoTranscriptionEnabled !== false
      : config.audioTranscriptionEnabled !== false;
  });
  const auxiliaryAutomatic = useWindowPreferencesStore((state) =>
    medium === "video"
      ? state.videoTranscriptionEnabled
      : state.audioTranscriptionEnabled,
  );
  const automaticEnabled = configuredAutomatic ?? auxiliaryAutomatic;
  const transcriptOpen = useContentSessionStore(
    (state) => state.transcriptOpen[medium],
  );
  const playback = usePlaybackClientStore((state) =>
    state.session?.key === hash ? state.session : null,
  );
  const transcriptView = useContentSessionStore(
    (state) => state.transcriptViews[hash],
  );
  const transcriptRef = useRef<HTMLOListElement | null>(null);
  const panelRef = useRef<HTMLElement | null>(null);
  const transcriptViewRequest = useRef(0);
  const [sessionErrors, setSessionErrors] = useState<Partial<Record<SessionOwner, Message>>>({});
  const visibilityRequest = useRef(0);
  const state = view ?? {
    status: "loading" as const,
    text: null,
    message: null,
    percent: null,
    replacement: null,
  };

  useEffect(() => {
    void load(hash);
  }, [hash, load]);

  useEffect(() => {
    const element = transcriptRef.current;
    if (inDetails || element === null || transcriptView === undefined) return;
    if (panelRef.current !== null) panelRef.current.scrollTop = transcriptView.scrollTop;
    restoreSelection(element, transcriptView.selection);
  }, [inDetails, state.text, transcriptOpen, transcriptView]);

  const ffmpegInstalled = tools.some(
    (entry) => entry.id === "ffmpeg" && entry.status !== "not-installed",
  );
  const modelInstalled = tools.some(
    (entry) =>
      entry.id === "whisper-large-v3-turbo" && entry.status !== "not-installed",
  );
  const toolsAvailable = ffmpegInstalled && modelInstalled;
  const unavailable =
    work !== null ? work.state === "unavailable" : !toolsAvailable;
  const waiting =
    paused || work?.state === "blocked" || work?.state === "waiting";
  const projectedRunning = work?.state === "running";
  const projectedProgress =
    work?.done !== null &&
    work?.done !== undefined &&
    work.total !== null &&
    work.total > 0
      ? work.total === 100
        ? percent(Math.min(100, Math.round(work.done)) / 100)
        : `${work.done}/${work.total}`
      : null;

  const reportSessionFailure = (
    owner: SessionOwner,
    kind: string,
    failure: Message,
    error: unknown,
  ) => {
    log.warn("content session change failed", { kind, ...toErrorFields(error) });
    setSessionErrors((current) => ({ ...current, installation: undefined, [owner]: failure }));
    recordActionFailure(kind, failure, error);
  };

  useEffect(() => {
    if (inDetails) return;
    let active = true;
    void installContentSessionClient().catch(() => {
      if (active) {
        setSessionErrors((current) => ({
          ...current,
          installation: message("transcript.viewSyncFailed"),
        }));
      }
    });
    return () => { active = false; };
  }, [inDetails]);

  const retainTranscriptView = (next: TranscriptViewState) => {
    const request = ++transcriptViewRequest.current;
    void setTranscriptView(hash, next)
      .then(() => {
        if (request !== transcriptViewRequest.current) return;
        setSessionErrors((current) => ({
          ...current,
          installation: undefined,
          position: undefined,
        }));
      })
      .catch((error) => {
        if (request !== transcriptViewRequest.current) return;
        reportSessionFailure(
          "position",
          "transcript-position-change-failed",
          message("transcript.positionRetainFailed"),
          error,
        );
      });
  };

  // The replacement's own reason is OneCopy's sentence; the work projection's
  // is the core's recorded words.
  const replacementNotice =
    state.replacement?.status === "failed" ? (
      <OperationResult level="error" className="mb-2">
        {t("transcript.replacementFailed", {
          reason: state.replacement.message ?? "",
        })}
      </OperationResult>
    ) : state.replacement !== null ? (
      <p className="mb-2 text-xs text-primary">
        {state.replacement.status === "running" &&
        state.replacement.percent !== null
          ? t("transcript.updatingProgress", {
              progress: percent(state.replacement.percent / 100),
            })
          : t("transcript.updating")}
      </p>
    ) : state.status === "ready" && work?.state === "failed" ? (
      <OperationResult level="error" className="mb-2">
        {t("transcript.replacementFailed", { reason: reasonText(work.reason, t) ?? "" })}
      </OperationResult>
    ) : state.status === "ready" && projectedRunning ? (
      <p className="mb-2 text-xs text-primary">
        {projectedProgress === null
          ? t("transcript.updating")
          : t("transcript.updatingProgress", { progress: projectedProgress })}
      </p>
    ) : null;

  let content: React.ReactNode;
  if (state.status !== "ready" && (state.status === "running" || projectedRunning)) {
    const progress =
      state.status === "running" ? state.percent : projectedProgress;
    content = (
      <p className="text-xs text-primary">
        {progress === null
          ? t("transcript.transcribing")
          : t("transcript.transcribingProgress", {
              progress:
                typeof progress === "number" ? percent(progress / 100) : progress,
            })}
      </p>
    );
  } else if (state.status === "failed" || (state.status !== "ready" && work?.state === "failed")) {
    content = (
      <OperationResult level="error">
        {(state.status === "failed"
          ? state.message === null
            ? null
            : text(state.message)
          : reasonText(work?.reason, t)) ?? t("transcript.failed")}
      </OperationResult>
    );
  } else if (state.status === "ready") {
    content =
      state.text === null || state.text.trim() === "" ? (
        <p className="text-xs text-ink-muted">{t("transcript.noSpeech")}</p>
      ) : (
        <ol
          ref={transcriptRef}
          className="select-text font-sans text-sm leading-relaxed text-ink"
          onMouseUp={inDetails ? undefined : (event) => {
            retainTranscriptView({
              scrollTop: panelRef.current?.scrollTop ?? 0,
              selection: selectionOffsets(event.currentTarget),
            });
          }}
          onKeyUp={inDetails ? undefined : (event) => {
            retainTranscriptView({
              scrollTop: panelRef.current?.scrollTop ?? 0,
              selection: selectionOffsets(event.currentTarget),
            });
          }}
        >
          {parseTranscript(state.text).map((segment, index, segments) => {
            const current =
              playback !== null &&
              playback.position >= segment.seconds &&
              (segments[index + 1] === undefined ||
                playback.position < segments[index + 1].seconds);
            return (
              <li
                key={`${segment.seconds}-${index}`}
                className={`flex items-baseline gap-2 rounded px-1 py-0.5 ${
                  current ? "bg-primary-surface" : ""
                }`}
              >
                {segment.timestamp !== "" ? (
                  <button
                    className="shrink-0 font-mono text-xs text-primary hover:underline"
                    title={t("preview.playFrom", { time: segment.timestamp })}
                    onClick={() => requestPlaybackSeek(hash, segment.seconds)}
                  >
                    {segment.timestamp}
                  </button>
                ) : null}
                <span className="min-w-0 whitespace-pre-wrap break-words">{segment.text}</span>
              </li>
            );
          })}
        </ol>
      );
  } else if (unavailable) {
    content = (
      <p className="text-xs text-ink-muted">
        {work?.reason === null || work?.reason === undefined
          ? t("transcript.unavailable")
          : t("transcript.unavailableReason", { reason: reasonText(work.reason, t) ?? "" })}
      </p>
    );
  } else if (state.status === "queued") {
    content = (
      <p className="text-xs text-ink-muted">{t("transcript.queued")}</p>
    );
  } else if (waiting) {
    content = (
      <p className="text-xs text-ink-muted">
        {reasonText(work?.reason, t) ?? t("transcript.paused")}
      </p>
    );
  } else if (state.status === "loading") {
    content = (
      <p className="text-xs text-ink-muted">{t("transcript.loading")}</p>
    );
  } else if (work?.state === "disabled") {
    content = (
      <p className="text-xs text-ink-muted">
        {reasonText(work.reason, t) ?? t("transcript.automaticOff")}
      </p>
    );
  } else if (automaticEnabled) {
    content = (
      <p className="text-xs text-ink-muted">{t("transcript.queued")}</p>
    );
  } else {
    content = (
      <p className="text-xs text-ink-muted">{t("transcript.notTranscribed")}</p>
    );
  }

  const controlError = state.controlError ?? null;

  const actions: React.ReactNode[] = [];
  if (!inDetails) {
    const openBackgroundWork = () => {
      useAppShellStore.getState().openUtility("backgroundWork");
      void useDerivedWorkStore.getState().load();
    };
    const openIssues = () => useAppShellStore.getState().openUtility("issues");
    const failed = state.status === "failed" || work?.state === "failed";
    if (state.replacement !== null && state.replacement.status !== "failed") {
      actions.push(
        <Button key="cancel" variant="ghost" onClick={() => void cancel()}>
          {t("transcript.cancelUpdate")}
        </Button>,
        <Button key="work" variant="ghost" onClick={openBackgroundWork}>
          {t("work.title")}
        </Button>,
      );
    } else if (state.status === "ready") {
      actions.push(
        <Button
          key="replace"
          variant="ghost"
          onClick={() => void start(hash, true)}
        >
          {t("transcript.retranscribe")}
        </Button>,
      );
      if (state.replacement?.status === "failed") {
        actions.push(
          <Button key="issues" variant="ghost" onClick={openIssues}>
            {t("issues.title")}
          </Button>,
        );
      }
    } else if (state.status === "running" || projectedRunning) {
      if (state.status === "running") {
        actions.push(
          <Button key="cancel" variant="ghost" onClick={() => void cancel()}>
            {t("common.cancel")}
          </Button>,
        );
      }
      actions.push(
        <Button key="work" variant="ghost" onClick={openBackgroundWork}>
          {t("work.title")}
        </Button>,
      );
    } else if (waiting) {
      actions.push(
        <Button key="work" onClick={openBackgroundWork}>
          {t("work.title")}
        </Button>,
      );
    } else if (unavailable) {
      actions.push(
        <Button
          key="tools"
          onClick={() => useAppShellStore.getState().openUtility("managedTools")}
        >
          {t("app.openManagedTools")}
        </Button>,
      );
      if (failed) {
        actions.push(
          <Button key="issues" variant="ghost" onClick={openIssues}>
            {t("issues.title")}
          </Button>,
        );
      }
    } else if (failed) {
      actions.push(
        <Button key="retry" onClick={() => void start(hash)}>
          {t("common.retry")}
        </Button>,
        <Button key="issues" variant="ghost" onClick={openIssues}>
          {t("issues.title")}
        </Button>,
      );
    } else if (
      work?.state === "disabled" ||
      (!automaticEnabled && work === null)
    ) {
      actions.push(
        <Button key="transcribe" onClick={() => void start(hash)}>
          {t("transcript.transcribeThisFile")}
        </Button>,
      );
    }
  }

  const expanded = inDetails || transcriptOpen;
  return (
    <section
      ref={panelRef}
      data-transcript-scroll={inDetails ? undefined : true}
      tabIndex={inDetails ? undefined : 0}
      aria-label={t("transcript.title")}
      className={`border-t border-border pt-3 ${inDetails ? "mt-3" :
        `min-h-0 shrink-0 overflow-y-auto focus-visible:outline-none ${medium === "video" ? "max-h-[35%]" : "max-h-[45%]"}`}`}
      onKeyDown={inDetails ? undefined : (event) => {
        if (!transcriptOwnsScrollKey(event.nativeEvent)) return;
        const panel = event.currentTarget;
        const action = passiveScrollKey(event.key, event.shiftKey, panel.clientHeight);
        if (action === null) return;
        event.preventDefault();
        event.stopPropagation();
        if (action === "start") panel.scrollTop = 0;
        else if (action === "end") panel.scrollTop = panel.scrollHeight;
        else panel.scrollTop += action;
      }}
      onScroll={inDetails ? undefined : (event) => retainTranscriptView({
        scrollTop: event.currentTarget.scrollTop,
        selection: transcriptView?.selection ?? null,
      })}
    >
      <div className="mb-1.5 flex flex-wrap items-baseline justify-between gap-x-3 gap-y-2">
        <h2 className="text-xs font-semibold uppercase tracking-wide text-ink-muted">
          {t("transcript.title")}
        </h2>
        {!inDetails ? <span className="ml-auto flex flex-wrap items-baseline justify-end gap-2">
          {actions}
          <Button
            variant="ghost"
            onClick={() => {
              const request = ++visibilityRequest.current;
              void setTranscriptOpen(medium, !transcriptOpen)
                .then(() => {
                  if (request !== visibilityRequest.current) return;
                  setSessionErrors((current) => ({
                    ...current,
                    installation: undefined,
                    visibility: undefined,
                  }));
                })
                .catch((error) => {
                  if (request !== visibilityRequest.current) return;
                  reportSessionFailure(
                    "visibility",
                    "transcript-visibility-change-failed",
                    message("transcript.visibilityChangeFailed"),
                    error,
                  );
                });
            }}
          >
            {transcriptOpen ? t("common.collapse") : t("common.expand")}
          </Button>
        </span> : null}
      </div>
      {controlError !== null ? (
        <OperationResult level="error" className="mb-2">
          {text(controlError)}
        </OperationResult>
      ) : null}
      {(Object.entries(sessionErrors) as Array<[SessionOwner, Message | undefined]>).map(([owner, failure]) =>
        failure !== undefined ? (
          <OperationResult
            key={owner}
            level="error"
            className="mb-2"
            onDismiss={() => setSessionErrors((current) => ({ ...current, [owner]: undefined }))}
            dismissLabel={t(SESSION_DISMISS_LABEL[owner])}
          >
            {text(failure)}
          </OperationResult>
        ) : null,
      )}
      {expanded ? (
        <>
          {replacementNotice}
          {content}
        </>
      ) : null}
    </section>
  );
}
