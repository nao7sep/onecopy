import { describe, expect, it } from "vitest";
import {
  faceStarRating,
  itemPresentation,
  workPresentationRows,
} from "../../src/models/itemPresentation";
import {
  EMPTY_ITEM_WORK,
  type ItemWorkState,
  type ItemWorkStates,
  type SectionItem,
} from "../../src/models/items";

function state(
  value: ItemWorkState["state"],
  over: Partial<ItemWorkState> = {},
): ItemWorkState {
  return {
    state: value,
    hasValue: false,
    reason: null,
    done: null,
    total: null,
    ...over,
  };
}

function item(over: Partial<SectionItem> = {}): SectionItem {
  return {
    hash: "hash",
    pathId: 1,
    fileName: "photo.jpg",
    resolvedUtcMs: 0,
    copyCount: 1,
    width: 100,
    height: 100,
    hasThumb: true,
    similarGroupId: null,
    sharpness: null,
    faceScore: null,
    byteSize: 100,
    hasCompanions: false,
    durationMs: null,
    dirPaths: ["/photos"],
    derivedWork: EMPTY_ITEM_WORK,
    ...over,
  };
}

function presentation(over: Partial<SectionItem> = {}) {
  return itemPresentation(item(over), {
    similarCount: 0,
    selectionOrdinal: null,
    selectedCount: 0,
    showFaceStars: true,
  });
}

describe("item presentation priority", () => {
  it("keeps failures above ordinary work", () => {
    const work: ItemWorkStates = {
      ...EMPTY_ITEM_WORK,
      preview: state("failed"),
      faces: state("running", { done: 4, total: 10 }),
    };
    expect(presentation({ derivedWork: work }).status).toMatchObject({
      text: "Preview failed",
      tone: "danger",
    });
  });

  it("shows active progress before waiting and visual-output debt", () => {
    const work: ItemWorkStates = {
      ...EMPTY_ITEM_WORK,
      preview: state("pending"),
      snapshots: state("waiting", { reason: "Waiting for ffmpeg" }),
      transcripts: state("running", { done: 42, total: 100 }),
    };
    expect(presentation({ derivedWork: work }).status).toMatchObject({
      text: "Transcript 42%",
      tone: "primary",
    });
  });

  it("does not flood tiles with optional disabled or unavailable analysis", () => {
    const work: ItemWorkStates = {
      ...EMPTY_ITEM_WORK,
      faces: state("disabled", { reason: "Turn on face scoring" }),
      transcripts: state("unavailable", { reason: "Waiting for transcription model" }),
    };
    expect(presentation({ derivedWork: work }).status).toBeNull();
    expect(workPresentationRows(work).map((row) => row.value)).toEqual([
      "Turn on face scoring",
      "Waiting for transcription model",
    ]);
  });
});

describe("item presentation slots", () => {
  it("combines relationships once and labels every symbol", () => {
    const result = itemPresentation(
      item({ copyCount: 3, similarGroupId: 9, hasCompanions: true }),
      {
        similarCount: 4,
        selectionOrdinal: null,
        selectedCount: 0,
        showFaceStars: true,
      },
    );
    expect(result.relationships?.text).toBe("×3 · ≈4 · pair");
    expect(result.relationships?.label).toContain("3 exact copies");
    expect(result.relationships?.label).toContain("4 similar photos");
    expect(result.relationships?.label).toContain("companion files");
  });

  it("uses a check for one selection and ordinals for several", () => {
    expect(
      itemPresentation(item(), {
        similarCount: 0,
        selectionOrdinal: 1,
        selectedCount: 1,
        showFaceStars: true,
      }).selection,
    ).toEqual({ ordinal: null, label: "Selected" });
    expect(
      itemPresentation(item(), {
        similarCount: 0,
        selectionOrdinal: 2,
        selectedCount: 4,
        showFaceStars: true,
      }).selection,
    ).toEqual({ ordinal: 2, label: "Selected 2 of 4" });
  });

  it("shows useful ready analysis while quieting empty results", () => {
    expect(presentation({ faceScore: 0.66 }).analysis?.text).toBe("★★★");
    expect(presentation({ faceScore: 0 }).analysis).toBeNull();
    expect(
      itemPresentation(item({ faceScore: 0.66 }), {
        similarCount: 0,
        selectionOrdinal: null,
        selectedCount: 0,
        showFaceStars: false,
      }).analysis,
    ).toBeNull();
    const transcript = state("ready", { hasValue: true });
    expect(
      presentation({
        derivedWork: { ...EMPTY_ITEM_WORK, transcripts: transcript },
      }).analysis,
    ).toMatchObject({ text: "CC", label: "Transcript available" });
  });
});

describe("face star mapping", () => {
  it("maps the observed score range without treating zero as a rating", () => {
    expect(faceStarRating(null)).toBe(0);
    expect(faceStarRating(0)).toBe(0);
    expect(faceStarRating(0.579)).toBe(1);
    expect(faceStarRating(0.58)).toBe(2);
    expect(faceStarRating(0.649)).toBe(2);
    expect(faceStarRating(0.65)).toBe(3);
  });
});
