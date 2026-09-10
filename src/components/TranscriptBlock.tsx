import { useEffect, useRef, useState } from "react";
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
  type SessionOwner = "installation" | "position" | "visibility";
  const [sessionErrors, setSessionErrors] = useState<Partial<Record<SessionOwner, string>>>({});
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
        ? `${Math.min(100, Math.round(work.done))}%`
        : `${work.done}/${work.total}`
      : null;

  const reportSessionFailure = (
    owner: SessionOwner,
    kind: string,
    message: string,
    error: unknown,
  ) => {
    log.warn("content session change failed", { kind, ...toErrorFields(error) });
    setSessionErrors((current) => ({ ...current, installation: undefined, [owner]: message }));
    recordActionFailure(kind, message, error);
  };

  useEffect(() => {
    if (inDetails) return;
    let active = true;
    void installContentSessionClient().catch(() => {
      if (active) {
        setSessionErrors((current) => ({
          ...current,
          installation: "Transcript view settings could not be synchronized. Try Expand or Collapse again.",
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
          "Couldn’t retain the transcript position.",
          error,
        );
      });
  };

  const replacementNotice =
    state.replacement?.status === "failed" ? (
      <OperationResult level="error" className="mb-2">
        The replacement failed. The previous transcript is still shown.{" "}
        {state.replacement.message}
      </OperationResult>
    ) : state.replacement !== null ? (
      <p className="mb-2 text-xs text-primary">
        Updating transcript
        {state.replacement.status === "running" &&
        state.replacement.percent !== null
          ? ` — ${state.replacement.percent}%`
          : "…"}
        {
          " The previous transcript remains available until the replacement is ready."
        }
      </p>
    ) : state.status === "ready" && work?.state === "failed" ? (
      <OperationResult level="error" className="mb-2">
        The replacement failed. The previous transcript is still shown.{" "}{work.reason}
      </OperationResult>
    ) : state.status === "ready" && projectedRunning ? (
      <p className="mb-2 text-xs text-primary">
        Updating transcript{projectedProgress === null ? "…" : ` — ${projectedProgress}`}
        {" The previous transcript remains available until the replacement is ready."}
      </p>
    ) : null;

  let content: React.ReactNode;
  if (state.status !== "ready" && (state.status === "running" || projectedRunning)) {
    const progress =
      state.status === "running" ? state.percent : projectedProgress;
    content = (
      <p className="text-xs text-primary">
        Transcribing
        {progress !== null
          ? ` — ${progress}${typeof progress === "number" ? "%" : ""}`
          : "…"}
      </p>
    );
  } else if (state.status === "failed" || (state.status !== "ready" && work?.state === "failed")) {
    content = (
      <OperationResult level="error">
        {state.status === "failed"
          ? (state.message ?? "Transcription failed.")
          : (work?.reason ?? "Transcription failed.")}
      </OperationResult>
    );
  } else if (state.status === "ready") {
    content =
      state.text === null || state.text.trim() === "" ? (
        <p className="text-xs text-ink-muted">Checked — no speech found.</p>
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
                    title={`Play from ${segment.timestamp}`}
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
        Not available —{" "}
        {work?.reason ??
          "install ffmpeg and the transcription model from Managed tools"}
        .
      </p>
    );
  } else if (state.status === "queued") {
    content = (
      <p className="text-xs text-ink-muted">Queued for transcription.</p>
    );
  } else if (waiting) {
    content = (
      <p className="text-xs text-ink-muted">
        {work?.reason ?? "Queued — transcription is paused."}
      </p>
    );
  } else if (state.status === "loading") {
    content = <p className="text-xs text-ink-muted">Loading transcript…</p>;
  } else if (work?.state === "disabled") {
    content = (
      <p className="text-xs text-ink-muted">
        {work.reason ?? "Automatic transcription is off."}
      </p>
    );
  } else if (automaticEnabled) {
    content = (
      <p className="text-xs text-ink-muted">Queued for transcription.</p>
    );
  } else {
    content = <p className="text-xs text-ink-muted">Not transcribed yet.</p>;
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
          Cancel update
        </Button>,
        <Button key="work" variant="ghost" onClick={openBackgroundWork}>
          Background work
        </Button>,
      );
    } else if (state.status === "ready") {
      actions.push(
        <Button
          key="replace"
          variant="ghost"
          onClick={() => void start(hash, true)}
        >
          Re-transcribe
        </Button>,
      );
      if (state.replacement?.status === "failed") {
        actions.push(
          <Button key="issues" variant="ghost" onClick={openIssues}>
            Issues
          </Button>,
        );
      }
    } else if (state.status === "running" || projectedRunning) {
      if (state.status === "running") {
        actions.push(
          <Button key="cancel" variant="ghost" onClick={() => void cancel()}>
            Cancel
          </Button>,
        );
      }
      actions.push(
        <Button key="work" variant="ghost" onClick={openBackgroundWork}>
          Background work
        </Button>,
      );
    } else if (waiting) {
      actions.push(
        <Button key="work" onClick={openBackgroundWork}>
          Background work
        </Button>,
      );
    } else if (unavailable) {
      actions.push(
        <Button
          key="tools"
          onClick={() => useAppShellStore.getState().openUtility("managedTools")}
        >
          Managed tools
        </Button>,
      );
      if (failed) {
        actions.push(
          <Button key="issues" variant="ghost" onClick={openIssues}>
            Issues
          </Button>,
        );
      }
    } else if (failed) {
      actions.push(
        <Button key="retry" onClick={() => void start(hash)}>
          Retry
        </Button>,
        <Button key="issues" variant="ghost" onClick={openIssues}>
          Issues
        </Button>,
      );
    } else if (
      work?.state === "disabled" ||
      (!automaticEnabled && work === null)
    ) {
      actions.push(
        <Button key="transcribe" onClick={() => void start(hash)}>
          Transcribe this file
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
      aria-label="Transcript"
      className={`border-t border-border pt-3 ${inDetails ? "mt-3" :
        `min-h-0 shrink-0 overflow-y-auto focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-primary-ring ${medium === "video" ? "max-h-[35%]" : "max-h-[45%]"}`}`}
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
          Transcript
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
                    "Couldn’t change the transcript view.",
                    error,
                  );
                });
            }}
          >
            {transcriptOpen ? "Collapse" : "Expand"}
          </Button>
        </span> : null}
      </div>
      {controlError !== null ? (
        <OperationResult level="error" className="mb-2">
          {controlError}
        </OperationResult>
      ) : null}
      {(Object.entries(sessionErrors) as Array<[SessionOwner, string | undefined]>).map(([owner, message]) =>
        message ? (
          <OperationResult
            key={owner}
            level="error"
            className="mb-2"
            onDismiss={() => setSessionErrors((current) => ({ ...current, [owner]: undefined }))}
            dismissLabel={`Close ${owner} result`}
          >
            {message}
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
