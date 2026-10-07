// @vitest-environment happy-dom
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import QuitSaveModal from "../../src/components/QuitSaveModal";
import { useQuitSaveStore } from "../../src/workflows/quit";

afterEach(() => { cleanup(); useQuitSaveStore.setState({ choose: null }); });

it("focuses Cancel and requires the explicit destructive choice", async () => {
  const choose = vi.fn();
  useQuitSaveStore.setState({ choose });
  render(<QuitSaveModal />);
  expect(screen.getByRole("dialog", { name: "Settings could not be saved" })).toBeTruthy();
  await waitFor(() => expect(document.activeElement).toBe(screen.getByRole("button", { name: "Cancel" })));
  fireEvent.click(screen.getByRole("button", { name: "Retry" }));
  expect(choose).toHaveBeenLastCalledWith("retry");
  fireEvent.click(screen.getByRole("button", { name: "Quit anyway" }));
  expect(choose).toHaveBeenLastCalledWith("quit");
  fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
  expect(choose).toHaveBeenLastCalledWith("cancel");
  act(() => useQuitSaveStore.setState({ choose: null }));
  expect(screen.queryByRole("dialog")).toBeNull();
});
