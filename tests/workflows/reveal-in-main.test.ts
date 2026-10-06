import { beforeEach, expect, it, vi } from "vitest";
import { revealInMain } from "../../src/workflows/reveal-in-main";
import { useItemsStore } from "../../src/state/items-store";
import { useComparisonStore } from "../../src/state/comparison-store";
import { useFullscreenViewStore } from "../../src/state/fullscreen-view-store";
import { closeViewer } from "../../src/workflows/fullscreen-view";
import { closeComparison } from "../../src/workflows/comparison";

vi.mock("../../src/workflows/fullscreen-view", () => ({ closeViewer: vi.fn() }));
vi.mock("../../src/workflows/comparison", () => ({ closeComparison: vi.fn() }));

beforeEach(() => {
  vi.restoreAllMocks();
  vi.clearAllMocks();
  useComparisonStore.setState({ open: false, busy: false, pendingAction: null });
  useFullscreenViewStore.setState({ session: null, pendingDelete: null });
});

it("closes the requesting modal before exiting the active Comparison through its owner", async () => {
  useComparisonStore.setState({ open: true });
  vi.spyOn(useItemsStore.getState(), "revealPath").mockResolvedValue("revealed");
  const order: string[] = [];
  vi.mocked(closeComparison).mockImplementation(async () => { order.push("comparison"); });
  expect(await revealInMain("/fixture/image.jpg", () => true, () => { order.push("modal"); })).toBe("revealed");
  expect(order).toEqual(["modal", "comparison"]);
  expect(closeViewer).not.toHaveBeenCalled();
});

it("leaves a busy operation untouched without even starting navigation", async () => {
  useComparisonStore.setState({ open: true, busy: true });
  const reveal = vi.spyOn(useItemsStore.getState(), "revealPath");
  const closed = vi.fn();
  expect(await revealInMain("/fixture/image.jpg", () => true, closed)).toBe("blocked");
  expect(reveal).not.toHaveBeenCalled();
  expect(closed).not.toHaveBeenCalled();
  expect(closeComparison).not.toHaveBeenCalled();
});

it("keeps an unavailable target in its current viewing context", async () => {
  useComparisonStore.setState({ open: true });
  vi.spyOn(useItemsStore.getState(), "revealPath").mockResolvedValue("unavailable");
  const closed = vi.fn();
  expect(await revealInMain("/fixture/missing.jpg", () => true, closed)).toBe("unavailable");
  expect(closed).not.toHaveBeenCalled();
  expect(closeComparison).not.toHaveBeenCalled();
});
