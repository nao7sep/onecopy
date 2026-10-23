import { describe, expect, it } from "vitest";
import { ViewModeAdmission } from "../../src/state/view-mode-admission";

describe("viewing mode admission", () => {
  it.each(["fullscreen", "comparison"] as const)("owns %s during preparation and native retirement", (mode) => {
    const owner = new ViewModeAdmission();
    const other = mode === "fullscreen" ? "comparison" : "fullscreen";
    const request = owner.begin(mode)!;
    expect(owner.begin(other)).toBeNull();
    expect(request.commit()).toBe(true);
    const settled = owner.retire(mode);
    expect(request.current()).toBe(false);
    expect(owner.begin(other)).toBeNull();
    settled();
    expect(owner.begin(other)).not.toBeNull();
  });

  it("a rejected replacement preserves the admitted session", () => {
    const owner = new ViewModeAdmission();
    const active = owner.begin("comparison")!;
    active.commit();
    const replacement = owner.begin("comparison")!;
    replacement.abandon();
    expect(active.current()).toBe(true);
    expect(owner.begin("fullscreen")).toBeNull();
  });

  it("stale cleanup cannot retire a later same-mode request", () => {
    const owner = new ViewModeAdmission();
    const old = owner.begin("fullscreen")!;
    const settled = owner.retire("fullscreen");
    const next = owner.begin("fullscreen")!;
    old.abandon();
    settled();
    expect(old.commit()).toBe(false);
    expect(next.commit()).toBe(true);
    expect(owner.begin("comparison")).toBeNull();
  });

  it("retains admission until every overlapping retirement settles", () => {
    const owner = new ViewModeAdmission();
    owner.begin("comparison")!.commit();
    const first = owner.retire("comparison");
    const second = owner.retire("comparison");
    first();
    expect(owner.begin("fullscreen")).toBeNull();
    second();
    expect(owner.begin("fullscreen")).not.toBeNull();
  });

  it("entry failure preserves a replacement already preparing", () => {
    const owner = new ViewModeAdmission();
    const active = owner.begin("comparison")!;
    active.commit();
    const replacement = owner.begin("comparison")!;
    const settled = owner.retire("comparison", true);
    expect(active.current()).toBe(false);
    expect(replacement.current()).toBe(true);
    settled();
    expect(owner.begin("fullscreen")).toBeNull();
    expect(replacement.commit()).toBe(true);
  });

  it("closing the other mode does not interrupt preparation", () => {
    const owner = new ViewModeAdmission();
    const pending = owner.begin("comparison")!;
    owner.retire("fullscreen")();
    expect(pending.current()).toBe(true);
    expect(pending.commit()).toBe(true);
  });
});
