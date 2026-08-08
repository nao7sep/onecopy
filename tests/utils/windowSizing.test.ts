import { describe, expect, it } from "vitest";
import {
  CONTENT_MIN_HEIGHT,
  FOOTER_HEIGHT,
  GRID_MIN_WIDTH,
  RIGHT_PANE_WIDTH,
  SIDEBAR_WIDTH,
  computeMinWindowHeight,
  computeMinWindowWidth,
} from "../../src/utils/windowSizing";

describe("window minimums are derived, never hand-typed", () => {
  it("width is the sum of the content row", () => {
    expect(computeMinWindowWidth()).toBe(
      SIDEBAR_WIDTH + GRID_MIN_WIDTH + RIGHT_PANE_WIDTH,
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
