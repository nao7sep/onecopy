import { describe, expect, it } from "vitest";
import {
  CONTENT_MIN_HEIGHT,
  FOOTER_HEIGHT,
  GRID_MIN_WIDTH,
  HEADER_HEIGHT,
  PREVIEW_PANE_MIN_WIDTH,
  RIGHT_PANE_MIN_WIDTH,
  SIDEBAR_MIN_WIDTH,
  SPLITTER_WIDTH,
  clampPaneWidths,
  computeMinWindowHeight,
  computeMinWindowWidth,
} from "../../src/utils/windowSizing";

describe("window minimums are derived, never hand-typed", () => {
  it("width is the sum of the content row at its minimums", () => {
    expect(computeMinWindowWidth()).toBe(
      SIDEBAR_MIN_WIDTH + SPLITTER_WIDTH + GRID_MIN_WIDTH + SPLITTER_WIDTH + RIGHT_PANE_MIN_WIDTH,
    );
  });

  it("height reserves the footer, and the title section does not tax it", () => {
    expect(computeMinWindowHeight()).toBe(CONTENT_MIN_HEIGHT + FOOTER_HEIGHT);
    // The title section lives INSIDE the sidebar column, so it must not be
    // added on top — that is the whole point of not making it a full-width
    // band. It still has to fit within the content row it shares.
    expect(HEADER_HEIGHT).toBeLessThan(CONTENT_MIN_HEIGHT);
  });

  it("the launch size in tauri.conf.json clears the minimums", async () => {
    const { readFileSync } = await import("node:fs");
    const conf = JSON.parse(readFileSync("src-tauri/tauri.conf.json", "utf8")) as {
      app: { windows: { width: number; height: number }[] };
    };
    const [main] = conf.app.windows;
    expect(main.width).toBeGreaterThanOrEqual(computeMinWindowWidth());
    expect(main.height).toBeGreaterThanOrEqual(computeMinWindowHeight());
  });
});

describe("pane clamping derives display from intent", () => {
  it("passes both intents through when they fit", () => {
    // Wide container: intents display as dragged (preview closed → 0).
    expect(clampPaneWidths(300, 350, 2000)).toEqual({ left: 300, right: 350, preview: 0 });
  });

  it("shrinks proportionally to headroom when they do not fit", () => {
    // Deliberately ASYMMETRIC. The previous input was clampPaneWidths(400,
    // 400, 1000), where a proportional split and an equal split produce
    // identical numbers — so the word "proportionally" was unverifiable and
    // every assertion was a one-sided inequality that an equal split passes.
    const { left, right } = clampPaneWidths(600, 250, 1000);
    expect(left + right).toBeLessThanOrEqual(1000 - GRID_MIN_WIDTH - 2 * SPLITTER_WIDTH);
    expect(left).toBeGreaterThanOrEqual(SIDEBAR_MIN_WIDTH);
    expect(right).toBeGreaterThanOrEqual(RIGHT_PANE_MIN_WIDTH);
    // The pane with more headroom above its minimum gives up strictly more.
    expect(600 - left).toBeGreaterThan(250 - right);
  });

  it("bottoms out at both minimums and never below", () => {
    const { left, right } = clampPaneWidths(9999, 9999, computeMinWindowWidth());
    expect(left).toBe(SIDEBAR_MIN_WIDTH);
    expect(right).toBe(RIGHT_PANE_MIN_WIDTH);
    // Even in an impossibly narrow container the minimums hold (the window
    // minimum prevents the container from actually going this small).
    const floor = clampPaneWidths(500, 500, 100);
    expect(floor.left).toBe(SIDEBAR_MIN_WIDTH);
    expect(floor.right).toBe(RIGHT_PANE_MIN_WIDTH);
  });

  it("sub-minimum intents display at the minimum", () => {
    expect(clampPaneWidths(10, 10, 2000)).toEqual({
      left: SIDEBAR_MIN_WIDTH,
      right: RIGHT_PANE_MIN_WIDTH,
      preview: 0,
    });
  });

  it("clamps the preview as a third pane when it is open", () => {
    // Everything fits: intents pass through.
    expect(clampPaneWidths(256, 288, 2000, 480)).toEqual({
      left: 256,
      right: 288,
      preview: 480,
    });
    // Too narrow: every pane shrinks toward its own minimum, the grid keeps
    // its fill minimum, and nothing goes below its floor.
    const tight = clampPaneWidths(400, 400, 1400, 600);
    expect(tight.left).toBeGreaterThanOrEqual(SIDEBAR_MIN_WIDTH);
    expect(tight.right).toBeGreaterThanOrEqual(RIGHT_PANE_MIN_WIDTH);
    expect(tight.preview).toBeGreaterThanOrEqual(PREVIEW_PANE_MIN_WIDTH);
    expect(tight.left + tight.right + tight.preview).toBeLessThanOrEqual(
      1400 - GRID_MIN_WIDTH - 3 * SPLITTER_WIDTH,
    );
    // The pane with the most headroom gives up the most.
    expect(600 - tight.preview).toBeGreaterThanOrEqual(400 - tight.left);
  });

  it("raises the window minimum only while the preview pane is open", () => {
    expect(computeMinWindowWidth(true)).toBe(
      computeMinWindowWidth(false) + PREVIEW_PANE_MIN_WIDTH + SPLITTER_WIDTH,
    );
  });
});
