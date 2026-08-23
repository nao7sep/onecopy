// @vitest-environment happy-dom
//
// The app's ONE editable-target predicate (keyboard-shortcut-conventions).
// Three inline copies of this test had drifted apart before it existed — one
// of them silently dropping the contenteditable case — which is why the rule
// is one predicate per app rather than a shape each site re-types.

import { describe, expect, it } from "vitest";
import { isEditableTarget } from "../../src/utils/shortcuts";

function el(html: string): HTMLElement {
  const host = document.createElement("div");
  host.innerHTML = html;
  return host.firstElementChild as HTMLElement;
}

describe("what counts as typing", () => {
  it("stands down over the text surfaces the app actually has", () => {
    expect(isEditableTarget(el('<input type="text">'))).toBe(true);
    expect(isEditableTarget(el("<input>"))).toBe(true); // no type = text
    expect(isEditableTarget(el("<textarea></textarea>"))).toBe(true);
    expect(isEditableTarget(el('<input type="number">'))).toBe(true);
  });

  it("does NOT stand down over controls that consume no printable key", () => {
    // A false stand-down is its own defect: Delete over a focused checkbox
    // would stop reaching the command layer.
    expect(isEditableTarget(el('<input type="checkbox">'))).toBe(false);
    expect(isEditableTarget(el('<input type="radio">'))).toBe(false);
    expect(isEditableTarget(el('<input type="range">'))).toBe(false);
    expect(isEditableTarget(el('<input type="button">'))).toBe(false);
    expect(isEditableTarget(el("<select></select>"))).toBe(false);
  });

  it("ignores ordinary chrome, and a target that is not an element at all", () => {
    expect(isEditableTarget(el("<div>grid</div>"))).toBe(false);
    expect(isEditableTarget(el("<button>Save</button>"))).toBe(false);
    expect(isEditableTarget(null)).toBe(false);
  });
});

describe("the walk up parentElement", () => {
  it("sees a contenteditable host", () => {
    expect(isEditableTarget(el('<div contenteditable="true">note</div>'))).toBe(true);
  });

  it("sees a DESCENDANT of one — the case a tagName-only test misses", () => {
    // A rich editor's real event target is a child of the editable host, so
    // a target-only check reads a plain SPAN and lets every chord through
    // while the user is mid-sentence. OneCopy ships no such surface yet;
    // this is what makes the guard true when one arrives.
    const host = el('<div contenteditable="true"><span>word</span></div>');
    const inner = host.querySelector("span")!;
    expect(isEditableTarget(inner)).toBe(true);
  });

  it("does not drag an unrelated ancestor into it", () => {
    const host = el("<div><span>word</span></div>");
    expect(isEditableTarget(host.querySelector("span")!)).toBe(false);
  });
});
