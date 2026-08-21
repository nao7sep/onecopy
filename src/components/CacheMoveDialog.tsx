import ModalShell from "./ModalShell";

export default function CacheMoveDialog({
  copiedBytes,
  totalBytes,
  cancelling,
  onCancel,
}: {
  copiedBytes: number;
  totalBytes: number;
  cancelling: boolean;
  onCancel: () => void;
}) {
  const percent =
    totalBytes > 0 ? Math.max(0, Math.min(100, Math.round((copiedBytes / totalBytes) * 100))) : 0;

  return (
    <ModalShell
      title="Moving cache"
      onClose={onCancel}
      closeLabel={cancelling ? "Cancelling…" : "Cancel"}
      closeDisabled={cancelling}
      widthClass="w-[380px]"
    >
      <div className="py-1">
        <div
          role="progressbar"
          aria-label="Cache move progress"
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={percent}
          className="h-2 w-full overflow-hidden rounded bg-surface-muted"
        >
          <div
            className="h-full bg-primary transition-[width]"
            style={{ width: `${percent}%` }}
          />
        </div>
        <p className="mt-2 text-xs text-ink-muted">
          {(copiedBytes / 1_048_576).toFixed(0)} MB of{" "}
          {(totalBytes / 1_048_576).toFixed(0)} MB — the old location stays active until every
          file is verified.
        </p>
      </div>
    </ModalShell>
  );
}
