import { beforeEach, expect, it } from "vitest";
import { useAppShellStore } from "../../src/state/app-shell-store";
import { useAppStore } from "../../src/state/app-store";
import { useSettingsStore } from "../../src/state/settings-store";
import { effectiveConfig } from "../helpers/config";

beforeEach(() => {
  useAppShellStore.setState({ utilitySurface: null });
  useSettingsStore.setState({ draft: null, opened: null, saving: false });
  useAppStore.setState({ appData: {
    config: effectiveConfig({ sourceDirs: ["/photos"] }),
    configDefaults: effectiveConfig(),
    aiAccelerationCapabilities: [],
    state: {},
    dataRoot: "/test-data",
    debugEnabled: false,
    quarantines: [],
  } });
});

it.each([null, "issues", "backgroundWork"] as const)("prepares Settings before replacing %s", (surface) => {
  useAppShellStore.setState({ utilitySurface: surface });
  let draftAtPublication = false;
  const stop = useAppShellStore.subscribe((state) => {
    if (state.utilitySurface === "settings") draftAtPublication = useSettingsStore.getState().draft !== null;
  });
  useAppShellStore.getState().openUtility("settings");
  stop();
  expect(draftAtPublication).toBe(true);
  expect(useSettingsStore.getState().draft?.sourceDirs).toEqual(["/photos"]);
});

it("keeps existing edits and prepares a fresh draft after discard or save", () => {
  const shell = useAppShellStore.getState();
  shell.openUtility("settings");
  useSettingsStore.getState().update({ notificationDisplaySeconds: 23 });
  const draft = useSettingsStore.getState().draft;
  shell.openUtility("settings");
  expect(useSettingsStore.getState().draft).toBe(draft);
  useSettingsStore.getState().discardDraft();
  shell.closeUtility();
  shell.openUtility("settings");
  expect(useSettingsStore.getState().draft?.notificationDisplaySeconds).not.toBe(23);
  useSettingsStore.setState({ draft: null, opened: null });
  shell.closeUtility();
  shell.openUtility("settings");
  expect(useSettingsStore.getState().draft).not.toBeNull();
});
