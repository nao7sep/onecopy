import { useAppStore } from "../state/app-store";
import type { QuarantineRecord } from "../repositories";
import ModalShell from "./ModalShell";
import Button from "./ui/Button";

// What the user is told when a settings file would not parse.
//
// The core sets the unreadable file aside rather than resetting over it, which
// preserves whatever was in there — but a set-aside nobody mentions is just a
// silent reset with extra steps (storage-path-conventions). So this surface is
// half of that recovery, not a courtesy: it names the file it could not read,
// the exact path the original bytes are waiting at, what the app is running on
// instead, and what it did NOT touch. Dismissible, because there is nothing to
// decide — the recovery already happened.

/** What starting over means for each store, in the user's terms. Falls back to
 * a neutral phrasing so an unlisted store still reports honestly. */
function startedWith(file: string): string {
  switch (file) {
    case "config.json":
      return "OneCopy started with its built-in settings, and wrote a fresh settings file.";
    case "state.json":
      return "OneCopy started with a fresh view — sort order, the last open month, and pane widths are back to their defaults.";
    default:
      return "OneCopy started with its built-in defaults for that file.";
  }
}

function Record({ record }: { record: QuarantineRecord }) {
  return (
    <li className="rounded-lg border border-border p-3">
      <p className="text-sm text-ink-strong">
        <span className="font-semibold">{record.file}</span> could not be read.
      </p>
      <p className="mt-1 text-sm text-ink">{startedWith(record.file)}</p>
      <p className="mt-2 text-xs text-ink-muted">Your original file is kept here:</p>
      {/* Selectable: the whole point is that the user can go get these bytes. */}
      <p className="select-text break-all text-xs text-ink">{record.quarantinedTo}</p>
    </li>
  );
}

export default function QuarantineNotice() {
  const quarantines = useAppStore((s) => s.quarantines);
  const dismiss = useAppStore((s) => s.dismissQuarantines);

  if (quarantines.length === 0) return null;

  return (
    <ModalShell title="A settings file could not be read" onClose={dismiss}>
      <ul className="space-y-2">
        {quarantines.map((record) => (
          <Record key={record.quarantinedTo} record={record} />
        ))}
      </ul>
      <p className="mt-3 text-sm text-ink-muted">
        Nothing else was touched: your photos, your trash and the scan index are
        exactly as they were.
      </p>
      <div className="mt-4 flex justify-end">
        <Button variant="primary" onClick={dismiss}>
          OK
        </Button>
      </div>
    </ModalShell>
  );
}
