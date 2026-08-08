import { useState } from "react";
import {
  useDestinationsStore,
  type DirEntry,
  type MoveMode,
} from "../state/destinations-store";

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
}: {
  path: string;
  parent: string | null;
  isEmpty: boolean;
}) {
  const moveSelectionTo = useDestinationsStore((s) => s.moveSelectionTo);
  const deleteFolder = useDestinationsStore((s) => s.deleteFolder);
  const [creating, setCreating] = useState(false);
  const [name, setName] = useState("");
  const createFolder = useDestinationsStore((s) => s.createFolder);

  return (
    <span className="ml-1 hidden shrink-0 gap-1 text-xs group-hover:inline-flex">
      <button
        className="rounded border border-border px-1 text-primary hover:bg-primary-surface"
        title="Move the selected item here; its other copies go to trash. Shift-click: delete them permanently."
        onClick={(e) =>
          void moveSelectionTo(path, e.shiftKey ? "move-delete-rest" : "move-trash-rest")
        }
      >
        Move here
      </button>
      <button
        className="rounded border border-border px-1 text-ink hover:bg-surface-muted"
        title="Copy the selected item here; nothing else is touched."
        onClick={() => void moveSelectionTo(path, "copy")}
      >
        Copy
      </button>
      {creating ? (
        <input
          autoFocus
          className="w-24 rounded border border-border bg-background px-1 text-ink"
          value={name}
          placeholder="folder name"
          onChange={(e) => setName(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && name.trim() !== "") {
              void createFolder(path, name.trim());
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
          className="rounded border border-border px-1 text-ink hover:bg-surface-muted"
          title="New subfolder"
          onClick={() => setCreating(true)}
        >
          +
        </button>
      )}
      {isEmpty && parent !== null ? (
        <button
          className="rounded border border-border px-1 text-danger hover:bg-danger-surface"
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
  const { dropReady, handlers } = useDropHandlers(entry.path);

  return (
    <li>
      <div
        className={`group flex items-center rounded px-1 py-0.5 text-sm ${
          dropReady ? "bg-primary-surface ring-1 ring-primary-ring" : "hover:bg-surface-muted"
        }`}
        style={{ paddingLeft: `${depth * 12}px` }}
        {...handlers}
      >
        <button
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
        <NodeActions path={entry.path} parent={parent} isEmpty={isEmpty} />
      </div>
      {isOpen ? (
        <ul>
          {(children[entry.path] ?? []).map((child) => (
            <DirNode key={child.path} entry={child} parent={entry.path} depth={depth + 1} />
          ))}
        </ul>
      ) : null}
    </li>
  );
}

export default function DestinationsTab() {
  const roots = useDestinationsStore((s) => s.roots);
  const expanded = useDestinationsStore((s) => s.expanded);
  const message = useDestinationsStore((s) => s.message);
  const addRoot = useDestinationsStore((s) => s.addRoot);

  return (
    <div className="flex h-full flex-col p-3">
      <div className="mb-2 flex items-center justify-between">
        <h2 className="text-sm font-semibold text-ink-strong">Destinations</h2>
        <button
          className="rounded border border-border px-2 py-0.5 text-xs text-primary hover:bg-primary-surface"
          onClick={() => void addRoot()}
        >
          Add root…
        </button>
      </div>
      {roots.length === 0 ? (
        <p className="text-sm text-ink-muted">
          Add a destination root — the place cleaned-up files move to. It must
          lie outside every scanned directory.
        </p>
      ) : (
        <ul className="min-h-0 flex-1 overflow-y-auto">
          {roots.map((root) => {
            const isOpen = expanded.has(root);
            return (
              <RootRow key={root} root={root} isOpen={isOpen} />
            );
          })}
        </ul>
      )}
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
  const { dropReady, handlers } = useDropHandlers(root);
  return (
    <li className="mb-1">
      <div
        className={`group flex items-center rounded px-1 py-0.5 text-sm ${
          dropReady ? "bg-primary-surface ring-1 ring-primary-ring" : "hover:bg-surface-muted"
        }`}
        {...handlers}
      >
                  <button
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
                  />
                  <button
                    className="ml-1 hidden shrink-0 rounded border border-border px-1 text-xs text-ink-muted group-hover:inline hover:bg-surface-muted"
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
