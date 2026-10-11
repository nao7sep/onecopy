// @vitest-environment happy-dom

import { expect, it, vi } from "vitest";
import { EMPTY_ITEM_WORK, type SectionItem } from "../../src/models/items";
import { useItemsStore } from "../../src/state/items-store";
import { usePreviewStore } from "../../src/state/preview-store";
import { installItemWorkflow } from "../../src/workflows/items";
import { installPreviewCommandWiring } from "../../src/workflows/preview";
import { fireEvent, mockCommands, resetTauriMocks, WebviewWindow } from "../mocks/tauri";
import { inEnglish } from "../helpers/i18n";

function item(hash: string, pathId: number): SectionItem {
  return {
    hash, pathId, fileName: `${hash}.jpg`, byteSize: 1, copyCount: 1,
    width: 1, height: 1, hasThumb: true, similarGroupId: null,
    sharpness: null, faceScore: null, hasCompanions: false, durationMs: null,
    resolvedUtcMs: 0, dirPaths: [], derivedWork: EMPTY_ITEM_WORK,
  };
}

it("recovers failed Preview close from Main's latest selection and refuses the previous Delete gesture", async () => {
  resetTauriMocks({ keepListeners: true });
  mockCommands({ patch_state: () => ({}), log_event: () => null, record_recent_notification: () => ({}) });
  usePreviewStore.setState({ follow: false, placement: null, placementPreference: "window", current: null, error: null });
  useItemsStore.setState({
    items: [item("a", 1), item("b", 2)], selectedItem: "a", selectedKeys: new Set(["a"]), detail: null,
  });
  installItemWorkflow();
  await installPreviewCommandWiring();
  const window = new WebviewWindow("preview");
  const area = document.createElement("div");
  area.id = "main-item-area";
  const forwarded = vi.fn();
  area.addEventListener("keydown", forwarded);
  document.body.appendChild(area);
  try {
    await usePreviewStore.getState().open({ hash: "a", pathId: null }, null);
    const previousRevision = usePreviewStore.getState().current!.selectionRevision;
    let rejectClose!: (error: Error) => void;
    window.destroy.mockImplementationOnce(() => new Promise<void>((_resolve, reject) => { rejectClose = reject; }));
    usePreviewStore.getState().close();
    await vi.waitFor(() => expect(rejectClose).toBeDefined());
    useItemsStore.setState({ selectedItem: "b", selectedKeys: new Set(["b"]), detail: null });
    rejectClose(new Error("native destroy failed"));
    await vi.waitFor(() => expect(usePreviewStore.getState().error).not.toBeNull());

    const recovered = usePreviewStore.getState().current;
    expect(recovered).toMatchObject({ hash: "b", selectedCount: 1 });
    expect(recovered?.selectionRevision).not.toBe(previousRevision);
    expect(inEnglish(usePreviewStore.getState().error)).toBe("Couldn’t close the Preview window.");
    fireEvent("preview://key", { key: "Delete", selectionRevision: previousRevision });
    expect(forwarded).not.toHaveBeenCalled();
    fireEvent("preview://key", { key: "Delete", selectionRevision: recovered?.selectionRevision });
    expect(forwarded).toHaveBeenCalledOnce();
    usePreviewStore.getState().close();
    await vi.waitFor(() => expect(window.destroy).toHaveBeenCalledTimes(2));
  } finally {
    area.remove();
  }
});
