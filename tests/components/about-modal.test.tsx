// @vitest-environment happy-dom

import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import AboutModal from "../../src/components/AboutModal";
import {
  LATEST_RELEASE_PAGE,
  useReleaseCheckStore,
} from "../../src/state/release-check-store";

const mocks = vi.hoisted(() => ({
  openUrl: vi.fn(),
  recordActionFailure: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: mocks.openUrl }));
vi.mock("../../src/state/notifications-store", () => ({
  recordActionFailure: mocks.recordActionFailure,
}));
vi.mock("../../src/repositories", () => ({
  log: { warn: vi.fn() },
  toErrorFields: (error: unknown) => ({ error }),
}));

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  useReleaseCheckStore.setState({
    checking: false,
    manualResult: null,
    noticeVersion: null,
    noticeLinkError: null,
  });
});

describe("About link results", () => {
  it("identifies the application licence", () => {
    render(<AboutModal open onClose={() => undefined} />);

    expect(screen.getByText(/GNU GPL v3 or later/)).toBeTruthy();
  });

  it("presents a manual newer result and opens only the fixed release page", async () => {
    useReleaseCheckStore.setState({
      manualResult: { status: "newer", version: "0.2.0" },
    });
    render(<AboutModal open onClose={() => undefined} />);

    expect(screen.getByRole("button", { name: "Check GitHub for New Release" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "View Release on GitHub" }));

    await waitFor(() => expect(mocks.openUrl).toHaveBeenCalledWith(LATEST_RELEASE_PAGE));
  });

  it("retains authored modal-local copy and diagnostics separately", async () => {
    const hostile = new TypeError("EACCES /private/tmp/HOSTILE-SENTINEL IPC wrapper");
    mocks.openUrl.mockRejectedValueOnce(hostile);
    render(<AboutModal open onClose={() => undefined} />);

    fireEvent.click(screen.getByRole("button", { name: "GitHub" }));

    const result = await screen.findByRole("alert");
    expect(result.textContent).toContain("Couldn’t open GitHub. Try again or open it in your browser.");
    expect(result.textContent).not.toMatch(/EACCES|HOSTILE-SENTINEL|TypeError|IPC|private\/tmp/);
    await waitFor(() => expect(mocks.recordActionFailure).toHaveBeenCalledWith(
      "about-link-open-failed",
      "Couldn’t open GitHub. Try again or open it in your browser.",
      hostile,
    ));
  });

  it("clears only its own result through the quiet X", async () => {
    mocks.openUrl.mockRejectedValueOnce(new Error("blocked"));
    render(<AboutModal open onClose={() => undefined} />);
    fireEvent.click(screen.getByRole("button", { name: "Report an issue" }));
    await screen.findByRole("alert");

    fireEvent.click(screen.getByRole("button", { name: "Close Report an issue result" }));

    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("keeps link results independent and ignores an older same-link failure", async () => {
    let rejectOlder!: (error: unknown) => void;
    mocks.openUrl
      .mockImplementationOnce(() => new Promise<void>((_resolve, reject) => { rejectOlder = reject; }))
      .mockResolvedValueOnce(undefined)
      .mockRejectedValueOnce(new Error("issues unavailable"));
    render(<AboutModal open onClose={() => undefined} />);

    fireEvent.click(screen.getByRole("button", { name: "GitHub" }));
    fireEvent.click(screen.getByRole("button", { name: "GitHub" }));
    await act(async () => rejectOlder(new Error("stale EACCES /private/tmp/ONECOPY_STALE")));
    expect(screen.queryByText(/Couldn’t open GitHub/)).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Report an issue" }));
    expect(await screen.findByText(/Couldn’t open Report an issue/)).toBeTruthy();
  });
});
