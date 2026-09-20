// @vitest-environment happy-dom
// The wizard's Finish step.

import { beforeEach, describe, expect, it } from "vitest";
import { useSectionsStore } from "../../src/state/sections-store";
import { useWizardStore } from "../../src/state/wizard-store";
import { finishWizard } from "../../src/workflows/wizard";
import { invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";
import { inEnglish } from "../helpers/i18n";

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
    record_recent_notification: () => ({}),
  });
  useWizardStore.setState({
    open: true,
    step: 2,
    dirs: [{ path: "/root", counting: false }] as never,
    timezone: "Asia/Tokyo",
    error: null,
    finishing: false,
  });
});

describe("finish", () => {
  it("saves the directories, timezone, and optional feature choices", async () => {
    useWizardStore.setState({
      optionalFeatures: {
        videoSnapshotsEnabled: true,
        similarPhotoAnalysisEnabled: false,
        scoreFaces: true,
        videoTranscriptionEnabled: false,
        audioTranscriptionEnabled: true,
      },
    });
    await finishWizard();

    const merged = Object.assign({}, ...patchConfigPayloads()) as Record<
      string,
      unknown
    >;
    expect(merged.sourceDirs).toEqual(["/root"]);
    expect(merged.defaultTimezone).toBe("Asia/Tokyo");
    expect(merged).toMatchObject({
      videoSnapshotsEnabled: true,
      similarPhotoAnalysisEnabled: false,
      scoreFaces: true,
      videoTranscriptionEnabled: false,
      audioTranscriptionEnabled: true,
    });
    expect(useWizardStore.getState().open).toBe(false);
  });

  it("does not save without a timezone", async () => {
    useWizardStore.setState({ timezone: "   " });
    await finishWizard();

    expect(patchConfigPayloads()).toEqual([]);
  });

  // One occurrence, one report: the open form shows the failure, so the core's
  // generic storage notice must not appear over it, and the Issue that keeps the
  // failure for later comes from the form rather than from a second notice.
  it("reports a failed setup save once, in the form, and keeps one Issue", async () => {
    mockCommands({
      patch_config: () => Promise.reject(new TypeError("EACCES writing config.json")),
    });

    await finishWizard();

    expect(useWizardStore.getState()).toMatchObject({ open: true, finishing: false });
    expect(inEnglish(useWizardStore.getState().error)).toBe(
      "Setup could not be saved. Your changes are still here; try again.",
    );
    const write = invokeCalls.find((call) => call.command === "patch_config");
    expect(write?.args.reportFailure).toBe(false);
    expect(invokeCalls.filter((call) => call.command === "record_recent_notification")).toHaveLength(1);
    expect(invokeCalls.some((call) => call.command === "publish_notification")).toBe(false);
    // The commit point failed, so nothing downstream of it ran.
    expect(invokeCalls.some((call) => call.command === "start_source_check")).toBe(false);
  });

  // Once the setup is saved, nothing may tell the form it was not. The automatic check
  // reports at the source check, which is where its recovery lives.
  it("leaves a failed automatic check to the source check that owns it", async () => {
    mockCommands({
      start_source_check: () => Promise.reject(new TypeError("source check unavailable")),
    });

    await finishWizard();

    expect(useWizardStore.getState().open).toBe(false);
    expect(useWizardStore.getState().error).toBeNull();
    expect(inEnglish(useSectionsStore.getState().error)).toBe(
      "Couldn’t start checking source folders.",
    );
    const recorded = invokeCalls
      .filter((call) => call.command === "record_recent_notification")
      .map((call) => (call.args.request as { kind: string }).kind);
    expect(recorded).toEqual(["source-check-start-failed"]);
  });

  it("admits one Finish submission and keeps newer draft state open", async () => {
    let finishSave: (() => void) | undefined;
    mockCommands({
      patch_config: () =>
        new Promise<Record<string, never>>((resolve) => {
          finishSave = () => resolve({});
        }),
    });

    const first = finishWizard();
    const duplicate = finishWizard();
    expect(first).toBe(duplicate);
    expect(patchConfigPayloads()).toHaveLength(1);
    useWizardStore.setState({ timezone: "UTC" });
    finishSave?.();
    await first;

    expect(useWizardStore.getState()).toMatchObject({
      open: true,
      finishing: false,
      timezone: "UTC",
    });
    expect(inEnglish(useWizardStore.getState().error)).toBe(
      "Setup was saved, but newer changes are still open. Review them, then finish again.",
    );
    expect(patchConfigPayloads()).toHaveLength(1);
  });
});

describe("the staged timezone", () => {
  it("takes the chosen zone without asking the core to check it", () => {
    useWizardStore.getState().setTimezone("Asia/Tokyo");

    expect(useWizardStore.getState().timezone).toBe("Asia/Tokyo");
    expect(invokeCalls.some((call) => call.command === "validate_timezone")).toBe(false);
  });
});

describe("loaded directory projection", () => {
  it("wrong-shape source members cannot suppress first-run setup", async () => {
    await useWizardStore.getState().init({ sourceDirs: [123, null, { path: "/wrong" }] });

    expect(useWizardStore.getState().open).toBe(true);
    expect(useWizardStore.getState().dirs).toEqual([]);
  });

  it("keeps only the newest configured-source availability reply", async () => {
    let settleOld: ((status: { missing: string[]; substituted: string[] }) => void) | undefined;
    let settleCurrent: ((status: { missing: string[]; substituted: string[] }) => void) | undefined;
    let request = 0;
    mockCommands({
      check_source_dirs: () =>
        new Promise<{ missing: string[]; substituted: string[] }>((resolve) => {
          request += 1;
          if (request === 1) settleOld = resolve;
          else settleCurrent = resolve;
        }),
    });

    const old = useWizardStore.getState().recheckPresence();
    const current = useWizardStore.getState().recheckPresence();
    settleCurrent?.({ missing: ["/current"], substituted: [] });
    await current;
    settleOld?.({ missing: ["/obsolete"], substituted: [] });
    await old;

    expect(useWizardStore.getState().missingDirs).toEqual(["/current"]);
  });

  it("does not publish an obsolete presence failure after a newer check succeeds", async () => {
    let rejectOld: ((error: Error) => void) | undefined;
    let request = 0;
    mockCommands({
      check_source_dirs: () => {
        request += 1;
        return request === 1
          ? new Promise<{ missing: string[]; substituted: string[] }>((_resolve, reject) => {
              rejectOld = reject;
            })
          : { missing: [], substituted: [] };
      },
    });

    const old = useWizardStore.getState().recheckPresence();
    await useWizardStore.getState().recheckPresence();
    rejectOld?.(new Error("obsolete presence failure"));
    await old;

    expect(useWizardStore.getState().error).toBeNull();
    expect(
      invokeCalls.filter((call) => call.command === "record_recent_notification"),
    ).toEqual([]);
  });
});
