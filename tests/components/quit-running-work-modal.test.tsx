// @vitest-environment happy-dom
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import QuitRunningWorkModal from "../../src/components/QuitRunningWorkModal";
import { useQuitRunningWorkStore } from "../../src/workflows/quit";
import { message } from "../../src/i18n/translate";

afterEach(() => { cleanup(); useQuitRunningWorkStore.setState({ choose: null, presented: null, standalone: false, body: null }); });

it("names the running work, focuses Keep running and requires the explicit Stop and quit", async () => {
  const choose = vi.fn();
  const presented = vi.fn();
  useQuitRunningWorkStore.setState({ choose, presented, body: message("quit.lines", {
    first: message("quit.runningCopy"),
    rest: message("quit.toolDownloading", { tool: message("tool.ffmpeg") }),
  }) });
  render(<QuitRunningWorkModal />);
  expect(screen.getByRole("dialog", { name: "Stop the running work and quit?" })).toBeTruthy();
  expect(screen.getByText(/A copy is running\.\s+ffmpeg is being downloaded\./)).toBeTruthy();
  expect(presented).toHaveBeenCalled();
  await waitFor(() => expect(document.activeElement).toBe(screen.getByRole("button", { name: "Keep running" })));
  fireEvent.click(screen.getByRole("button", { name: "Stop and quit" }));
  expect(choose).toHaveBeenLastCalledWith(true);
  fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
  expect(choose).toHaveBeenLastCalledWith(false);
  act(() => useQuitRunningWorkStore.setState({ choose: null }));
  expect(screen.queryByRole("dialog")).toBeNull();
});

it("draws nothing in Main for a standalone question", () => {
  useQuitRunningWorkStore.setState({ choose: vi.fn(), presented: null, standalone: true, body: message("quit.runningCopy") });
  render(<QuitRunningWorkModal />);
  expect(screen.queryByRole("dialog")).toBeNull();
});
