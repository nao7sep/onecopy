// @vitest-environment happy-dom

import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import AboutModal from "../../src/components/AboutModal";

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
});

describe("About link results", () => {
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

    fireEvent.click(screen.getByRole("button", { name: "Dismiss link result" }));

    expect(screen.queryByRole("alert")).toBeNull();
  });
});
