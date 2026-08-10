import { beforeEach, describe, expect, it } from "vitest";
import {
  hasOpenModal,
  isTopmostModal,
  popModal,
  pushModal,
} from "../../src/utils/modalStack";

// The stack is module-global; drain it between tests.
function drain() {
  // popModal is a no-op for unknown tokens, so popping until empty is safe
  // only by tracking — instead, push/pop symmetrically inside each test.
}

describe("modalStack", () => {
  beforeEach(drain);

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
