import { useEffect, useState } from "react";
import {
  useDestinationsStore,
  type DirEntry,
} from "../state/destinations-store";
import { useComposing, isComposingKeyboardEvent } from "../hooks/useComposing";
import ConfirmDialog from "./ConfirmDialog";
import ModalShell from "./ModalShell";
import { ChevronDown, ChevronRight } from "lucide-react";
import {
  addDestinationRoot,
  moveDestinationSelectionTo,
  confirmDestinationDeleteRest,
  moveSelectionTo,
  removeDestinationRoot,
  type MoveMode,
} from "../workflows/destinations";
import type { PendingDestinationDrop } from "../models/destinationTransfer";

// The right pane's destination tree, mirroring the sidebar's interaction
// (redesigned 2026-08-17, developer-approved): one composite tree with the
// sidebar's chevrons and rows, NO hover buttons and NO per-row buttons — the
// commands live in one persistent action bar under the tree, acting on the
// active row. Hover-revealed controls existed nowhere else in the app, they
// hid the Copy command well enough that the developer asked for it as a
// missing feature, and their width stole the path's. Root rows carry their
// full path as a second muted line; every other row is single-line,
// full-width. Move/copy work on ANY row — a root (the developer sorts the
// rest by hand) or a subfolder, recursively discovered.

type MoveModifiers = Pick<
  KeyboardEvent,
  "altKey" | "ctrlKey" | "metaKey" | "shiftKey"
>;

export function keyboardMoveMode(event: MoveModifiers): MoveMode | null {
  if (event.altKey && (event.metaKey || event.ctrlKey)) return null;
  if (event.metaKey || event.ctrlKey) return "copy";
  if (event.shiftKey) return "move-delete-rest";
  return "move-trash-rest";
}

/** Expandability from the LIVE children map first, the listing's flag second.
 * The listing's `hasChildren` is a snapshot taken when the PARENT was listed
 * — after "New subfolder" inside a leaf it still says false, which is exactly
 * how a freshly created folder used to be invisible until restart. */
export function nodeHasChildren(
  entry: { path: string; hasChildren: boolean },
  children: Record<string, DirEntry[]>,
): boolean {
  const listed = children[entry.path];
  return listed !== undefined ? listed.length > 0 : entry.hasChildren;
}

const EMPTY_DIR_ENTRIES: DirEntry[] = [];

/** The last path segment — the row label for roots. */
function leafName(path: string): string {
  const trimmed = path.replace(/[/\\]+$/, "");
  const cut = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return cut >= 0 ? trimmed.slice(cut + 1) || trimmed : trimmed;
}

function DirNode({
  entry,
  depth,
}: {
  entry: DirEntry;
  depth: number;
}) {
  const expanded = useDestinationsStore((s) => s.expanded);
  const children = useDestinationsStore((s) => s.children);
  const emptiness = useDestinationsStore((s) => s.emptiness);
  const toggleExpand = useDestinationsStore((s) => s.toggleExpand);
  const isOpen = expanded.has(entry.path);
  const isEmpty = emptiness[entry.path] === true;
  const hasChildren = nodeHasChildren(entry, children);
  const activePath = useDestinationsStore((s) => s.activePath);
  const setActive = useDestinationsStore((s) => s.setActive);
  const isActive = activePath === entry.path;
  const dropReady = useDestinationsStore(
    (state) => state.dragReceiverPath === entry.path,
  );

  return (
    <li
      id={`tree-${encodeURIComponent(entry.path)}`}
      role="treeitem"
      aria-selected={isActive}
      aria-expanded={hasChildren ? isOpen : undefined}
    >
      <div
        data-tree-path={entry.path}
        data-destination-receiver={entry.path}
        className={`flex items-center rounded-md px-1.5 py-1 text-sm transition-colors ${
          dropReady
            ? "bg-primary-surface ring-2 ring-primary"
            : isActive
              ? "bg-primary-surface ring-1 ring-primary-ring"
              : "hover:bg-surface-muted"
        }`}
        style={{ paddingLeft: `${depth * 12}px` }}
        onClick={() => setActive(entry.path)}
      >
        <button
          tabIndex={-1}
          className="w-4 shrink-0 text-ink-muted"
          onClick={() => void toggleExpand(entry.path)}
          title={hasChildren ? (isOpen ? "Collapse" : "Expand") : undefined}
        >
          {hasChildren ? (isOpen ? <ChevronDown size={13} /> : <ChevronRight size={13} />) : "\u00b7"}
        </button>
        <span
          className={`min-w-0 flex-1 truncate ${
            isEmpty ? "italic text-ink-muted" : "text-ink"
          }`}
          title={entry.path}
        >
          {entry.name}
        </span>
      </div>
      {isOpen ? (
        <ChildNodes path={entry.path} depth={depth + 1} />
      ) : null}
    </li>
  );
}

function ChildNodes({ path, depth }: { path: string; depth: number }) {
  const entries = useDestinationsStore((s) => s.children[path]) ?? EMPTY_DIR_ENTRIES;
  const status = useDestinationsStore((s) => s.listing[path]);
  return (
    <ul role="group">
      {status === "loading" ? (
        <li role="none" className="py-1 text-xs text-ink-muted" style={{ paddingLeft: `${depth * 12 + 6}px` }}>
          {entries.length > 0 ? "Refreshing folders…" : "Reading folders…"}
        </li>
      ) : status === "error" ? (
        <li role="none" className="py-1 text-xs text-danger" style={{ paddingLeft: `${depth * 12 + 6}px` }}>
          Couldn’t read this folder.
        </li>
      ) : entries.length === 0 ? (
        <li role="none" className="py-1 text-xs text-ink-muted" style={{ paddingLeft: `${depth * 12 + 6}px` }}>
          No subfolders
        </li>
      ) : null}
      {entries.map((child) => (
        <DirNode key={child.path} entry={child} depth={depth} />
      ))}
    </ul>
  );
}

function RootRow({ root, isOpen }: { root: string; isOpen: boolean }) {
  const toggleExpand = useDestinationsStore((s) => s.toggleExpand);
  const activePath = useDestinationsStore((s) => s.activePath);
  const setActive = useDestinationsStore((s) => s.setActive);
  const isActive = activePath === root;
  const dropReady = useDestinationsStore(
    (state) => state.dragReceiverPath === root,
  );
  return (
    <li
      id={`tree-${encodeURIComponent(root)}`}
      className="mb-1"
      role="treeitem"
      aria-selected={isActive}
      aria-expanded={isOpen}
    >
      <div
        data-tree-path={root}
        data-destination-receiver={root}
        className={`flex items-start rounded-md px-1.5 py-1 text-sm transition-colors ${
          dropReady
            ? "bg-primary-surface ring-2 ring-primary"
            : isActive
              ? "bg-primary-surface ring-1 ring-primary-ring"
              : "hover:bg-surface-muted"
        }`}
        onClick={() => setActive(root)}
      >
        <button
          tabIndex={-1}
          className="w-4 shrink-0 pt-0.5 text-ink-muted"
          onClick={() => void toggleExpand(root)}
        >
          {isOpen ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
        </button>
        {/* Roots get TWO lines — name, then the full path in muted small
            text. Only here: a root is where truncation actually hurt (the
            path IS its identity), while hundreds of subfolder rows doubling
            in height would halve the visible tree. */}
        <span className="min-w-0 flex-1">
          <span className="block truncate font-medium text-ink-strong">
            {leafName(root)}
          </span>
          <span className="block truncate text-[11px] text-ink-muted" title={root}>
            {root}
          </span>
        </span>
      </div>
      {isOpen ? (
        <ChildNodes path={root} depth={1} />
      ) : null}
    </li>
  );
}

// The flattened render order of visible rows — what Up/Down move across.
interface VisibleRow {
  path: string;
  parent: string | null;
  hasChildren: boolean;
  isExpanded: boolean;
}

function visibleRows(
  roots: string[],
  children: Record<string, DirEntry[]>,
  expanded: Set<string>,
): VisibleRow[] {
  const rows: VisibleRow[] = [];
  const walk = (path: string, parent: string | null, hasChildren: boolean) => {
    const isExpanded = expanded.has(path);
    rows.push({ path, parent, hasChildren, isExpanded });
    if (isExpanded) {
      for (const child of children[path] ?? []) {
        walk(child.path, path, nodeHasChildren(child, children));
      }
    }
  };
  for (const root of roots) walk(root, null, true);
  return rows;
}

/** The persistent command bar under the tree: every command the old hover
 * buttons carried, acting on the ACTIVE row, always visible at full width. */
function ActionBar() {
  const activePath = useDestinationsStore((s) => s.activePath);
  const roots = useDestinationsStore((s) => s.roots);
  const emptiness = useDestinationsStore((s) => s.emptiness);
  const createFolder = useDestinationsStore((s) => s.createFolder);
  const deleteFolder = useDestinationsStore((s) => s.deleteFolder);
  const [creating, setCreating] = useState(false);
  const [name, setName] = useState("");
  const { composingRef, handlers: composingHandlers } = useComposing();

  if (activePath === null) {
    return (
      <p className="mt-2 shrink-0 text-xs text-ink-muted">
        Select a folder to move or copy into it.
      </p>
    );
  }
  const isRoot = roots.includes(activePath);
  const parent = isRoot
    ? null
    : activePath.slice(
        0,
        Math.max(activePath.lastIndexOf("/"), activePath.lastIndexOf("\\")),
      );

  const button =
    "inline-flex h-7 items-center rounded-md px-2 text-xs font-medium transition-colors";

  return (
    <div className="mt-2 shrink-0 border-t border-border pt-2">
      <p className="mb-1 truncate text-sm font-medium text-ink-strong" title={activePath}>
        {leafName(activePath)}
      </p>
      <div className="flex flex-wrap items-center gap-1">
        <button
          className={`${button} text-primary hover:bg-primary-surface`}
          title="Move the selection here; its other copies go to trash (Enter). Shift: delete them permanently."
          onClick={(e) =>
            void moveSelectionTo(
              activePath,
              e.shiftKey ? "move-delete-rest" : "move-trash-rest",
            )
          }
        >
          Move here
        </button>
        <button
          className={`${button} text-ink hover:bg-surface-muted`}
          title="Copy the selection here; nothing else is touched (Cmd/Ctrl+Enter)"
          onClick={() => void moveSelectionTo(activePath, "copy")}
        >
          Copy here
        </button>
        {creating ? (
          <input
            autoFocus
            className="h-7 w-32 rounded-md border border-border bg-background px-2 text-xs text-ink outline-none focus-visible:ring-2 focus-visible:ring-primary-ring"
            value={name}
            placeholder="folder name"
            onChange={(e) => setName(e.target.value)}
            {...composingHandlers}
            onKeyDown={(e) => {
              // Enter/Escape during IME composition belong to the IME: Enter
              // confirms the candidate (never commits a half-resolved name to
              // disk), Escape cancels it (never destroys the edit).
              if (isComposingKeyboardEvent(composingRef, e)) {
                e.stopPropagation();
                return;
              }
              if (e.key === "Enter" && name.trim() !== "") {
                void createFolder(activePath, name.trim());
                setCreating(false);
                setName("");
              } else if (e.key === "Escape") {
                setCreating(false);
                setName("");
              }
              e.stopPropagation();
            }}
          />
        ) : (
          <button
            className={`${button} text-ink hover:bg-surface-muted`}
            title="Create a subfolder inside this folder"
            onClick={() => setCreating(true)}
          >
            New subfolder
          </button>
        )}
        {!isRoot && emptiness[activePath] === true && parent !== null ? (
          <button
            className={`${button} text-danger hover:bg-danger-surface`}
            title="Delete this empty folder"
            onClick={() => void deleteFolder(activePath, parent)}
          >
            Delete empty
          </button>
        ) : null}
        {isRoot ? (
          <button
            className={`${button} text-ink-muted hover:bg-surface-muted hover:text-ink`}
            title="Remove this root from the list (the folder itself is untouched)"
            onClick={() => void removeDestinationRoot(activePath)}
          >
            Remove root
          </button>
        ) : null}
      </div>
    </div>
  );
}

export default function DestinationsTab() {
  const roots = useDestinationsStore((s) => s.roots);
  const expanded = useDestinationsStore((s) => s.expanded);
  const children = useDestinationsStore((s) => s.children);
  const message = useDestinationsStore((s) => s.message);
  const result = useDestinationsStore((s) => s.result);
  const dismissResult = useDestinationsStore((s) => s.dismissResult);
  const confirmation = useDestinationsStore((s) => s.confirmation);
  const dismissConfirmation = useDestinationsStore((s) => s.dismissConfirmation);
  const activePath = useDestinationsStore((s) => s.activePath);
  const setActive = useDestinationsStore((s) => s.setActive);
  const toggleExpand = useDestinationsStore((s) => s.toggleExpand);

  // Folders created OUTSIDE the app (Finder, Explorer) appear when the pane
  // mounts and whenever the app window regains focus — the exact moment a
  // user returns from making 2026/, 2027/ by hand.
  useEffect(() => {
    void useDestinationsStore.getState().refreshExpanded();
    const onFocus = () => void useDestinationsStore.getState().refreshExpanded();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, []);

  // One composite tree (the conventions' sibling pattern): the container is
  // the single tab stop, Up/Down move across visible rows, Right expands or
  // enters, Left collapses or exits to the parent, Enter moves the grid
  // selection here (Shift = delete-rest, Cmd/Ctrl = copy).
  const onKeyDown = (event: React.KeyboardEvent) => {
    const rows = visibleRows(roots, children, expanded);
    if (rows.length === 0) return;
    const index = activePath !== null ? rows.findIndex((r) => r.path === activePath) : -1;
    const row = index >= 0 ? rows[index] : null;
    // Keeping the active item in view (composite-control conventions) — the
    // rows are rendered, so the marker is queryable immediately.
    const container = event.currentTarget as HTMLElement;
    const activate = (path: string | null) => {
      setActive(path);
      if (path !== null) {
        container
          .querySelector(`[data-tree-path="${CSS.escape(path)}"]`)
          ?.scrollIntoView({ block: "nearest" });
      }
    };

    const vertical = ["ArrowDown", "ArrowUp", "PageDown", "PageUp", "Home", "End"];
    if (vertical.includes(event.key)) {
      event.preventDefault();
      const target =
        event.key === "ArrowDown"
          ? Math.min(index + 1, rows.length - 1)
          : event.key === "ArrowUp"
            ? Math.max(index - 1, 0)
            : event.key === "PageDown"
              ? Math.min(index + 10, rows.length - 1)
              : event.key === "PageUp"
                ? Math.max(index - 10, 0)
                : event.key === "Home"
                  ? 0
                  : rows.length - 1;
      activate(rows[target]?.path ?? null);
    } else if (event.key === "ArrowRight" && row) {
      event.preventDefault();
      if (!row.isExpanded && row.hasChildren) {
        void toggleExpand(row.path);
      } else if (row.isExpanded) {
        const first = (children[row.path] ?? [])[0];
        if (first) activate(first.path);
      }
    } else if (event.key === "ArrowLeft" && row) {
      event.preventDefault();
      if (row.isExpanded) {
        void toggleExpand(row.path);
      } else if (row.parent !== null) {
        activate(row.parent);
      }
    } else if (event.key === "Enter" && row) {
      const mode = keyboardMoveMode(event.nativeEvent);
      if (mode === null) return;
      event.preventDefault();
      event.stopPropagation();
      void moveSelectionTo(row.path, mode);
    }
  };

  const pendingDeleteRest = useDestinationsStore((s) => s.pendingDeleteRest);
  const pendingDrop = useDestinationsStore((s) => s.pendingDrop);

  return (
    <div className="flex h-full flex-col p-3">
      {pendingDrop !== null ? (
        <DropChoiceModal
          drop={pendingDrop}
          onClose={() => useDestinationsStore.getState().setPendingDrop(null)}
        />
      ) : null}
      {pendingDeleteRest !== null ? (
        <ConfirmDialog
          title="Move and delete the rest?"
          message={`Move ${pendingDeleteRest.count} item${
            pendingDeleteRest.count === 1 ? "" : "s"
          } here and PERMANENTLY delete the remaining copies? The deleted copies bypass the trash and cannot be recovered.`}
          confirmLabel="Move and delete permanently"
          onConfirm={() => void confirmDestinationDeleteRest()}
          onCancel={() => useDestinationsStore.getState().cancelPendingDeleteRest()}
        />
      ) : null}
      <div className="mb-2 flex items-center justify-between">
        <h2 className="text-sm font-semibold text-ink-strong">Destinations</h2>
        <button
          className="inline-flex h-8 items-center gap-1.5 rounded-lg border border-border px-3 text-xs font-medium text-ink transition-colors hover:border-border-strong hover:bg-surface-muted"
          onClick={() => void addDestinationRoot()}
        >
          Add root…
        </button>
      </div>
      {/* The container renders (and stays Tab-reachable) even with no
          roots — an empty composite is still a landing place. */}
      <ul
        role="tree"
        data-destination-scroll
        aria-label="Destination folders"
        aria-activedescendant={
          activePath !== null ? `tree-${encodeURIComponent(activePath)}` : undefined
        }
        tabIndex={0}
        className="min-h-0 flex-1 overflow-y-auto outline-none"
        onKeyDown={onKeyDown}
      >
        {roots.length === 0 ? (
          <li role="none" className="text-sm text-ink-muted">
            Add a destination root — the place cleaned-up files move to. It
            must lie outside every scanned directory.
          </li>
        ) : (
          roots.map((root) => {
            const isOpen = expanded.has(root);
            return <RootRow key={root} root={root} isOpen={isOpen} />;
          })
        )}
      </ul>
      <ActionBar />
      {confirmation !== null ? (
        <div
          role="status"
          className="mt-2 flex shrink-0 items-start gap-2 rounded-md border border-border-strong bg-surface-muted px-2.5 py-2 text-xs text-ink"
        >
          <span className="min-w-0 flex-1 break-words">
            <strong className="font-semibold">Done:</strong> {confirmation}
          </span>
          <button
            className="shrink-0 rounded px-1 font-medium underline-offset-2 hover:underline"
            onClick={dismissConfirmation}
            aria-label="Dismiss confirmation"
          >
            Dismiss
          </button>
        </div>
      ) : null}
      {result !== null ? (
        <div
          role={result.severity === "info" ? "status" : "alert"}
          className={`mt-2 flex shrink-0 items-start gap-2 rounded-md border px-2.5 py-2 text-xs ${
            result.severity === "error"
              ? "border-danger bg-danger-surface text-danger"
              : result.severity === "warning"
                ? "border-warning bg-warning-surface text-warning"
                : "border-border-strong bg-surface-muted text-ink"
          }`}
        >
          <span className="min-w-0 flex-1 break-words">
            <strong className="font-semibold">
              {result.severity === "error"
                ? "Error"
                : result.severity === "warning"
                  ? "Needs attention"
                  : "Information"}
              :
            </strong>{" "}
            {result.message}
          </span>
          <button
            className="shrink-0 rounded px-1 font-medium underline-offset-2 hover:underline"
            onClick={dismissResult}
            aria-label="Dismiss result"
          >
            Dismiss
          </button>
        </div>
      ) : null}
      {message !== "" ? (
        <p className="mt-2 shrink-0 break-words text-xs text-ink-muted">{message}</p>
      ) : null}
    </div>
  );
}

/** The drop's Move/Copy question (Phase 33). Dropping used to read modifier
 * keys held at release — a silent decision nobody remembers making with a
 * mouse button down. The permanent variant is deliberately absent here: it
 * stays behind the keyboard chord and its own confirmation. */
function DropChoiceModal({
  drop,
  onClose,
}: {
  drop: PendingDestinationDrop;
  onClose: () => void;
}) {
  const { path, selection } = drop;
  const button =
    "inline-flex h-8 shrink-0 items-center justify-center rounded-lg px-3 text-sm font-medium outline-none transition-colors focus-visible:ring-2 focus-visible:ring-primary-ring";
  return (
    <ModalShell
      title={`Drop into ${leafName(path)}`}
      onClose={onClose}
      widthClass="w-[min(720px,calc(100vw-3rem))]"
      closeLabel="Cancel"
      primaryAction={
        <>
          <button
            className={`${button} bg-primary-solid text-ink-inverted shadow-sm hover:bg-primary-solid-hover`}
            onClick={() => {
              onClose();
              useDestinationsStore.getState().setActive(path);
              void moveDestinationSelectionTo(path, "move-trash-rest", selection);
            }}
          >
            Move here
          </button>
          <button
            className={`${button} border border-border text-ink hover:border-border-strong hover:bg-surface-muted`}
            onClick={() => {
              onClose();
              useDestinationsStore.getState().setActive(path);
              void moveDestinationSelectionTo(path, "copy", selection);
            }}
          >
            Copy here
          </button>
        </>
      }
    >
      <p className="select-text break-all text-sm text-ink" title={path}>
        {path}
      </p>
      <p className="mt-1 text-xs text-ink-muted">
        Move delivers one copy here and trashes the rest; Copy leaves everything
        in place.
      </p>
    </ModalShell>
  );
}
