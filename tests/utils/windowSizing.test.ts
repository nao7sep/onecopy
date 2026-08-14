import { describe, expect, it } from "vitest";
import {
  CONTENT_MIN_HEIGHT,
  FOOTER_HEIGHT,
  GRID_MIN_WIDTH,
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

  it("height is the content plus the footer", () => {
    expect(computeMinWindowHeight()).toBe(CONTENT_MIN_HEIGHT + FOOTER_HEIGHT);
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
    // Wide container: intents display as dragged.
    expect(clampPaneWidths(300, 350, 2000)).toEqual({ left: 300, right: 350 });
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
    });
  });
});
