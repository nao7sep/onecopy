// @vitest-environment happy-dom

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import QuickView from "../../src/components/QuickView";
import { EMPTY_ITEM_WORK } from "../../src/models/items";
import { useItemsStore } from "../../src/state/items-store";
import { useQuickViewStore } from "../../src/state/quick-view-store";
import { resetModalStack } from "../../src/utils/modalStack";
import { mockCommands, resetTauriMocks } from "../mocks/tauri";

function Host() {
  const session = useQuickViewStore((state) => state.session);
  return session?.presentation === "quick" ? <QuickView /> : null;
}

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  resetModalStack();
  mockCommands({ patch_state: () => null });
  useQuickViewStore.setState({
    session: {
      presentation: "quick",
      members: [{ key: "photo-hash", pathId: 1 }],
      index: 0,
      scope: "section",
    },
    pendingDelete: null,
  });
  useItemsStore.setState({
    selected: { kind: "image", month: "2026-01" },
    items: [
      {
        hash: "photo-hash",
        pathId: 1,
        fileName: "family.jpg",
        resolvedUtcMs: 0,
        copyCount: 1,
        width: 4000,
        height: 3000,
        hasThumb: true,
        similarGroupId: null,
        sharpness: null,
        faceScore: null,
        byteSize: 10,
        hasCompanions: false,
        durationMs: null,
        dirPaths: ["/photos"],
        derivedWork: EMPTY_ITEM_WORK,
      },
    ],
    selectedItem: "photo-hash",
    selectedKeys: new Set(["photo-hash"]),
    detail: {
      fileName: "family.jpg",
      kind: "image",
      byteSize: 10,
      width: 4000,
      height: 3000,
      durationMs: null,
      dateState: "dated" as const,
      resolvedUtcMs: 0,
      resolvedSource: "metadata",
      dateOnly: false,
      copyPaths: ["/photos/family.jpg"],
      companionPaths: [],
      stripFrames: null,
    },
  });
});

afterEach(() => {
  cleanup();
  resetModalStack();
});

describe("Quick View", () => {
  it("uses the shared media surface and Escape returns focus to the grid", async () => {
    const opener = document.createElement("button");
    document.body.append(opener);
    opener.focus();
    render(<Host />);
    expect(screen.getByRole("dialog", { name: "Quick View" })).toBeTruthy();
    expect(screen.getByAltText("family.jpg")).toBeTruthy();

    await act(async () => fireEvent.keyDown(window, { key: "Escape" }));

    expect(useQuickViewStore.getState().session).toBeNull();
    expect(document.activeElement).toBe(opener);
    opener.remove();
  });

  it("keeps viewer commands active while a notification control has focus", async () => {
    render(
      <>
        <Host />
        <button data-notification>Dismiss notification</button>
      </>,
    );
    const notification = screen.getByRole("button", { name: "Dismiss notification" });
    notification.focus();

    fireEvent.keyDown(notification, { key: "Enter" });
    expect(useQuickViewStore.getState().session?.presentation).toBe("quick");

    await act(async () => fireEvent.keyDown(notification, { key: " " }));
    expect(useQuickViewStore.getState().session).toBeNull();
  });
});
