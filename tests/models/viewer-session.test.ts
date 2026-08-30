import { describe, expect, it } from "vitest";
import {
  createViewerSession,
  moveViewerSession,
  reconcileViewerSession,
  viewerCurrentKey,
} from "../../src/models/viewerSession";

describe("viewer session", () => {
  const members = (keys: string[]) => keys.map((key, index) => ({ key, pathId: index + 1 }));

  it("freezes the whole displayed section for one selected item", () => {
    const session = createViewerSession(
      "quick",
      members(["a", "b", "c"]),
      new Set(["b"]),
      "b",
    );
    expect(session).toEqual({
      presentation: "quick",
      members: members(["a", "b", "c"]),
      index: 1,
      scope: "section",
    });
  });

  it("freezes a multi-selection in displayed order at its anchor", () => {
    const session = createViewerSession(
      "fullscreen",
      members(["a", "b", "c", "d"]),
      new Set(["d", "b"]),
      "d",
    );
    expect(session).toEqual({
      presentation: "fullscreen",
      members: [members(["a", "b", "c", "d"])[1], members(["a", "b", "c", "d"])[3]],
      index: 1,
      scope: "selection",
    });
  });

  it("clamps navigation at both ends", () => {
    const session = createViewerSession("quick", members(["a", "b"]), new Set(["a"]), "a")!;
    expect(viewerCurrentKey(moveViewerSession(session, "previous"))).toBe("a");
    expect(viewerCurrentKey(moveViewerSession(session, "last"))).toBe("b");
    expect(viewerCurrentKey(moveViewerSession(session, "next"))).toBe("b");
  });

  it("keeps a surviving current item when earlier members disappear", () => {
    const session = createViewerSession(
      "quick",
      members(["a", "b", "c"]),
      new Set(["b"]),
      "b",
    )!;
    const reconciled = reconcileViewerSession(session, members(["b", "c"]).map((member, index) => ({ ...member, pathId: index + 2 })))!;
    expect(reconciled.members.map((member) => member.key)).toEqual(["b", "c"]);
    expect(viewerCurrentKey(reconciled)).toBe("b");
  });

  it("chooses next, previous, then closes when members disappear", () => {
    const session = createViewerSession(
      "quick",
      members(["a", "b", "c"]),
      new Set(["b"]),
      "b",
    )!;
    const next = reconcileViewerSession(session, [members(["a", "b", "c"])[0], members(["a", "b", "c"])[2]])!;
    expect(viewerCurrentKey(next)).toBe("c");
    const previous = reconcileViewerSession(session, [members(["a"])[0]])!;
    expect(viewerCurrentKey(previous)).toBe("a");
    expect(reconcileViewerSession(session, [])).toBeNull();
  });

  it("keeps a member when background identity completion changes its key", () => {
    const session = createViewerSession("quick", [{ key: "path-8", pathId: 8 }], new Set(["path-8"]), "path-8")!;
    const reconciled = reconcileViewerSession(session, [{ key: "hash", pathId: 8 }])!;
    expect(viewerCurrentKey(reconciled)).toBe("hash");
  });
});
