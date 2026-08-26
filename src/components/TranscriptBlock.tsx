import { useEffect } from "react";
import { useBinariesStore } from "../state/binaries-store";
import { useDerivedWorkStore } from "../state/derived-work-store";
import { useTranscriptStore } from "../state/transcript-store";
import Button from "./ui/Button";

export default function TranscriptBlock({
  hash,
  variant = "full",
}: {
  hash: string;
  variant?: "full" | "compact";
}) {
  const view = useTranscriptStore((state) => state.rows[hash]);
  const load = useTranscriptStore((state) => state.load);
  const start = useTranscriptStore((state) => state.start);
  const cancel = useTranscriptStore((state) => state.cancel);
  const tools = useBinariesStore((state) => state.entries);
  const transcriptWork = useDerivedWorkStore((state) =>
    state.snapshot?.classes.find((row) => row.id === "transcripts"),
  );
  const masterPaused = useDerivedWorkStore((state) => state.snapshot?.masterPaused === true);

  useEffect(() => {
    void load(hash);
  }, [hash, load]);

  const state = view ?? {
    status: "loading" as const,
    text: null,
    message: null,
    percent: null,
  };
  const ffmpegInstalled = tools.some(
    (entry) => entry.id === "ffmpeg" && entry.status !== "not-installed",
  );
  const modelInstalled = tools.some(
    (entry) =>
      entry.id === "whisper-large-v3-turbo" && entry.status !== "not-installed",
  );
  const toolsAvailable = ffmpegInstalled && modelInstalled;
  const paused =
    masterPaused || transcriptWork?.state === "paused" || transcriptWork?.state === "stopping";
  const compact = variant === "compact";

  let content: React.ReactNode;
  if (state.status === "loading") {
    content = <p className="text-xs text-ink-muted">Loading transcript…</p>;
  } else if (state.status === "running") {
    content = (
      <p className="text-xs text-primary">
        Transcribing{state.percent !== null ? ` — ${state.percent}%` : "…"}
      </p>
    );
  } else if (state.status === "failed") {
    content = <p className="break-words text-xs text-danger">{state.message ?? "Transcription failed."}</p>;
  } else if (state.status === "ready") {
    content =
      state.text === null || state.text.trim() === "" ? (
        <p className="text-xs text-ink-muted">Checked — no speech found.</p>
      ) : (
        <pre
          className={`select-text overflow-y-auto whitespace-pre-wrap font-sans leading-relaxed text-ink ${
            compact ? "max-h-24 text-xs" : "max-h-48 text-sm"
          }`}
        >
          {state.text}
        </pre>
      );
  } else if (!toolsAvailable) {
    content = (
      <p className="text-xs text-ink-muted">
        Not available — install ffmpeg and the transcription model from Managed tools.
      </p>
    );
  } else if (paused) {
    content = <p className="text-xs text-ink-muted">Queued — transcription is paused.</p>;
  } else {
    content = <p className="text-xs text-ink-muted">Not transcribed yet.</p>;
  }

  let action: React.ReactNode = null;
  if (!compact && state.status === "running") {
    action = (
      <Button variant="ghost" onClick={() => void cancel()}>
        Cancel
      </Button>
    );
  } else if (!compact && (state.status === "pending" || state.status === "failed")) {
    if (paused) {
      action = (
        <Button onClick={() => useDerivedWorkStore.getState().setOpen(true)}>
          Background work
        </Button>
      );
    } else if (!toolsAvailable) {
      action = (
        <Button onClick={() => useBinariesStore.getState().setModalOpen(true)}>
          Managed tools
        </Button>
      );
    } else {
      action = (
        <Button onClick={() => void start(hash)}>
          {state.status === "failed" ? "Retry" : "Transcribe"}
        </Button>
      );
    }
  }

  return (
    <section className={compact ? "mt-2" : "border-t border-border pt-3"}>
      <div className="mb-1.5 flex items-center justify-between gap-3">
        <h2 className="text-xs font-semibold uppercase tracking-wide text-ink-muted">
          Transcript
        </h2>
        {action}
      </div>
      {content}
    </section>
  );
}
