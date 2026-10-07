import { beforeEach, afterEach, describe, expect, it, vi } from "vitest";
import { refreshSourceAvailability, recheckSources } from "../../src/workflows/source-availability";
import { useWizardStore } from "../../src/state/wizard-store";
import { useSectionsStore } from "../../src/state/sections-store";
import { invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  useWizardStore.setState({ open: false, missingDirs: ["/photos"], substitutedDirs: [], presenceUnknown: false });
  mockCommands({ check_source_dirs: () => ({ missing: [], substituted: [] }), log_event: () => null });
});
afterEach(() => vi.restoreAllMocks());

describe("source recovery", () => {
  it("rechecks files through the existing scan owner once the source is verified", async () => {
    const start = vi.spyOn(useSectionsStore.getState(), "startSourceCheck").mockResolvedValue(true);
    await recheckSources();
    expect(useWizardStore.getState().missingDirs).toEqual([]);
    expect(start).toHaveBeenCalledExactlyOnceWith("explicit");
  });

  it.each(["substituted", "unreadable"])("never starts recovery for a %s identity", async (failure) => {
    const start = vi.spyOn(useSectionsStore.getState(), "startSourceCheck").mockResolvedValue(true);
    mockCommands({ check_source_dirs: () => {
      if (failure === "unreadable") throw new Error("identity probe failed");
      return { missing: [], substituted: ["/photos"] };
    } });
    await recheckSources();
    expect(start).not.toHaveBeenCalled();
    expect(useWizardStore.getState().presenceUnknown || useWizardStore.getState().substitutedDirs.length > 0).toBe(true);
  });

  it("joins repeated clicks and permits a later retry after admission fails", async () => {
    let finish!: (value: boolean) => void;
    const start = vi.spyOn(useSectionsStore.getState(), "startSourceCheck")
      .mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }))
      .mockResolvedValue(true);
    const first = recheckSources();
    const second = recheckSources();
    expect(first).toBe(second);
    await vi.waitFor(() => expect(start).toHaveBeenCalledOnce());
    finish(false);
    await first;
    await recheckSources();
    expect(start).toHaveBeenCalledTimes(2);
  });

  it("coalesces completion bursts and refreshes a returning source without another scan", async () => {
    const start = vi.spyOn(useSectionsStore.getState(), "startSourceCheck").mockResolvedValue(true);
    let finish!: (value: { missing: string[]; substituted: string[] }) => void;
    let reads = 0;
    mockCommands({ check_source_dirs: () => ++reads === 1
      ? new Promise((resolve) => { finish = resolve; })
      : { missing: [], substituted: [] } });
    const first = refreshSourceAvailability();
    for (let i = 0; i < 10; i++) void refreshSourceAvailability();
    finish({ missing: ["/photos"], substituted: [] });
    await first;
    expect(invokeCalls.filter((call) => call.command === "check_source_dirs")).toHaveLength(2);
    expect(useWizardStore.getState().missingDirs).toEqual([]);
    expect(start).not.toHaveBeenCalled();
  });
});
