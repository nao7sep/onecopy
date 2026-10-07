// Deleted files, as Browse shows one root's listing: pure grouping, date
// bucketing, search and selection over the entries `trash_entries` returns.
// The backend decides what each stored file is (its status); this module only
// arranges and selects, so every rule here is testable without a window.

import type { MutationResult } from "./mutation";

export type TrashKind = "delete" | "move-cleanup" | "overwrite-displaced";
export type TrashRole = "main" | "companion";
export type EntryStatus = "restorable" | "changed" | "unrepresentable" | "excluded";

export interface TrashEntry {
  /** `<day folder>/<stored name>`, unique within one root. */
  id: string;
  day: string;
  storedName: string;
  storedPath: string;
  originalRelative: string;
  deletedAtUtc: string;
  size: number;
  /** The modification time the record promises. */
  mtimeMs: number;
  /** The deleted item this file belongs to (see `groupKey`). */
  group: string;
  kind: TrashKind;
  operation: string;
  item: string | null;
  role: TrashRole;
  movedTo: string | null;
  status: EntryStatus;
  /** For a companion: where a main file of its deleted item in its folder was
   * restored to under another name. */
  mainRestoredAs: string | null;
}

export interface TrashListing {
  entries: TrashEntry[];
  unrecordedFiles: number;
  malformedLines: number;
  /** Record lines a newer OneCopy wrote, left as they are. */
  newerLines: number;
}

/** One logical item removed by one operation: its main copies, the
 * companions paired with them, and duplicate copies. */
export interface DeletionGroup {
  key: string;
  /** Main copies first, then companions, each by folder and name. */
  entries: TrashEntry[];
  representative: TrashEntry;
  /** The latest deletion time among the group's files. */
  deletedAtUtc: string;
  size: number;
  mains: number;
  companions: number;
  kind: TrashKind;
  movedTo: string | null;
}

export interface DayBucket {
  /** The local calendar day, `yyyy-mm-dd`. */
  day: string;
  groups: DeletionGroup[];
}

export function isRestorable(entry: TrashEntry): boolean {
  return entry.status === "restorable";
}

/** The original file name. */
export function entryName(entry: TrashEntry): string {
  const relative = entry.originalRelative;
  const cut = relative.lastIndexOf("/");
  return cut < 0 ? relative : relative.slice(cut + 1);
}

/** The original folder relative to the root; "" for the root itself. */
export function entryFolder(entry: TrashEntry): string {
  const relative = entry.originalRelative;
  const cut = relative.lastIndexOf("/");
  return cut < 0 ? "" : relative.slice(0, cut);
}

/** Which deletion group an entry belongs to, as the backend decides it: one
 * operation's logical item, or for a file without one the operation, folder
 * and stem (the companion rule), so duplicates in other folders stay
 * separate. */
export function groupKey(entry: TrashEntry): string {
  return entry.group;
}

function byPlace(left: TrashEntry, right: TrashEntry): number {
  const leftMain = left.role === "companion" ? 1 : 0;
  const rightMain = right.role === "companion" ? 1 : 0;
  if (leftMain !== rightMain) return leftMain - rightMain;
  const folder = entryFolder(left).localeCompare(entryFolder(right));
  if (folder !== 0) return folder;
  const name = entryName(left).localeCompare(entryName(right));
  return name !== 0 ? name : left.id.localeCompare(right.id);
}

function isCompanion(entry: TrashEntry): boolean {
  return entry.role === "companion";
}

export function groupEntries(entries: readonly TrashEntry[]): DeletionGroup[] {
  const groups = new Map<string, TrashEntry[]>();
  for (const entry of entries) {
    const key = groupKey(entry);
    const members = groups.get(key);
    if (members === undefined) groups.set(key, [entry]);
    else members.push(entry);
  }
  return [...groups.entries()].map(([key, members]) => {
    const sorted = [...members].sort(byPlace);
    const companions = sorted.filter(isCompanion).length;
    const deletedAtUtc = sorted.reduce(
      (latest, entry) => (entry.deletedAtUtc > latest ? entry.deletedAtUtc : latest),
      sorted[0].deletedAtUtc,
    );
    const representative = sorted.find((entry) => !isCompanion(entry)) ?? sorted[0];
    return {
      key,
      entries: sorted,
      representative,
      deletedAtUtc,
      size: sorted.reduce((total, entry) => total + entry.size, 0),
      mains: sorted.length - companions,
      companions,
      kind: representative.kind,
      movedTo: representative.role === "companion" ? null : representative.movedTo,
    };
  });
}

/** The local calendar day of an instant in `timeZone` (the computer's zone
 * when omitted). A UTC day folder can span two local days, so the list is
 * bucketed by this, never by the folder's name. */
export function localDay(iso: string, timeZone?: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "";
  const parts = new Intl.DateTimeFormat("en-CA", {
    timeZone,
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
  }).formatToParts(date);
  const part = (type: string) => parts.find((candidate) => candidate.type === type)?.value ?? "";
  return `${part("year")}-${part("month")}-${part("day")}`;
}

/** Groups bucketed by local day of deletion, newest day and group first. */
export function bucketByDay(
  groups: readonly DeletionGroup[],
  timeZone?: string,
): DayBucket[] {
  const buckets = new Map<string, DeletionGroup[]>();
  for (const group of groups) {
    const day = localDay(group.deletedAtUtc, timeZone);
    const members = buckets.get(day);
    if (members === undefined) buckets.set(day, [group]);
    else members.push(group);
  }
  return [...buckets.entries()]
    .sort(([left], [right]) => right.localeCompare(left))
    .map(([day, members]) => ({
      day,
      groups: [...members].sort(
        (left, right) =>
          right.deletedAtUtc.localeCompare(left.deletedAtUtc) ||
          entryName(left.representative).localeCompare(entryName(right.representative)),
      ),
    }));
}

/** Case-insensitive, Unicode-normalized form for search on both sides. */
export function searchKey(text: string): string {
  return text.normalize("NFC").toLowerCase();
}

/** Groups with at least one file whose name or original folder matches. */
export function filterGroups(
  groups: readonly DeletionGroup[],
  query: string,
): DeletionGroup[] {
  const needle = searchKey(query.trim());
  if (needle === "") return [...groups];
  return groups.filter((group) =>
    group.entries.some(
      (entry) =>
        searchKey(entryName(entry)).includes(needle) ||
        searchKey(entryFolder(entry)).includes(needle),
    ),
  );
}

/** Whether every restorable file of the group is selected (and it has one). */
export function groupSelected(group: DeletionGroup, selected: ReadonlySet<string>): boolean {
  const restorable = group.entries.filter(isRestorable);
  return restorable.length > 0 && restorable.every((entry) => selected.has(entry.id));
}

/** Checking a group selects all of its restorable files; checking it again
 * when all are selected clears them. */
export function toggleGroup(
  group: DeletionGroup,
  selected: ReadonlySet<string>,
): Set<string> {
  const next = new Set(selected);
  const restorable = group.entries.filter(isRestorable);
  if (groupSelected(group, selected)) {
    for (const entry of restorable) next.delete(entry.id);
  } else {
    for (const entry of restorable) next.add(entry.id);
  }
  return next;
}

export function toggleEntry(entry: TrashEntry, selected: ReadonlySet<string>): Set<string> {
  const next = new Set(selected);
  if (!isRestorable(entry)) return next;
  if (next.has(entry.id)) next.delete(entry.id);
  else next.add(entry.id);
  return next;
}

/** Keeps only selected ids a fresh listing still offers as restorable. */
export function reconcileSelection(
  selected: ReadonlySet<string>,
  entries: readonly TrashEntry[],
): Set<string> {
  const restorable = new Set(entries.filter(isRestorable).map((entry) => entry.id));
  return new Set([...selected].filter((id) => restorable.has(id)));
}

/** One row of the virtualized list. */
export type BrowseRow =
  | { type: "day"; key: string; day: string }
  | { type: "group"; key: string; group: DeletionGroup; expanded: boolean }
  | { type: "entry"; key: string; group: DeletionGroup; entry: TrashEntry };

/** Keep the first visible row at its pixel offset; if it vanished, retain
 * the nearest surviving ordinal and clamp to the new viewport extent. */
export function restoredBrowseScroll(
  rows: readonly BrowseRow[],
  anchor: { key: string | null; index: number; offset: number },
  rowHeight: number,
  viewport: number,
): number {
  const found = rows.findIndex((row) => row.key === anchor.key);
  const index = found >= 0 ? found : Math.min(anchor.index, Math.max(0, rows.length - 1));
  return Math.min(Math.max(0, rows.length * rowHeight - viewport), index * rowHeight + anchor.offset);
}

export function browseRows(
  buckets: readonly DayBucket[],
  expanded: ReadonlySet<string>,
): BrowseRow[] {
  const rows: BrowseRow[] = [];
  for (const bucket of buckets) {
    rows.push({ type: "day", key: `day:${bucket.day}`, day: bucket.day });
    for (const group of bucket.groups) {
      const open = expanded.has(group.key);
      rows.push({ type: "group", key: `group:${group.key}`, group, expanded: open });
      if (!open) continue;
      for (const entry of group.entries) {
        rows.push({ type: "entry", key: `entry:${entry.id}`, group, entry });
      }
    }
  }
  return rows;
}

// ---------------------------------------------------------------------------
// Restore

export type RestoreSkip =
  | "already-there"
  | "changed"
  | "missing"
  | "unrepresentable"
  | "excluded"
  | "folder-is-link"
  | "file-in-the-way"
  | "other-drive";

export interface RestoreReviewFile {
  id: string;
  original: string | null;
  /** Where the file goes, relative to the root; null when it is skipped. */
  target: string | null;
  renamed: boolean;
  skip: RestoreSkip | null;
  /** A companion that will not pair with its main file, which was restored
   * earlier under another name: where that main file is. */
  mainRestoredAs: string | null;
}

export interface RestoreReview {
  files: RestoreReviewFile[];
  /** Folders the restore recreates, relative to the root. */
  folders: string[];
  /** Companions of a restored main file that stay in Deleted files. */
  companionsLeft: string[];
}

export interface RestoreOutcome {
  cancelled: boolean;
  error: string | null;
  restored: string[];
  alreadyPresent: number;
  failed: number;
  unknown: number;
  unstarted: number;
  filesTotal: number;
  planToken: string | null;
  requiresReview: boolean;
  planChanged: boolean;
  review: RestoreReview | null;
}

/** The restore's receipt in the shape every file operation's receipt takes. */
export function restoreReceipt(outcome: RestoreOutcome): MutationResult {
  return {
    operationId: 0,
    kind: "restore",
    cancelled: outcome.cancelled,
    summary: {
      itemsCompleted: outcome.restored.length,
      itemsPartial: 0,
      itemsUnstarted: outcome.unstarted,
      filesCompleted: outcome.restored.length,
      filesFailed: Math.max(0, outcome.failed - outcome.unknown),
      filesUnknown: outcome.unknown,
      filesUnstarted: outcome.unstarted,
      filesAlreadyPresent: outcome.alreadyPresent,
      trashAvailable: false,
      error: outcome.error,
    },
  };
}

/** The review's primary action restores anything at all, and renames when any
 * file takes a new name. */
export function reviewAction(review: RestoreReview): "none" | "restore" | "rename-and-restore" {
  const moving = review.files.filter((file) => file.target !== null);
  if (moving.length === 0) return "none";
  return moving.some((file) => file.renamed) ? "rename-and-restore" : "restore";
}
