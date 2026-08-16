import { describe, expect, it } from "vitest";
import {
  dropMode,
  keyboardMoveMode,
} from "../../src/components/DestinationsTab";

function key(
  modifiers: Partial<Pick<KeyboardEvent, "altKey" | "ctrlKey" | "metaKey" | "shiftKey">> = {},
): KeyboardEvent {
  return {
    altKey: false,
    ctrlKey: false,
    metaKey: false,
    shiftKey: false,
    ...modifiers,
  } as KeyboardEvent;
}

describe("destination move mode", () => {
  it("stands down for AltGr instead of selecting a destructive fallback", () => {
    expect(keyboardMoveMode(key({ ctrlKey: true, altKey: true }))).toBeNull();
  });

  it("maps the three keyboard actions", () => {
    expect(keyboardMoveMode(key({ ctrlKey: true }))).toBe("copy");
    expect(keyboardMoveMode(key({ shiftKey: true }))).toBe("move-delete-rest");
    expect(keyboardMoveMode(key())).toBe("move-trash-rest");
  });

  it("keeps pointer modifiers independent of AltGr keyboard handling", () => {
    expect(dropMode(key({ metaKey: true, altKey: true }))).toBe("copy");
  });
});
