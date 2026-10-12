import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

// Display truncation is a reviewed exception (text-cleanup-conventions rule 4):
// a shortened value shows one marker and has a keyboard and pointer path to
// its whole, so each site is listed here with the path that serves it. A new
// site, or one removed, fails until this inventory is reviewed again; a clip
// with no marker fails outright.

const REVIEWED_SITES: Record<string, number> = {
  // Row name and folder lines; the active row's detail line shows them whole.
  "src/components/DeletedFilesModal.tsx": 5,
  // The drag image of the current selection, whose names Details shows.
  "src/components/DestinationDragPreview.tsx": 2,
  // Tree rows, root lines and the header's leaf name; the header's wrapping
  // path line shows the selected folder whole.
  "src/components/DestinationsTab.tsx": 4,
  // Tiles, list cells, badges and column headings; the selection's Details
  // pane shows each value, and a heading is cut only by a pointer resize.
  "src/components/Grid.tsx": 10,
  // The closed native select, whose open list shows every option whole.
  "src/components/ui/Field.tsx": 1,
  // Record titles and summaries; the detail pane shows the selected record.
  "src/windows/RecordsWindow.tsx": 2,
};

const sources = execFileSync("git", ["ls-files", "src"], { encoding: "utf8" })
  .split("\n")
  .filter((path) => /\.(tsx?|css)$/.test(path));

describe("text truncation", () => {
  it("cuts text only at the reviewed sites", () => {
    const sites: Record<string, number> = {};
    for (const path of sources) {
      const count = readFileSync(path, "utf8").match(/\btruncate\b|\btext-ellipsis\b|text-overflow/g)?.length ?? 0;
      if (count > 0) sites[path] = count;
    }
    expect(sites).toEqual(REVIEWED_SITES);
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
