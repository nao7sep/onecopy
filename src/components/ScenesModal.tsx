// The video scenes modal (the developer's design): Enter on an anchor video
// opens a configurable grid of strip frames — default 6×4 — drawn from the
// CACHED strip, so opening costs no ffmpeg work. Short videos carry fewer
// frames by derivation (near-identical neighbours earn nothing), so the grid
// simply shows what exists. Below the grid: the on-demand TRANSCRIPT —
// cached after the first run, one Transcribe click otherwise, with the
// missing-model case naming its remedy. Delete acts from here exactly as
// from the grid (Delete/Backspace, Shift for permanent); moving stays a
// grid/tree act.

import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import ModalShell from "./ModalShell";
import ConfirmDialog from "./ConfirmDialog";
import Button from "./ui/Button";
import { stripUrl } from "../models/items";
import { useItemsStore } from "../state/items-store";
import { useAppStore } from "../state/app-store";
import { useBinariesStore } from "../state/binaries-store";
import { log, toErrorFields } from "../repositories";

export default function ScenesModal({
  hash,
  onClose,
}: {
  hash: string;
  onClose: () => void;
}) {
  const detail = useItemsStore((s) => s.detail);
  const [confirmPermanent, setConfirmPermanent] = useState(false);

  // Delete from the scenes view = delete the anchor video (then close — the
  // subject is gone). The global command layer is suppressed under modals,
  // so the modal owns this binding.
  //
  // Scoped to THIS video, never the grid's multi-selection: the modal shows
  // one video and its footer promises the key acts on it, so reading the
  // selection behind the modal would destroy files the surface never named.
  // Permanent deletion confirms here as everywhere else — this was the app's
  // only trash-bypassing path without a prompt.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Delete" && event.key !== "Backspace") return;
      event.preventDefault();
      if (event.shiftKey) {
        setConfirmPermanent(true);
        return;
      }
      void useItemsStore
        .getState()
        .deleteKeys(new Set([hash]), false)
        .then(onClose);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [hash, onClose]);

  const frames = detail?.stripFrames ?? 0;

  return (
    <ModalShell
      title={detail?.fileName ?? "Scenes"}
      onClose={onClose}
      widthClass="w-[1100px]"
      footerStart={
        frames === 0
          ? "No snapshots for this video yet — ffmpeg derives them during a scan."
          : `${frames} scenes · Delete/Backspace acts on the video (Shift: permanent)`
      }
    >
      <ScenesGrid hash={hash} frames={frames} />
      <TranscriptSection hash={hash} />
      {confirmPermanent ? (
        <ConfirmDialog
          title="Delete permanently?"
          message={`Permanently delete ${
            detail?.fileName ?? "this video"
          } and every copy? This bypasses the trash and cannot be undone.`}
          confirmLabel="Delete permanently"
          onConfirm={() => {
            setConfirmPermanent(false);
            void useItemsStore
              .getState()
              .deleteKeys(new Set([hash]), true)
              .then(onClose);
          }}
          onCancel={() => setConfirmPermanent(false)}
        />
      ) : null}
    </ModalShell>
  );
}

function ScenesGrid({ hash, frames }: { hash: string; frames: number }) {
  // Grid dimensions are config (scenesGridColumns × scenesGridRows); the
  // strip may hold fewer frames than the grid offers — show what exists.
  const columns = useGridDim("scenesGridColumns", 6);
  const rows = useGridDim("scenesGridRows", 4);
  const shown = Math.min(frames, columns * rows);
  if (frames === 0) return null;
  return (
    <div
      className="grid gap-1"
      style={{ gridTemplateColumns: `repeat(${columns}, minmax(0, 1fr))` }}
    >
      {Array.from({ length: shown }, (_, i) => (
        <img
          key={i}
          src={stripUrl(hash, i)}
          alt={`scene ${i + 1}`}
          loading="lazy"
          className="w-full rounded-lg border border-border object-cover"
        />
      ))}
    </div>
  );
}

/** The transcript block under the scenes. States, in order of appearance:
 * cached text (instant) → a Transcribe control → live progress → the text.
 * With the model absent the control is replaced by the honest remedy: what to
 * install and the one click that opens Managed tools. */
function TranscriptSection({ hash }: { hash: string }) {
  const [text, setText] = useState<string | null>(null);
  const [running, setRunning] = useState(false);
  const [percent, setPercent] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const modelInstalled = useBinariesStore((s) =>
    s.entries.some(
      (entry) => entry.id === "whisper-large-v3-turbo" && entry.status !== "not-installed",
    ),
  );

  useEffect(() => {
    let stale = false;
    setText(null);
    setError(null);
    setRunning(false);
    void invoke<string | null>("transcript_get", { hash })
      .then((cached) => {
        if (!stale) setText(cached);
      })
      .catch((err) => log.warn("transcript load failed", toErrorFields(err)));

    const disposers: Array<() => void> = [];
    void listen<{ hash: string; percent: number }>("transcribe://progress", (event) => {
      if (event.payload.hash === hash && !stale) {
        setRunning(true);
        setPercent(event.payload.percent);
      }
    }).then((fn) => disposers.push(fn));
    void listen<{ hash: string; text: string }>("transcribe://done", (event) => {
      if (event.payload.hash === hash && !stale) {
        setRunning(false);
        setText(event.payload.text);
      }
    }).then((fn) => disposers.push(fn));
    void listen<{ hash: string; message: string }>("transcribe://error", (event) => {
      if (event.payload.hash === hash && !stale) {
        setRunning(false);
        setError(event.payload.message);
      }
    }).then((fn) => disposers.push(fn));
    return () => {
      stale = true;
      for (const dispose of disposers) dispose();
    };
  }, [hash]);

  return (
    <div className="mt-4 border-t border-border pt-3">
      <div className="mb-2 flex items-center justify-between gap-3">
        <h2 className="text-xs font-semibold uppercase tracking-wide text-ink-muted">
          Transcript
        </h2>
        {running ? (
          <span className="flex items-center gap-2 text-xs text-primary">
            Transcribing… {percent}%
            <Button
              variant="ghost"
              onClick={() => {
                void invoke("transcribe_cancel");
              }}
            >
              Cancel
            </Button>
          </span>
        ) : text === null && modelInstalled ? (
          <Button
            onClick={() => {
              setError(null);
              setRunning(true);
              setPercent(0);
              void invoke("transcribe", { hash }).catch((err) => {
                setRunning(false);
                log.error("transcribe start failed", toErrorFields(err));
              });
            }}
          >
            Transcribe
          </Button>
        ) : null}
      </div>
      {error !== null ? (
        <p className="text-xs text-danger">{error}</p>
      ) : text !== null ? (
        text.trim() === "" ? (
          <p className="text-xs text-ink-muted">No speech found in this video.</p>
        ) : (
          <pre className="max-h-48 select-text overflow-y-auto whitespace-pre-wrap font-sans text-sm leading-relaxed text-ink">
            {text}
          </pre>
        )
      ) : running ? null : modelInstalled ? (
        <p className="text-xs text-ink-muted">
          Not transcribed yet — the transcript is created once and kept.
        </p>
      ) : (
        <p className="text-xs text-ink-muted">
          Transcription needs the Whisper model.{" "}
          <button
            className="text-primary hover:underline"
            onClick={() => useBinariesStore.getState().setModalOpen(true)}
          >
            Install it from Managed tools
          </button>
          .
        </p>
      )}
    </div>
  );
}

function useGridDim(key: "scenesGridColumns" | "scenesGridRows", fallback: number): number {
  const value = useAppStore((s) => s.appData?.config?.[key]);
  return typeof value === "number" && Number.isFinite(value) && value >= 1
    ? Math.floor(value)
    : fallback;
}
