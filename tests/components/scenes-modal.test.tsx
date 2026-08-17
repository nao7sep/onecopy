// @vitest-environment happy-dom
//
// The scenes modal was the app's only permanent, trash-bypassing delete with
// no confirmation, and it resolved its target from the grid's multi-selection
// rather than the video it displays. Both are destructive-safety properties,
// so they are asserted through the real component and a real keydown.

import { beforeEach, afterEach, describe, expect, it } from "vitest";
import { render, screen, cleanup } from "@testing-library/react";
import ScenesModal from "../../src/components/ScenesModal";
import { useItemsStore } from "../../src/state/items-store";
import type { SectionItem } from "../../src/models/items";
import { invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";

function video(pathId: number): SectionItem {
  return {
    hash: `h${pathId}`,
    pathId,
    fileName: `CLIP_${pathId}.mov`,
    resolvedUtcMs: pathId * 1000,
    copyCount: 1,
    width: 1920,
    height: 1080,
    hasThumb: true,
    similarGroupId: null,
    sharpness: null,
    byteSize: 5000,
    hasCompanions: false,
    durationMs: 30000,
  };
}

/** Three videos, all selected, with the modal opened on the middle one. */
function seedMultiSelection(): void {
  const items = [video(1), video(2), video(3)];
  useItemsStore.setState({
    selected: { kind: "video", month: "2026-01" },
    items,
    loading: false,
    selectedItem: "h2",
    selectedKeys: new Set(["h1", "h2", "h3"]),
    rangeOrigin: "h2",
    rangeBase: new Set(["h1", "h2", "h3"]),
    detail: {
      fileName: "CLIP_2.mov",
      kind: "video",
      byteSize: 5000,
      width: 1920,
      height: 1080,
      durationMs: 30000,
      resolvedUtcMs: 2000,
      resolvedSource: "metadata",
      dateOnly: false,
      copyPaths: ["/root/CLIP_2.mov"],
      companionPaths: [],
      stripFrames: 4,
    },
    sortOrders: { media: "time", other: "name" },
    message: null,
  });
}

function press(key: string, shiftKey = false): void {
  window.dispatchEvent(
    new KeyboardEvent("keydown", { key, shiftKey, bubbles: true }),
  );
}

function deleteInvokes() {
  return invokeCalls.filter((c) => c.command === "delete_item");
}

beforeEach(() => {
  resetTauriMocks();
  mockCommands({
    transcript_get: () => null,
    patch_state: () => ({}),
    get_item_detail: () => null,
    delete_item: () => ({ deletedFiles: 1, failedFiles: 0, removedRows: 1 }),
    get_section_items: () => [],
    get_section_counts: () => [],
  });
  seedMultiSelection();
});

afterEach(() => cleanup());

describe("permanent delete", () => {
  it("asks before bypassing the trash", async () => {
    render(<ScenesModal hash="h2" onClose={() => {}} />);

    press("Backspace", true);

    expect(deleteInvokes()).toHaveLength(0);
    expect(await screen.findByText(/Delete permanently\?/i)).toBeTruthy();
  });

  it("deletes only the video it is showing once confirmed", async () => {
    render(<ScenesModal hash="h2" onClose={() => {}} />);
    press("Backspace", true);

    const confirm = await screen.findByRole("button", {
      name: /delete permanently/i,
    });
    confirm.click();
    await Promise.resolve();
    await Promise.resolve();

    const deleted = deleteInvokes();
    expect(deleted).toHaveLength(1);
    expect(deleted[0]?.args.hash).toBe("h2");
    expect(deleted[0]?.args.permanent).toBe(true);
  });
});

describe("trash delete", () => {
  it("acts on the displayed video, not the grid's multi-selection", async () => {
    render(<ScenesModal hash="h2" onClose={() => {}} />);

    // Three videos are selected behind the modal; the footer promises the key
    // acts on "the video", so exactly one may be destroyed.
    press("Backspace");
    await Promise.resolve();
    await Promise.resolve();

    const deleted = deleteInvokes();
    expect(deleted).toHaveLength(1);
    expect(deleted[0]?.args.hash).toBe("h2");
    expect(deleted[0]?.args.permanent).toBe(false);
  });
});
