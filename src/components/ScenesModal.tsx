// The video scenes modal (the developer's design): Enter on an anchor video
// opens a configurable grid of strip frames — default 6×4 — drawn from the
// CACHED strip, so opening costs no ffmpeg work. Short videos carry fewer
// frames by derivation (near-identical neighbours earn nothing), so the grid
// simply shows what exists. Space below the grid is reserved for the deferred
// transcription text. Delete acts from here exactly as from the grid
// (Delete/Backspace, Shift for permanent); moving stays a grid/tree act.

import { useEffect } from "react";
import ModalShell from "./ModalShell";
import { stripUrl } from "../models/items";
import { useItemsStore } from "../state/items-store";
import { useAppStore } from "../state/app-store";

export default function ScenesModal({
  hash,
  onClose,
}: {
  hash: string;
  onClose: () => void;
}) {
  const detail = useItemsStore((s) => s.detail);

  // Delete from the scenes view = delete the anchor video (then close — the
  // subject is gone). The global command layer is suppressed under modals,
  // so the modal owns this binding.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Delete" || event.key === "Backspace") {
        event.preventDefault();
        void useItemsStore
          .getState()
          .deleteSelected(event.shiftKey)
          .then(onClose);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onClose]);

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
      {/* Reserved: the deferred on-demand transcription renders here as one
          text block under the scenes (Design: Video handling). */}
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
          className="w-full rounded border border-border object-cover"
        />
      ))}
    </div>
  );
}

function useGridDim(key: "scenesGridColumns" | "scenesGridRows", fallback: number): number {
  const value = useAppStore((s) => s.appData?.config?.[key]);
  return typeof value === "number" && Number.isFinite(value) && value >= 1
    ? Math.floor(value)
    : fallback;
}
