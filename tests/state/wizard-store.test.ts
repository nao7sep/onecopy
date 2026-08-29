// The wizard's Finish step.

import { beforeEach, describe, expect, it } from "vitest";
import { useWizardStore } from "../../src/state/wizard-store";
import { finishWizard } from "../../src/workflows/wizard";
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
    start_source_check: () => true,
    get_section_counts: () => [],
  });
  useWizardStore.setState({
    open: true,
    step: 2,
    dirs: [{ path: "/root", counting: false }] as never,
    timezone: "Asia/Tokyo",
    timezoneValid: true,
    timezonePending: false,
    error: null,
  });
});

describe("finish", () => {
  it("saves the directories and timezone", async () => {
    await finishWizard();

    const merged = Object.assign({}, ...patchConfigPayloads()) as Record<
      string,
      unknown
    >;
    expect(merged.sourceDirs).toEqual(["/root"]);
    expect(merged.defaultTimezone).toBe("Asia/Tokyo");
  });

  it("does not save while timezone validation is pending or invalid", async () => {
    useWizardStore.setState({ timezoneValid: false, timezonePending: true });
    await finishWizard();
    useWizardStore.setState({ timezonePending: false });
    await finishWizard();

    expect(patchConfigPayloads()).toEqual([]);
  });
});

describe("timezone validation", () => {
  it("ignores an older reply that arrives after the current value", async () => {
    let settleOld: ((valid: boolean) => void) | undefined;
    let settleCurrent: ((valid: boolean) => void) | undefined;
    mockCommands({
      validate_timezone: ({ name }) =>
        new Promise<boolean>((resolve) => {
          if (name === "Tokyo") settleOld = resolve;
          else settleCurrent = resolve;
        }),
    });

    const old = useWizardStore.getState().setTimezone("Tokyo");
    const current = useWizardStore.getState().setTimezone("Asia/Tokyo");
    settleCurrent?.(true);
    await current;
    settleOld?.(false);
    await old;

    expect(useWizardStore.getState()).toMatchObject({
      timezone: "Asia/Tokyo",
      timezoneValid: true,
      timezonePending: false,
    });
  });
});

describe("loaded directory projection", () => {
  it("wrong-shape source members cannot suppress first-run setup", async () => {
    await useWizardStore.getState().init({ sourceDirs: [123, null, { path: "/wrong" }] });

    expect(useWizardStore.getState().open).toBe(true);
    expect(useWizardStore.getState().dirs).toEqual([]);
  });
});
