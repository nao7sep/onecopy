// @vitest-environment happy-dom
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import MetadataPane from "../../src/components/MetadataPane";
import type { ItemDetail } from "../../src/models/items";
import type { GroupMember } from "../../src/state/comparison-store";
import { useItemsStore } from "../../src/state/items-store";
import { seedAppConfig } from "../helpers/config";
import { invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";

const detail: ItemDetail = {
  fileName: "photo.jpg", kind: "image", byteSize: 100, width: 10, height: 10, durationMs: null,
  dateState: "undated", resolvedUtcMs: null, resolvedSource: null, dateOnly: false,
  copyPaths: ["/photo.jpg"], copyCount: 1, companionPaths: [], stripFrames: null, previewRevision: null,
};
function member(hash: string): GroupMember {
  return { hash, fileName: `${hash}.jpg`, width: 10, height: 10, byteSize: 100, sharpness: null, faceScore: null, copyCount: 1, hasThumb: true };
}
const pane = (snapshot: ItemDetail = detail) => <MetadataPane hash="photo" detail={snapshot} item={null} />;
const select = vi.fn(async (_hash: string) => {});
beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  seedAppConfig();
  mockCommands({ log_event: () => null });
  select.mockClear();
  vi.spyOn(useItemsStore.getState(), "selectIdentity").mockImplementation(select);
  vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockReturnValue(252);
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

describe("Details similar membership", () => {
  it("shows a newly created group and removes vanished members for the unchanged anchor", async () => {
    let members: GroupMember[] = [];
    mockCommands({ get_similar_group: () => members });
    const view = render(pane());
    await waitFor(() => expect(screen.queryByText("Loading…")).toBeNull());
    expect(screen.queryByRole("listbox")).toBeNull();
    members = [member("photo"), member("sibling")];
    view.rerender(pane({ ...detail }));
    expect(await screen.findAllByRole("option")).toHaveLength(2);
    members = [member("photo")];
    view.rerender(pane({ ...detail }));
    await waitFor(() => expect(screen.queryByRole("listbox")).toBeNull());
    expect(screen.queryByRole("button", { name: "Compare" })).toBeNull();
  });

  it("fences old membership replies after a newer detail publication", async () => {
    let old!: (rows: GroupMember[]) => void;
    mockCommands({ get_similar_group: () => new Promise<GroupMember[]>((resolve) => { old = resolve; }) });
    const view = render(pane());
    await waitFor(() => expect(old).toBeTypeOf("function"));
    mockCommands({ get_similar_group: () => [member("photo"), member("current")] });
    view.rerender(pane({ ...detail }));
    view.rerender(pane({ ...detail, copyCount: 2 }));
    expect(invokeCalls.filter((call) => call.command === "get_similar_group")).toHaveLength(1);
    await act(async () => old([member("photo"), member("obsolete")]));
    await screen.findByAltText("current.jpg");
    expect(invokeCalls.filter((call) => call.command === "get_similar_group")).toHaveLength(2);
    expect(screen.queryByAltText("obsolete.jpg")).toBeNull();
    expect(screen.getByAltText("current.jpg")).toBeTruthy();
  });

  it("bounds mounted thumbnails while keyboard and scrolling reach the whole group", async () => {
    const members = [member("photo"), ...Array.from({ length: 4999 }, (_, index) => member(`item-${index}`))];
    mockCommands({ get_similar_group: () => members });
    render(pane());
    const list = await screen.findByRole("listbox");
    expect(screen.getAllByRole("option").length).toBeLessThanOrEqual(8);
    act(() => list.focus());
    fireEvent.keyDown(list, { key: "End" });
    const active = document.getElementById(list.getAttribute("aria-activedescendant")!);
    expect(active?.getAttribute("aria-posinset")).toBe("5000");
    expect(screen.getAllByRole("option").length).toBeLessThanOrEqual(8);
    fireEvent.keyDown(list, { key: "Enter" });
    expect(select).toHaveBeenCalledWith("item-4998");
    list.scrollLeft = 0;
    fireEvent.scroll(list);
    expect(document.getElementById(list.getAttribute("aria-activedescendant")!)).toBeTruthy();
    expect(screen.getAllByRole("option").length).toBeLessThanOrEqual(8);
    expect(document.activeElement).toBe(list);
    expect(list.querySelectorAll("button, [tabindex='0']")).toHaveLength(0);
  });

  it("preserves the keyboard identity on reorder and recovers removal without stealing focus", async () => {
    let members = [member("photo"), member("b"), member("c")];
    mockCommands({ get_similar_group: () => members });
    const view = render(<><input aria-label="elsewhere" />{pane()}</>);
    const list = await screen.findByRole("listbox");
    act(() => list.focus());
    fireEvent.keyDown(list, { key: "ArrowRight" });
    const activeId = list.getAttribute("aria-activedescendant");
    members = [member("b"), member("photo"), member("c")];
    view.rerender(<><input aria-label="elsewhere" />{pane({ ...detail })}</>);
    await waitFor(() => expect(document.getElementById(activeId!)?.getAttribute("aria-posinset")).toBe("1"));
    expect(list.getAttribute("aria-activedescendant")).toBe(activeId);
    const elsewhere = screen.getByRole("textbox");
    act(() => elsewhere.focus());
    members = [member("photo"), member("c")];
    view.rerender(<><input aria-label="elsewhere" />{pane({ ...detail })}</>);
    await waitFor(() => expect(screen.queryByAltText("b.jpg")).toBeNull());
    expect(document.activeElement).toBe(elsewhere);
    expect(document.getElementById(list.getAttribute("aria-activedescendant")!)?.getAttribute("aria-posinset")).toBe("1");
  });
});
