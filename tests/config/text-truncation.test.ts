import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

// Display truncation is a reviewed exception (text-cleanup-conventions rule 4):
// a shortened value shows one marker and has a keyboard and touch path to its
// whole, never a hover title alone (interface-styling-conventions, "Labels"),
// so each site is listed here with the path that serves it. A new site, or one
// removed, fails until this inventory is reviewed again; a clip with no marker
// fails outright.

const REVIEWED_SITES: Record<string, number> = {
  // Row name and facts lines; the active row's detail line shows them whole.
  "src/components/DeletedFilesModal.tsx": 4,
  // The drag image of the current selection, whose names Details shows.
  "src/components/DestinationDragPreview.tsx": 2,
  // Tree rows, root names and the action bar's leaf name; the action bar's
  // path line opens the active folder's whole path.
  "src/components/DestinationsTab.tsx": 3,
  // Tiles, list cells, badges and column headings; the selection's Details
  // pane shows each value, and a heading is cut only by a pointer resize.
  "src/components/Grid.tsx": 9,
  // Command labels, under a cap every command fits in the longest language:
  // only a viewport narrower than the cap cuts one.
  "src/components/Menu.tsx": 1,
  // Dialog titles; a title cut by the name it carries names something its
  // body shows whole (a copy's paths, a drop's destination).
  "src/components/ModalShell.tsx": 1,
  // A notice's sentence and recorded detail, summarised on the toast; Issues
  // and Records, where every notice is recorded, show them whole.
  "src/components/NotificationHost.tsx": 2,
  // The closed native select, whose open list shows every option whole.
  "src/components/ui/Field.tsx": 1,
  // The shared one-line identifying text: a cut value opens whole on Enter,
  // a click or a tap, unless its site is listed under DETAIL_SURFACE_SITES.
  "src/components/ui/IdentifyingText.tsx": 3,
  // Record titles and summaries; the detail pane shows the selected record.
  "src/windows/RecordsWindow.tsx": 2,
};

// Sites that turn IdentifyingText's own reveal off, because the selection's
// detail surface already shows the value whole.
const DETAIL_SURFACE_SITES: Record<string, number> = {
  // Folder lines of rows; the active row's detail line shows them whole.
  "src/components/DeletedFilesModal.tsx": 2,
  // Root rows' paths; the action bar opens the active row's whole path.
  "src/components/DestinationsTab.tsx": 1,
  // The list's folders cell; the Details pane lists every copy whole.
  "src/components/Grid.tsx": 1,
  // A notice's path; Issues and Records show every notice whole.
  "src/components/NotificationHost.tsx": 1,
};

// The working tree, not the index: a new file is reviewed before it is
// committed, not after.
function sourceFiles(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) return sourceFiles(path);
    return /\.(tsx?|css)$/.test(entry.name) ? [path] : [];
  });
}

const sources = sourceFiles("src");

function countsOf(pattern: RegExp): Record<string, number> {
  const sites: Record<string, number> = {};
  for (const path of sources) {
    const count = readFileSync(path, "utf8").match(pattern)?.length ?? 0;
    if (count > 0) sites[path] = count;
  }
  return sites;
}

describe("text truncation", () => {
  it("cuts text only at the reviewed sites", () => {
    expect(countsOf(/\btruncate\b|\btext-ellipsis\b|text-overflow|\bline-clamp-\d/g)).toEqual(REVIEWED_SITES);
  });

  it("turns the shared reveal off only where a detail surface shows the value", () => {
    expect(countsOf(/<IdentifyingText\b[^>]*\breveal=\{false\}/g)).toEqual(DETAIL_SURFACE_SITES);
  });

  it("never clips text without a marker", () => {
    const clips: string[] = [];
    for (const path of sources) {
      const text = readFileSync(path, "utf8");
      const units = path.endsWith(".css") ? text.split("}") : text.split("\n");
      units.forEach((unit, index) => {
        const nowrap = /whitespace-nowrap|white-space:\s*nowrap/.test(unit);
        const hidden = /overflow(-x)?-(hidden|clip)\b|overflow(-x)?:\s*(hidden|clip)/.test(unit);
        const marked = /\btruncate\b|\btext-ellipsis\b|text-overflow:\s*ellipsis/.test(unit);
        if (/\btext-clip\b|text-overflow:\s*clip/.test(unit) || (nowrap && hidden && !marked)) {
          clips.push(`${path}:${index + 1}`);
        }
      });
    }
    expect(clips).toEqual([]);
  });
});
