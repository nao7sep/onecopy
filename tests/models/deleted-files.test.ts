import { describe, expect, it } from "vitest";
import {
  browseRows,
  bucketByDay,
  entryFolder,
  entryName,
  filterGroups,
  groupEntries,
  groupSelected,
  localDay,
  reconcileSelection,
  restoreReceipt,
  reviewAction,
  toggleEntry,
  toggleGroup,
  type TrashEntry,
} from "../../src/models/deletedFiles";
import { mutationProgressLine, mutationResultLine } from "../../src/models/mutation";
import { createTranslator } from "../../src/i18n/translate";

function entry(overrides: Partial<TrashEntry> & { id: string }): TrashEntry {
  const storedName = overrides.id.split("/").pop() ?? overrides.id;
  return {
    day: "20260927-utc",
    storedName,
    storedPath: `/root/.onecopy-trash/${overrides.id}`,
    originalRelative: `photos/${storedName}`,
    deletedAtUtc: "2026-09-27T10:00:00.000Z",
    size: 10,
    mtimeMs: 1,
    group: `item:${overrides.operation ?? "op-1"}:${overrides.item ?? "hash-1"}`,
    version: 2,
    kind: "delete",
    operation: "op-1",
    item: "hash-1",
    role: "main",
    movedTo: null,
    status: "restorable",
    mainRestoredAs: null,
    ...overrides,
  };
}

describe("grouping", () => {
  it("groups version 2 records by operation and item, mains before companions", () => {
    const groups = groupEntries([
      entry({ id: "d/x.arw", role: "companion", originalRelative: "a/x.arw" }),
      entry({ id: "d/x.jpg", originalRelative: "a/x.jpg" }),
      entry({ id: "d/x-2.jpg", originalRelative: "b/x.jpg" }),
      entry({ id: "d/y.jpg", item: "hash-2", originalRelative: "a/y.jpg" }),
      entry({ id: "d/x-3.jpg", operation: "op-2", originalRelative: "a/x.jpg" }),
    ]);
    expect(groups).toHaveLength(3);
    const first = groups.find((group) => group.entries.length === 3)!;
    expect(first.entries.map((member) => member.id)).toEqual(["d/x.jpg", "d/x-2.jpg", "d/x.arw"]);
    expect([first.mains, first.companions]).toEqual([2, 1]);
    expect(first.size).toBe(30);
    expect(entryName(first.representative)).toBe("x.jpg");
  });

  it("groups older records by the backend's day, folder and stem key", () => {
    const legacy = { version: 1, operation: null, item: null, kind: null, status: "unverified" as const };
    const groups = groupEntries([
      entry({ ...legacy, id: "d/IMG.JPG", originalRelative: "a/IMG.JPG", group: "day:d:a/img" }),
      entry({ ...legacy, id: "d/img.xmp", role: "companion", originalRelative: "a/img.xmp", group: "day:d:a/img" }),
      entry({ ...legacy, id: "d/IMG-2.JPG", originalRelative: "b/IMG.JPG", group: "day:d:b/img" }),
    ]);
    expect(groups.map((group) => group.entries.length).sort()).toEqual([1, 2]);
  });

  it("groups a displaced destination family by operation, folder and stem", () => {
    const displaced = { kind: "overwrite-displaced" as const, item: null, group: "stem:op-1:out/x" };
    const groups = groupEntries([
      entry({ ...displaced, id: "d/x.jpg", originalRelative: "out/x.jpg" }),
      entry({ ...displaced, id: "d/x.xmp", role: "companion", originalRelative: "out/x.xmp" }),
    ]);
    expect(groups).toHaveLength(1);
    expect(groups[0].kind).toBe("overwrite-displaced");
  });
});

describe("local days", () => {
  it("buckets by the local day of deletion, not the UTC day folder", () => {
    // 23:30 UTC on the 26th is already the 27th in Tokyo.
    expect(localDay("2026-09-26T23:30:00.000Z", "Asia/Tokyo")).toBe("2026-09-27");
    expect(localDay("2026-09-26T23:30:00.000Z", "UTC")).toBe("2026-09-26");
    const groups = groupEntries([
      entry({ id: "20260926-utc/late.jpg", item: "a", deletedAtUtc: "2026-09-26T23:30:00.000Z" }),
      entry({ id: "20260927-utc/early.jpg", item: "b", deletedAtUtc: "2026-09-27T01:00:00.000Z" }),
      entry({ id: "20260925-utc/old.jpg", item: "c", deletedAtUtc: "2026-09-25T12:00:00.000Z" }),
    ]);
    const tokyo = bucketByDay(groups, "Asia/Tokyo");
    expect(tokyo.map((bucket) => [bucket.day, bucket.groups.length])).toEqual([
      ["2026-09-27", 2],
      ["2026-09-25", 1],
    ]);
    expect(entryName(tokyo[0].groups[0].representative)).toBe("early.jpg");
    expect(bucketByDay(groups, "UTC").map((bucket) => bucket.day)).toEqual([
      "2026-09-27",
      "2026-09-26",
      "2026-09-25",
    ]);
  });
});

describe("search", () => {
  const groups = groupEntries([
    entry({ id: "d/Café.jpg", item: "a", originalRelative: "Trips/Paris/Café.jpg" }),
    entry({ id: "d/other.jpg", item: "b", originalRelative: "Home/other.jpg" }),
  ]);

  it("matches name or original folder, case- and normalization-insensitively", () => {
    expect(filterGroups(groups, "CAFÉ")).toHaveLength(1);
    expect(filterGroups(groups, "café")).toHaveLength(1);
    expect(filterGroups(groups, "trips/paris")).toHaveLength(1);
    expect(filterGroups(groups, "home")).toHaveLength(1);
    expect(filterGroups(groups, "  ")).toHaveLength(2);
    expect(filterGroups(groups, "nowhere")).toHaveLength(0);
  });

  it("names the top level of the root as an empty folder", () => {
    expect(entryFolder(entry({ id: "d/top.jpg", originalRelative: "top.jpg" }))).toBe("");
  });
});

describe("selection", () => {
  const group = groupEntries([
    entry({ id: "d/x.jpg" }),
    entry({ id: "d/x.xmp", role: "companion" }),
    entry({ id: "d/x-2.jpg", status: "changed" }),
  ])[0];

  it("checking a group selects every restorable file, and again clears them", () => {
    const all = toggleGroup(group, new Set());
    expect([...all].sort()).toEqual(["d/x.jpg", "d/x.xmp"]);
    expect(groupSelected(group, all)).toBe(true);
    expect(toggleGroup(group, all).size).toBe(0);
  });

  it("files can be deselected one by one, and unrestorable ones never select", () => {
    const all = toggleGroup(group, new Set());
    const withoutCompanion = toggleEntry(group.entries.find((member) => member.id === "d/x.xmp")!, all);
    expect([...withoutCompanion]).toEqual(["d/x.jpg"]);
    expect(groupSelected(group, withoutCompanion)).toBe(false);
    const changed = group.entries.find((member) => member.id === "d/x-2.jpg")!;
    expect(toggleEntry(changed, new Set()).size).toBe(0);
  });

  it("a fresh listing drops selected files it no longer offers", () => {
    const next = reconcileSelection(new Set(["d/x.jpg", "d/gone.jpg"]), group.entries);
    expect([...next]).toEqual(["d/x.jpg"]);
  });

  it("rows list day headers, groups and the files of expanded groups", () => {
    const buckets = bucketByDay([group], "UTC");
    expect(browseRows(buckets, new Set()).map((row) => row.type)).toEqual(["day", "group"]);
    expect(browseRows(buckets, new Set([group.key])).map((row) => row.type)).toEqual([
      "day",
      "group",
      "entry",
      "entry",
      "entry",
    ]);
  });
});

describe("the restore receipt", () => {
  const base = {
    cancelled: false,
    error: null,
    restored: ["/r/a.jpg", "/r/b.jpg"],
    alreadyPresent: 1,
    failed: 2,
    unknown: 1,
    unstarted: 3,
    filesTotal: 8,
    planToken: null,
    requiresReview: false,
    planChanged: false,
    review: null,
  };

  it("counts restored, already there, failed, unknown and unstarted files apart", () => {
    const line = mutationResultLine(restoreReceipt(base), createTranslator("en").t);
    expect(line).toBe(
      "Restore finished with failures — 2 restored · 1 already at its original location · 1 failed · 1 outcome unknown · 3 file steps unstarted",
    );
    const clean = mutationResultLine(
      restoreReceipt({ ...base, alreadyPresent: 0, failed: 0, unknown: 0, unstarted: 0 }),
      createTranslator("en").t,
    );
    expect(clean).toBe("Restore complete — 2 restored");
  });

  it("shows restoring progress in files, never bytes", () => {
    const line = mutationProgressLine(
      {
        operationId: 1,
        kind: "restore",
        phase: "restoring",
        itemsDone: 1,
        itemsTotal: 3,
        filesDone: 1,
        filesTotal: 3,
        bytesDone: 0,
        bytesTotal: 0,
        failures: 0,
        currentFileBytesDone: null,
        currentFileBytesTotal: null,
        nextPhase: "complete",
      },
      false,
      createTranslator("en").t,
      createTranslator("en").number,
    );
    expect(line).toBe("Restoring — 1/3 files");
  });

  it("offers Rename and Restore only when a file is renamed, and nothing when all are skipped", () => {
    const file = { id: "a", original: "a.jpg", target: "a.jpg", renamed: false, unverified: true, skip: null, mainRestoredAs: null };
    expect(reviewAction({ files: [file], folders: [], companionsLeft: [] })).toBe("restore");
    expect(reviewAction({ files: [{ ...file, renamed: true }], folders: [], companionsLeft: [] })).toBe(
      "rename-and-restore",
    );
    expect(
      reviewAction({ files: [{ ...file, target: null, skip: "changed" as const }], folders: [], companionsLeft: [] }),
    ).toBe("none");
  });
});
