// @vitest-environment happy-dom
//
// The user-facing half of corrupt-store recovery. Setting the unreadable file
// aside preserves the bytes, but a set-aside nobody mentions is a silent reset
// with extra steps (storage-path-conventions), so what this surface says IS
// the contract: which file was affected, the name its bytes were kept under,
// what the app is running on instead, and what was left alone. Internal
// recovery paths stay diagnostic-only.

import { beforeEach, afterEach, describe, expect, it } from "vitest";
import { render, cleanup, act } from "@testing-library/react";
import QuarantineNotice from "../../src/components/QuarantineNotice";
import { useAppStore } from "../../src/state/app-store";
import { resetTauriMocks } from "../mocks/tauri";

const CONFIG = {
  file: "config.json",
  quarantinedTo: "/Users/x/.onecopy/config-20260817-031500-utc.invalid",
};
const STATE = {
  file: "state.json",
  quarantinedTo: "/Users/x/.onecopy/state-20260817-031500-utc.invalid",
};
const RECORDS = {
  file: "records.sqlite3",
  quarantinedTo: "C:\\Users\\x\\AppData\\OneCopy\\records-20260817-031500-utc.invalid",
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

  it("names the file and the name its original was kept under, not the whole path", () => {
    render(<QuarantineNotice />);
    const text = document.body.textContent ?? "";
    expect(text).toContain("config.json");
    expect(text).toContain("kept as config-20260817-031500-utc.invalid");
    expect(text).not.toContain(CONFIG.quarantinedTo);
    expect(text).toContain("built-in settings");
    // Nothing was written yet; saving writes the new file.
    expect(text).toContain("Saving your settings writes a new settings file");
    expect(text).not.toContain("wrote");
    // And the reassurance that the reset was scoped to that one file.
    expect(text).toContain("photos");
  });

  it("says what a lost view state actually costs, not the file's name only", () => {
    useAppStore.setState({ quarantines: [STATE] });
    render(<QuarantineNotice />);
    const text = document.body.textContent ?? "";
    expect(text).toContain("sort order");
    expect(text).not.toContain(STATE.quarantinedTo);
  });

  it("says a set-aside history starts a new one, with a Windows path", () => {
    useAppStore.setState({ quarantines: [RECORDS] });
    render(<QuarantineNotice />);
    const text = document.body.textContent ?? "";
    expect(text).toContain("records.sqlite3");
    expect(text).toContain("new history");
    expect(text).toContain("kept as records-20260817-031500-utc.invalid");
  });

  it("says a set-aside index is built again and never claims the index was untouched", () => {
    useAppStore.setState({
      quarantines: [{ file: "index.sqlite3", quarantinedTo: "/Users/x/.onecopy/index-20260817-031500-utc.invalid" }],
    });
    render(<QuarantineNotice />);
    const text = document.body.textContent ?? "";
    expect(text).toContain("new library index");
    expect(text).toContain("kept as index-20260817-031500-utc.invalid");
    expect(text).not.toContain("scan index");
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
