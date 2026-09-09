// @vitest-environment happy-dom

import { describe, expect, it } from "vitest";
import { viewerOwnsKey } from "../../src/utils/viewerKeys";

function owns(key: string, kind = "image", fileName = "photo.jpg", target: HTMLElement = document.createElement("div"), init: KeyboardEventInit = {}) {
  const event = new KeyboardEvent("keydown", { key, cancelable: true, ...init });
  target.dispatchEvent(event);
  return viewerOwnsKey(event, kind, fileName);
}

describe("transient viewer command ownership", () => {
  it("keeps image Enter neutral and accepts media Enter", () => {
    expect(owns("Enter")).toBe(false);
    expect(owns("Enter", "video", "movie.mp4")).toBe(true);
    expect(owns("Enter", "other", "recording.mp3")).toBe(true);
  });

  it.each(["button", "a", "audio", "video", "select"])("leaves activation and arrows on real %s controls", (tag) => {
    const target = document.createElement(tag);
    target.setAttribute("href", "#");
    target.setAttribute("controls", "");
    expect(owns("Enter", "video", "movie.mp4", target)).toBe(false);
    expect(owns("ArrowRight", "video", "movie.mp4", target)).toBe(false);
    expect(owns(" ", "video", "movie.mp4", target)).toBe(true);
    expect(owns("f", "video", "movie.mp4", target)).toBe(true);
  });

  it("keeps text document scrolling local, but audio sequence bounds viewer-owned", () => {
    for (const key of ["Home", "End", "PageUp", "PageDown"]) {
      expect(owns(key, "other", "readme.txt")).toBe(false);
      expect(owns(key, "other", "recording.wav")).toBe(true);
    }
    expect(owns("ArrowRight", "other", "readme.txt")).toBe(true);
  });

  it("never intercepts typing, composition, modifiers, or an already consumed event", () => {
    expect(owns(" ", "image", "photo.jpg", document.createElement("input"))).toBe(false);
    for (const init of [{ isComposing: true }, { keyCode: 229 }, { metaKey: true }, { ctrlKey: true }, { altKey: true }]) {
      expect(owns("f", "image", "photo.jpg", document.createElement("div"), init)).toBe(false);
    }
    const event = new KeyboardEvent("keydown", { key: "f", cancelable: true });
    event.preventDefault();
    expect(viewerOwnsKey(event, "image", "photo.jpg")).toBe(false);
  });
});
