import { describe, expect, it } from "vitest";
import {
  CONTENT_MIN_HEIGHT,
  FOOTER_HEIGHT,
  GRID_MIN_WIDTH,
  HEADER_HEIGHT,
  PREVIEW_PANE_DEFAULT_RATIO,
  PREVIEW_PANE_MIN_WIDTH,
  RIGHT_PANE_MIN_WIDTH,
  SIDEBAR_MIN_WIDTH,
  SPLITTER_WIDTH,
  computeMinWindowHeight,
  computeMinWindowWidth,
  derivePaneWidths,
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

  it("the approved first-launch size clears the minimums", async () => {
    const { readFileSync } = await import("node:fs");
    const conf = JSON.parse(readFileSync("src-tauri/tauri.conf.json", "utf8")) as {
      app: { windows: { width: number; height: number }[] };
    };
    const [main] = conf.app.windows;
    expect(main).toMatchObject({ width: 1120, height: 640 });
    expect(main.width).toBeGreaterThanOrEqual(computeMinWindowWidth());
    expect(main.height).toBeGreaterThanOrEqual(computeMinWindowHeight());
  });
});

describe("pane display derives from fixed utility intent and one peer ratio", () => {
  it("passes utility intents through when they fit", () => {
    expect(derivePaneWidths(300, 350, 2000)).toEqual({ left: 300, right: 350, preview: 0 });
  });

  it("shrinks proportionally to headroom when they do not fit", () => {
    // Deliberately ASYMMETRIC. The previous input used equal 400px intents,
    // 400, 1000), where a proportional split and an equal split produce
    // identical numbers — so the word "proportionally" was unverifiable and
    // every assertion was a one-sided inequality that an equal split passes.
    const { left, right } = derivePaneWidths(600, 250, 1000);
    expect(left + right).toBeLessThanOrEqual(1000 - GRID_MIN_WIDTH - 2 * SPLITTER_WIDTH);
    expect(left).toBeGreaterThanOrEqual(SIDEBAR_MIN_WIDTH);
    expect(right).toBeGreaterThanOrEqual(RIGHT_PANE_MIN_WIDTH);
    // The pane with more headroom above its minimum gives up strictly more.
    expect(600 - left).toBeGreaterThan(250 - right);
  });

  it("bottoms out at both minimums and never below", () => {
    const { left, right } = derivePaneWidths(9999, 9999, computeMinWindowWidth());
    expect(left).toBe(SIDEBAR_MIN_WIDTH);
    expect(right).toBe(RIGHT_PANE_MIN_WIDTH);
    // Even in an impossibly narrow container the minimums hold (the window
    // minimum prevents the container from actually going this small).
    const floor = derivePaneWidths(500, 500, 100);
    expect(floor.left).toBe(SIDEBAR_MIN_WIDTH);
    expect(floor.right).toBe(RIGHT_PANE_MIN_WIDTH);
  });

  it("sub-minimum intents display at the minimum", () => {
    expect(derivePaneWidths(10, 10, 2000)).toEqual({
      left: SIDEBAR_MIN_WIDTH,
      right: RIGHT_PANE_MIN_WIDTH,
      preview: 0,
    });
  });

  it("splits the center peer area by ratio when both minimums fit", () => {
    const widths = derivePaneWidths(256, 288, 2000, PREVIEW_PANE_DEFAULT_RATIO);
    const peerWidth = 2000 - 256 - 288 - 3 * SPLITTER_WIDTH;
    expect(widths).toEqual({
      left: 256,
      right: 288,
      preview: Math.round(peerWidth / 2),
    });
  });

  it("clamps a remembered ratio without changing what a wide window restores", () => {
    const ratio = 0.7;
    const tight = derivePaneWidths(400, 400, computeMinWindowWidth(true), ratio);
    expect(tight.left).toBeGreaterThanOrEqual(SIDEBAR_MIN_WIDTH);
    expect(tight.right).toBeGreaterThanOrEqual(RIGHT_PANE_MIN_WIDTH);
    expect(tight.preview).toBeGreaterThanOrEqual(PREVIEW_PANE_MIN_WIDTH);
    const tightGrid =
      computeMinWindowWidth(true) -
      tight.left -
      tight.right -
      tight.preview -
      3 * SPLITTER_WIDTH;
    expect(tightGrid).toBeGreaterThanOrEqual(GRID_MIN_WIDTH);

    const wide = derivePaneWidths(400, 400, 2400, ratio);
    const widePeer = 2400 - wide.left - wide.right - 3 * SPLITTER_WIDTH;
    expect(wide.preview / widePeer).toBeCloseTo(ratio, 3);
  });

  it("uses the equal default for an invalid restored ratio", () => {
    expect(derivePaneWidths(256, 288, 2000, Number.NaN)).toEqual(
      derivePaneWidths(256, 288, 2000, PREVIEW_PANE_DEFAULT_RATIO),
    );
  });

  it("raises the window minimum only while the preview pane is open", () => {
    expect(computeMinWindowWidth(true)).toBe(
      computeMinWindowWidth(false) + PREVIEW_PANE_MIN_WIDTH + SPLITTER_WIDTH,
    );
  });
});
