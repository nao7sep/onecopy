// Small pure helpers with real invariants and no coverage.
//
// The zoom steppers snap through nearest() before moving, so an off-ladder
// value does not step from where it looks like it is. The URL builders are
// one half of a round trip whose other half is a Rust parse — a change to
// either that is not matched in the other shows up as a missing image, not
// as a type error.

import { describe, expect, it } from "vitest";
import {
  ZOOM_LEVELS,
  ZOOM_MAX,
  ZOOM_MIN,
  stepZoomIn,
  stepZoomOut,
} from "../../src/utils/zoom";
import {
  extLabel,
  formatDuration,
  originalUrl,
  previewUrl,
  stripUrl,
  thumbUrl,
} from "../../src/models/items";

describe("the zoom ladder", () => {
  it("steps from the NEAREST level, not from the raw value", () => {
    // 1.05 is not on the ladder. It snaps to 1.0 first, so stepping up lands
    // on 1.2 rather than somewhere just above 1.05.
    expect(stepZoomIn(1.05)).toBe(1.2);
    expect(stepZoomOut(1.05)).toBe(0.9);
  });

  it("moves one rung at a time from an on-ladder value", () => {
    expect(stepZoomIn(1.0)).toBe(1.2);
    expect(stepZoomOut(1.0)).toBe(0.9);
  });

  it("clamps at both ends instead of running off the ladder", () => {
    expect(stepZoomIn(ZOOM_MAX)).toBe(ZOOM_MAX);
    expect(stepZoomOut(ZOOM_MIN)).toBe(ZOOM_MIN);
    expect(stepZoomIn(999)).toBe(ZOOM_MAX);
    expect(stepZoomOut(0.01)).toBe(ZOOM_MIN);
  });

  it("keeps the ladder ascending and containing 100%", () => {
    const ascending = [...ZOOM_LEVELS].sort((a, b) => a - b);
    expect(ZOOM_LEVELS).toEqual(ascending);
    expect(ZOOM_LEVELS).toContain(1.0);
  });
});

describe("duration formatting", () => {
  it("reads m:ss under an hour and h:mm:ss above it", () => {
    expect(formatDuration(5000)).toBe("0:05");
    expect(formatDuration(65000)).toBe("1:05");
    expect(formatDuration(3725000)).toBe("1:02:05");
  });

  it("pads seconds and minutes so badges do not jump width", () => {
    expect(formatDuration(9000)).toBe("0:09");
    expect(formatDuration(3600000)).toBe("1:00:00");
  });
});

describe("cache and file URLs", () => {
  it("builds the strip key the Rust handler parses back", () => {
    // lib.rs splits on the LAST '-' to recover the index, so the shape here
    // and the parse there are one contract. A hash containing '-' would break
    // it, which is why the index must be the final segment.
    const url = stripUrl("abc123", 2);
    expect(url).toContain("strip-abc123-2");
    expect(url.split("/").pop()!.split("-").pop()).toBe("2");
  });

  it("prefixes thumb and preview keys distinctly", () => {
    expect(thumbUrl("abc123")).toContain("thumb-abc123");
    expect(previewUrl("abc123")).toContain("preview-abc123");
    expect(thumbUrl("abc123")).not.toBe(previewUrl("abc123"));
  });

  it("serves originals by bare hash on the mediafile protocol", () => {
    const url = originalUrl("abc123");
    expect(url).toContain("abc123");
    expect(url).toContain("mediafile");
    // No cache prefix: the original is keyed by the hash alone.
    expect(url).not.toContain("thumb-");
    expect(url).not.toContain("preview-");
  });
});

describe("the placeholder label", () => {
  it("uppercases the extension", () => {
    expect(extLabel("holiday.heic")).toBe("HEIC");
    expect(extLabel("A.tar.gz")).toBe("GZ");
  });

  it("falls back for names with no usable extension", () => {
    expect(extLabel("README")).toBe("FILE");
    expect(extLabel(".gitignore")).toBe("FILE");
    expect(extLabel("trailing.")).toBe("FILE");
  });
});
