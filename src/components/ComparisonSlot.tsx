import { factsLine } from "../models/items";
import { faceStarRating } from "../models/itemPresentation";
import type { GroupMember } from "../state/comparison-store";
import { useState } from "react";
import { ExternalLink, Focus, FolderOpen } from "lucide-react";
import InspectableImage from "./InspectableImage";
import { useAppStore } from "../state/app-store";
import { openInDefaultApp } from "../workflows/external-open";
import { log, toErrorFields } from "../repositories";
import FaceRating from "./FaceRating";
import OperationResult from "./ui/OperationResult";

// One comparison card, shared by the main and secondary display surfaces.

export default function ComparisonSlot({
  member,
  slotKey,
  selected,
  anchor,
  onSelect,
  onDecide,
  onReveal,
}: {
  member: GroupMember;
  slotKey: string | null;
  selected: boolean;
  anchor: boolean;
  onSelect: (mode: "exclusive" | "toggle" | "range") => void;
  onDecide: () => void;
  onReveal: () => void;
}) {
  const [externalError, setExternalError] = useState(false);
  const [previewFailed, setPreviewFailed] = useState(false);
  const facts = factsLine(member);
  const showFaceStars = useAppStore(
    (state) => state.appData?.config?.showFaceStars !== false,
  );
  const faceStars = showFaceStars ? faceStarRating(member.faceScore) : 0;
  return (
    <figure
      role="option"
      aria-selected={selected}
      aria-label={`${slotKey === null ? "Image" : `Key ${slotKey.toUpperCase()}`}: ${member.fileName}`}
      className={`group/slot relative flex h-full min-h-0 w-full cursor-pointer flex-col rounded-lg border-2 p-1 ${
        selected
          ? "border-primary bg-primary-surface"
          : "border-border bg-surface"
      } ${
        anchor
          ? "ring-2 ring-primary-ring ring-offset-1 ring-offset-background"
          : ""
      }`}
      onClick={(event) => {
        if (event.detail > 1) return;
        onSelect(
          event.shiftKey
            ? "range"
            : event.metaKey || event.ctrlKey
              ? "toggle"
              : "exclusive",
        );
      }}
      onDoubleClick={() => {
        onSelect("exclusive");
        onDecide();
      }}
      title="Click to select. Double-click to keep only this image on the page."
    >
      <div className="relative flex min-h-0 flex-1 items-center justify-center overflow-hidden">
        <InspectableImage
          hash={member.hash}
          fileName={member.fileName}
          enlargeSmall
          onError={() => setPreviewFailed(true)}
        />
        {previewFailed ? (
          <OperationResult
            level="error"
            className="absolute inset-x-3 top-1/2 -translate-y-1/2 text-sm shadow-sm"
          >
            Preview unavailable. File actions and known details remain available.
          </OperationResult>
        ) : null}
        {externalError ? (
          <OperationResult
            level="error"
            className="absolute bottom-2 left-2 right-2 shadow-sm"
          >
            Couldn’t open this image in its default app.
          </OperationResult>
        ) : null}
      </div>
      {slotKey !== null ? (
        <span
          className={`absolute left-2 top-2 flex h-8 w-8 items-center justify-center rounded text-lg font-bold ${
            selected
              ? "bg-primary text-ink-inverted"
              : "bg-surface-muted text-ink-strong"
          }`}
        >
          {slotKey.toUpperCase()}
        </span>
      ) : null}
      {member.copyCount > 1 ? (
        <span className="absolute right-2 top-2 rounded bg-primary-surface px-1 text-xs text-primary">
          ×{member.copyCount}
        </span>
      ) : null}
      <button
        className="absolute bottom-8 left-2 rounded-md bg-surface-muted p-1 text-ink-muted hover:text-ink"
        aria-label={`Open ${member.fileName} in default app`}
        title="Open in default app"
        onClick={(event) => {
          event.stopPropagation();
          onSelect("exclusive");
          setExternalError(false);
          void openInDefaultApp(member.hash, null).catch((error) => {
            log.warn("comparison external open failed", toErrorFields(error));
            setExternalError(true);
          });
        }}
        onDoubleClick={(event) => event.stopPropagation()}
      >
        <ExternalLink size={14} />
      </button>
      <button
        className="absolute bottom-8 left-9 rounded-md bg-surface-muted p-1 text-ink-muted hover:text-ink"
        aria-label={`Choose a copy of ${member.fileName} to reveal`}
        title="Reveal a physical copy"
        onClick={(event) => {
          event.stopPropagation();
          onSelect("exclusive");
          onReveal();
        }}
        onDoubleClick={(event) => event.stopPropagation()}
      >
        <FolderOpen size={14} />
      </button>
      <figcaption className="mt-1 shrink-0 text-xs text-ink-muted">
        <span className="flex justify-between gap-2">
          <span className="truncate text-ink" title={member.fileName}>
            {member.fileName}
          </span>
          <span className="flex shrink-0 gap-2">
            {/* A score is a best-face confidence/smile hint, not a percentage
                rating. Zero/no-face and unscored stay quiet. */}
            {faceStars !== 0 ? (
              <span className="text-primary">
                <FaceRating stars={faceStars} />
              </span>
            ) : null}
            {member.sharpness !== null ? (
              <span
                className="flex items-center gap-1"
                title="Sharpness (advisory)"
              >
                <Focus size={12} aria-hidden="true" />{" "}
                {Math.round(member.sharpness)}
              </span>
            ) : null}
          </span>
        </span>
        {/* Pixels and bytes are what make the choice possible. A group is very
            often one shot at three qualities — original, export, web copy —
            and at slot size they are the same picture. */}
        {facts !== "" ? (
          <span className="mt-0.5 block tabular-nums">{facts}</span>
        ) : null}
      </figcaption>
    </figure>
  );
}
