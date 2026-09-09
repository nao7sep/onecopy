import { useEffect, useRef, useState } from "react";
import { emit } from "@tauri-apps/api/event";
import InspectableImage from "../components/InspectableImage";
import OperationResult from "../components/ui/OperationResult";
import { isComposingEvent } from "../hooks/useComposing";
import { log, toErrorFields } from "../repositories";
import { listenThenAnnounce } from "../utils/handshake";
import { hasOpenModal } from "../utils/modalStack";
import { isEditableTarget } from "../utils/shortcuts";
import type { ComparisonImage } from "../workflows/comparison-image";

export default function ComparisonImageWindow() {
  const [image, setImage] = useState<ComparisonImage | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const imageRef = useRef(image);
  imageRef.current = image;
  const surface = useRef<HTMLDivElement>(null);

  const close = () => {
    const current = imageRef.current;
    if (current === null) return;
    void emit("comparison-image://close", { token: current.token }).catch((error) => {
      log.error("comparison image close failed", toErrorFields(error));
      setFailure("Couldn’t return to Comparison. Close this window to try again.");
    });
  };

  useEffect(() => {
    const stop = listenThenAnnounce<ComparisonImage | null>(
      "comparison-image://state", "comparison-image://ready", setImage,
    );
    surface.current?.focus();
    const onKey = (event: KeyboardEvent) => {
      if (event.defaultPrevented || isComposingEvent(event) || hasOpenModal()
        || isEditableTarget(event.target) || event.metaKey || event.ctrlKey || event.altKey || event.shiftKey) return;
      if (event.key !== " " && event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      if (!event.repeat) close();
    };
    window.addEventListener("keydown", onKey, true);
    return () => { stop(); window.removeEventListener("keydown", onKey, true); };
  }, []);

  return (
    <div ref={surface} tabIndex={-1} aria-label="Comparison image" className="flex h-screen flex-col bg-background">
      <div className="min-h-0 flex-1">
        {image === null ? <p className="p-4 text-ink-muted">Opening image…</p> : (
          <InspectableImage hash={image.member.hash} fileName={image.member.fileName} enlargeSmall
            onError={() => setFailure("Preview unavailable. Return to Comparison for file actions and known details.")} />
        )}
      </div>
      {failure !== null ? <OperationResult className="mx-3 mb-2" level="error">{failure}</OperationResult> : null}
      <footer className="flex shrink-0 items-center justify-between gap-3 border-t border-border bg-surface px-3 py-2 text-sm">
        <span className="min-w-0 truncate text-ink">{image?.member.fileName}</span>
        <span className="text-xs text-ink-muted">Space/Escape: return · Hold: original pixels</span>
        <button className="shrink-0 rounded border border-border px-3 py-1 text-ink hover:bg-surface-muted" onClick={close}>Close</button>
      </footer>
    </div>
  );
}
