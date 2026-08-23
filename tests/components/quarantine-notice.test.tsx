// @vitest-environment happy-dom
//
// The user-facing half of corrupt-store recovery. Setting the unreadable file
// aside preserves the bytes, but a set-aside nobody mentions is a silent reset
// with extra steps (storage-path-conventions), so what this surface says IS
// the contract: which file, where its bytes are now, what the app is running
// on instead, and what was left alone.

import { beforeEach, afterEach, describe, expect, it } from "vitest";
import { render, cleanup, act } from "@testing-library/react";
import QuarantineNotice from "../../src/components/QuarantineNotice";
import { useAppStore } from "../../src/state/app-store";
import { resetTauriMocks } from "../mocks/tauri";

const CONFIG = {
  file: "config.json",
  quarantinedTo: "/Users/x/.onecopy/config-20260817-031500-123-utc.invalid",
};
const STATE = {
  file: "state.json",
  quarantinedTo: "/Users/x/.onecopy/state-20260817-031500-124-utc.invalid",
};

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  useAppStore.setState({ quarantines: [] });
});

afterEach(() => cleanup());

describe("when nothing was quarantined", () => {
  it("renders nothing at all", () => {
    const view = render(<QuarantineNotice />);
    expect(view.container.textContent).toBe("");
  });
});

describe("when a store was set aside", () => {
  beforeEach(() => {
    useAppStore.setState({ quarantines: [CONFIG] });
  });

  it("names the file, the exact path holding the bytes, and what it started with", () => {
    render(<QuarantineNotice />);
    const text = document.body.textContent ?? "";
    expect(text).toContain("config.json");
    // The path must be exact — its whole purpose is that the user can go get
    // the original bytes.
    expect(text).toContain(CONFIG.quarantinedTo);
    expect(text).toContain("built-in settings");
    // And the reassurance that the reset was scoped to that one file.
    expect(text).toContain("photos");
  });

  it("says what a lost view state actually costs, not the file's name only", () => {
    useAppStore.setState({ quarantines: [STATE] });
    render(<QuarantineNotice />);
    const text = document.body.textContent ?? "";
    expect(text).toContain("sort order");
    expect(text).toContain(STATE.quarantinedTo);
  });

  it("reports every store on its own line when several failed", () => {
    useAppStore.setState({ quarantines: [CONFIG, STATE] });
    render(<QuarantineNotice />);
    expect(document.querySelectorAll("li")).toHaveLength(2);
  });

  it("dismisses, and stays dismissed", async () => {
    const view = render(<QuarantineNotice />);
    const ok = [...document.querySelectorAll("button")].find((b) => b.textContent === "OK")!;
    await act(async () => ok.click());
    expect(useAppStore.getState().quarantines).toEqual([]);
    expect(view.container.textContent).toBe("");
  });
});
