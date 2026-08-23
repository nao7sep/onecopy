import { beforeEach, describe, expect, it } from "vitest";
import {
  hasOpenModal,
  isTopmostModal,
  popModal,
  pushModal,
  resetModalStack,
} from "../../src/utils/modalStack";

// The stack is module-global. The previous `drain()` was a body of pure
// comments registered as beforeEach, so the isolation it implied did not
// exist — the specs passed only because they happened to pop symmetrically.
// File-level, so EVERY describe in this file starts from an empty stack.
beforeEach(resetModalStack);

describe("modalStack", () => {

  it("topmost is the most recently pushed still-open token", () => {
    const settings = {};
    const confirm = {};
    pushModal(settings);
    expect(isTopmostModal(settings)).toBe(true);

    pushModal(confirm);
    expect(isTopmostModal(settings)).toBe(false);
    expect(isTopmostModal(confirm)).toBe(true);

    // Stacked surfaces unwind one at a time.
    popModal(confirm);
    expect(isTopmostModal(settings)).toBe(true);
    popModal(settings);
    expect(isTopmostModal(settings)).toBe(false);
  });

  it("hasOpenModal reflects any registered surface, in any close order", () => {
    expect(hasOpenModal()).toBe(false);
    const a = {};
    const b = {};
    pushModal(a);
    pushModal(b);
    // Closing the lower surface first must not confuse the top.
    popModal(a);
    expect(hasOpenModal()).toBe(true);
    expect(isTopmostModal(b)).toBe(true);
    popModal(b);
    expect(hasOpenModal()).toBe(false);
  });
});

describe("isolation", () => {
  it("does not leak an unpopped token into the next spec", () => {
    pushModal({});
    expect(hasOpenModal()).toBe(true);
  });

  it("starts clean even though the previous spec never popped", () => {
    // This is the assertion the comment-only drain() could not make good on.
    expect(hasOpenModal()).toBe(false);
  });
});
