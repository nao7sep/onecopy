import { useState } from "react";
import {
  useDestinationsStore,
  type DirEntry,
  type MoveMode,
} from "../state/destinations-store";
import { useComposing, isComposingKeyboardEvent } from "../hooks/useComposing";
import ConfirmDialog from "./ConfirmDialog";

// Drop-target behavior shared by roots and nodes: the OS-independent modifier
// mapping is the design's — plain drop = move + trash the rest, Shift = move +
// delete the rest permanently, Cmd/Ctrl = copy and touch nothing.
function dropMode(event: React.DragEvent): MoveMode {
  if (event.metaKey || event.ctrlKey) return "copy";
  if (event.shiftKey) return "move-delete-rest";
  return "move-trash-rest";
}

function useDropHandlers(path: string) {
  const moveSelectionTo = useDestinationsStore((s) => s.moveSelectionTo);
  const [dropReady, setDropReady] = useState(false);
  return {
    dropReady,
    handlers: {
      onDragOver: (event: React.DragEvent) => {
        if (event.dataTransfer.types.includes("application/x-onecopy-drag")) {
          event.preventDefault();
          setDropReady(true);
        }
      },
      onDragLeave: () => setDropReady(false),
      onDrop: (event: React.DragEvent) => {
        event.preventDefault();
        setDropReady(false);
        void moveSelectionTo(path, dropMode(event));
      },
    },
  };
}

// The right pane's destination tree. Empty folders render dimmed-italic (they
// are the only deletable ones); "Move here" trashes the remaining copies,
// Shift-click deletes them permanently, "Copy" touches nothing else.

function NodeActions({
  path,
  parent,
  isEmpty,
  isActive,
}: {
  path: string;
  parent: string | null;
  isEmpty: boolean;
  isActive: boolean;
}) {
  const moveSelectionTo = useDestinationsStore((s) => s.moveSelectionTo);
  const deleteFolder = useDestinationsStore((s) => s.deleteFolder);
  const [creating, setCreating] = useState(false);
  const [name, setName] = useState("");
  const createFolder = useDestinationsStore((s) => s.createFolder);
  const { composingRef, handlers: composingHandlers } = useComposing();

  return (
    <span
      className={`ml-1 shrink-0 gap-1 text-xs ${
        isActive ? "inline-flex" : "hidden group-hover:inline-flex"
      }`}
    >
      <button
        tabIndex={-1}
        className="rounded-md px-1.5 py-0.5 text-primary transition-colors hover:bg-primary-surface"
        title="Move the selected item here; its other copies go to trash. Shift-click: delete them permanently."
        onClick={(e) =>
          void moveSelectionTo(path, e.shiftKey ? "move-delete-rest" : "move-trash-rest")
        }
      >
        Move here
      </button>
      <button
        tabIndex={-1}
        className="rounded-md px-1.5 py-0.5 text-ink transition-colors hover:bg-surface-muted"
        title="Copy the selected item here; nothing else is touched."
        onClick={() => void moveSelectionTo(path, "copy")}
      >
        Copy
      </button>
      {creating ? (
        <input
          autoFocus
          className="h-7 w-28 rounded-md border border-border bg-background px-2 text-ink outline-none focus-visible:ring-2 focus-visible:ring-primary-ring"
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
            const tree = e.currentTarget.closest('[role="tree"]') as HTMLElement | null;
            if (e.key === "Enter" && name.trim() !== "") {
              void createFolder(path, name.trim());
              setCreating(false);
              setName("");
              requestAnimationFrame(() => tree?.focus());
            } else if (e.key === "Escape") {
              setCreating(false);
              setName("");
              requestAnimationFrame(() => tree?.focus());
            }
            e.stopPropagation();
          }}
        />
      ) : (
        <button
          tabIndex={-1}
          className="rounded-md px-1.5 py-0.5 text-ink transition-colors hover:bg-surface-muted"
          title="New subfolder"
          onClick={() => setCreating(true)}
        >
          +
        </button>
      )}
      {isEmpty && parent !== null ? (
        <button
          tabIndex={-1}
          className="rounded-md px-1.5 py-0.5 text-danger transition-colors hover:bg-danger-surface"
          title="Delete this empty folder"
          onClick={() => void deleteFolder(path, parent)}
        >
          ✕
        </button>
      ) : null}
    </span>
  );
}

function DirNode({
  entry,
  parent,
  depth,
}: {
  entry: DirEntry;
  parent: string;
  depth: number;
}) {
  const expanded = useDestinationsStore((s) => s.expanded);
  const children = useDestinationsStore((s) => s.children);
  const emptiness = useDestinationsStore((s) => s.emptiness);
  const toggleExpand = useDestinationsStore((s) => s.toggleExpand);
  const isOpen = expanded.has(entry.path);
  const isEmpty = emptiness[entry.path] === true;
  const activePath = useDestinationsStore((s) => s.activePath);
  const setActive = useDestinationsStore((s) => s.setActive);
  const isActive = activePath === entry.path;
  const { dropReady, handlers } = useDropHandlers(entry.path);

  return (
    <li
      id={`tree-${encodeURIComponent(entry.path)}`}
      role="treeitem"
      aria-selected={isActive}
      aria-expanded={entry.hasChildren ? isOpen : undefined}
    >
      <div
        data-tree-path={entry.path}
        className={`group flex items-center rounded-md px-1.5 py-1 text-sm transition-colors ${
          dropReady || isActive
            ? "bg-primary-surface ring-1 ring-primary-ring"
            : "hover:bg-surface-muted"
        }`}
        style={{ paddingLeft: `${depth * 12}px` }}
        onClick={() => setActive(entry.path)}
        {...handlers}
      >
        <button
          tabIndex={-1}
          className="w-4 shrink-0 text-ink-muted"
          onClick={() => void toggleExpand(entry.path)}
          title={entry.hasChildren ? (isOpen ? "Collapse" : "Expand") : undefined}
        >
          {entry.hasChildren ? (isOpen ? "▾" : "▸") : "·"}
        </button>
        <span
          className={`min-w-0 flex-1 truncate ${
            isEmpty ? "italic text-ink-muted" : "text-ink"
          }`}
          title={entry.path}
        >
          {entry.name}
        </span>
        <NodeActions path={entry.path} parent={parent} isEmpty={isEmpty} isActive={isActive} />
      </div>
      {isOpen ? (
        <ul role="group">
          {(children[entry.path] ?? []).map((child) => (
            <DirNode key={child.path} entry={child} parent={entry.path} depth={depth + 1} />
          ))}
        </ul>
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
        walk(child.path, path, child.hasChildren);
      }
    }
  };
  for (const root of roots) walk(root, null, true);
  return rows;
}

export default function DestinationsTab() {
  const roots = useDestinationsStore((s) => s.roots);
  const expanded = useDestinationsStore((s) => s.expanded);
  const children = useDestinationsStore((s) => s.children);
  const message = useDestinationsStore((s) => s.message);
  const addRoot = useDestinationsStore((s) => s.addRoot);
  const activePath = useDestinationsStore((s) => s.activePath);
  const setActive = useDestinationsStore((s) => s.setActive);
  const toggleExpand = useDestinationsStore((s) => s.toggleExpand);
  const moveSelectionTo = useDestinationsStore((s) => s.moveSelectionTo);

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
      event.preventDefault();
      event.stopPropagation();
      const mode = event.metaKey || event.ctrlKey
        ? "copy"
        : event.shiftKey
          ? "move-delete-rest"
          : "move-trash-rest";
      void moveSelectionTo(row.path, mode);
    }
  };

  const pendingDeleteRest = useDestinationsStore((s) => s.pendingDeleteRest);

  return (
    <div className="flex h-full flex-col p-3">
      {pendingDeleteRest !== null && !pendingDeleteRest.confirmed ? (
        <ConfirmDialog
          title="Move and delete the rest?"
          message={`Move ${pendingDeleteRest.count} item${
            pendingDeleteRest.count === 1 ? "" : "s"
          } here and PERMANENTLY delete the remaining copies? The deleted copies bypass the trash and cannot be recovered.`}
          confirmLabel="Move and delete permanently"
          onConfirm={() => void useDestinationsStore.getState().confirmPendingDeleteRest()}
          onCancel={() => useDestinationsStore.getState().cancelPendingDeleteRest()}
        />
      ) : null}
      <div className="mb-2 flex items-center justify-between">
        <h2 className="text-sm font-semibold text-ink-strong">Destinations</h2>
        <button
          className="inline-flex h-8 items-center gap-1.5 rounded-lg border border-border px-3 text-xs font-medium text-ink transition-colors hover:border-border-strong hover:bg-surface-muted"
          onClick={() => void addRoot()}
        >
          Add root…
        </button>
      </div>
      {/* The container renders (and stays Tab-reachable) even with no
          roots — an empty composite is still a landing place. */}
      <ul
        role="tree"
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
      {message !== "" ? (
        <p className="mt-2 shrink-0 break-words text-xs text-ink-muted">{message}</p>
      ) : null}
    </div>
  );
}

function RootRow({ root, isOpen }: { root: string; isOpen: boolean }) {
  const children = useDestinationsStore((s) => s.children);
  const emptiness = useDestinationsStore((s) => s.emptiness);
  const toggleExpand = useDestinationsStore((s) => s.toggleExpand);
  const removeRoot = useDestinationsStore((s) => s.removeRoot);
  const activePath = useDestinationsStore((s) => s.activePath);
  const setActive = useDestinationsStore((s) => s.setActive);
  const isActive = activePath === root;
  const { dropReady, handlers } = useDropHandlers(root);
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
        className={`group flex items-center rounded-md px-1.5 py-1 text-sm transition-colors ${
          dropReady || isActive
            ? "bg-primary-surface ring-1 ring-primary-ring"
            : "hover:bg-surface-muted"
        }`}
        onClick={() => setActive(root)}
        {...handlers}
      >
                  <button
                    tabIndex={-1}
                    className="w-4 shrink-0 text-ink-muted"
                    onClick={() => void toggleExpand(root)}
                  >
                    {isOpen ? "▾" : "▸"}
                  </button>
                  <span
                    className="min-w-0 flex-1 truncate font-medium text-ink-strong"
                    title={root}
                  >
                    {root}
                  </span>
                  <NodeActions
                    path={root}
                    parent={null}
                    isEmpty={emptiness[root] === true}
                    isActive={isActive}
                  />
                  <button
                    tabIndex={-1}
                    className="ml-1 hidden shrink-0 rounded-md px-1.5 py-0.5 text-xs text-ink-muted transition-colors group-hover:inline hover:bg-surface-muted hover:text-ink"
                    title="Remove this root from the list (the folder itself is untouched)"
                    onClick={() => void removeRoot(root)}
                  >
                    −
                  </button>
                </div>
      {isOpen ? (
        <ul>
          {(children[root] ?? []).map((child) => (
            <DirNode key={child.path} entry={child} parent={root} depth={1} />
          ))}
        </ul>
      ) : null}
    </li>
  );
}
