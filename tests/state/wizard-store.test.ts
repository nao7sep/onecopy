// The wizard's Finish step.

import { beforeEach, describe, expect, it } from "vitest";
import { useWizardStore } from "../../src/state/wizard-store";
import { invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";

function patchConfigPayloads(): Array<Record<string, unknown>> {
  return invokeCalls
    .filter((c) => c.command === "patch_config")
    .map((c) => (c.args.patch ?? c.args) as Record<string, unknown>);
}

beforeEach(() => {
  resetTauriMocks();
  mockCommands({
    patch_config: () => ({}),
    patch_state: () => ({}),
    check_source_dirs: () => ({ missing: [], substituted: [] }),
    start_scan: () => true,
    get_section_counts: () => [],
  });
  useWizardStore.setState({
    open: true,
    step: 2,
    dirs: [{ path: "/root", counting: false }] as never,
    timezone: "Asia/Tokyo",
  });
});

describe("finish", () => {
  it("saves the directories and timezone", async () => {
    await useWizardStore.getState().finish();

    const merged = Object.assign({}, ...patchConfigPayloads()) as Record<
      string,
      unknown
    >;
    expect(merged.sourceDirs).toEqual(["/root"]);
    expect(merged.defaultTimezone).toBe("Asia/Tokyo");
  });

});

describe("loaded directory projection", () => {
  it("wrong-shape source members cannot suppress first-run setup", async () => {
    await useWizardStore.getState().init({ sourceDirs: [123, null, { path: "/wrong" }] });

    expect(useWizardStore.getState().open).toBe(true);
    expect(useWizardStore.getState().dirs).toEqual([]);
  });
});
