import { beforeEach, describe, expect, it } from "vitest";
import {
  LATEST_RELEASE_PAGE,
  releaseCheckEligible,
  startAutomaticReleaseCheck,
  useReleaseCheckStore,
} from "../../src/state/release-check-store";
import { useAppStore } from "../../src/state/app-store";
import {
  invokeCalls,
  mockCommands,
  openUrl,
  resetTauriMocks,
} from "../mocks/tauri";

const data = {
  config: {},
  state: {},
  dataRoot: "/tmp/onecopy-release-test",
  debugEnabled: false,
  quarantines: [],
};

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  mockCommands({ log_event: () => null, record_recent_notification: () => null });
  useAppStore.setState({ appData: data, startupFailure: null, quarantines: [] });
  useReleaseCheckStore.setState({
    automaticStarted: false,
    checking: false,
    manualResult: null,
    noticeVersion: null,
    noticeLinkError: null,
  });
});

describe("GitHub release checking", () => {
  it("treats missing, invalid, future, and day-old attempts as eligible", () => {
    const now = Date.parse("2026-09-10T00:00:00.000Z");
    expect(releaseCheckEligible(undefined, now)).toBe(true);
    expect(releaseCheckEligible("invalid", now)).toBe(true);
    expect(releaseCheckEligible("2026-09-10T00:00:00.001Z", now)).toBe(true);
    expect(releaseCheckEligible("2026-09-09T00:00:00.000Z", now)).toBe(true);
    expect(releaseCheckEligible("2026-09-09T00:00:00.001Z", now)).toBe(false);
  });

  it("keeps an automatic current result quiet and runs only once", async () => {
    mockCommands({ check_github_release: () => ({
      status: "current",
      publishedVersion: "0.1.0",
      attemptedAtUtc: "2026-09-10T00:00:00.000Z",
    }) });
    await startAutomaticReleaseCheck(data);
    await startAutomaticReleaseCheck(data);
    expect(invokeCalls.filter(({ command }) => command === "check_github_release")).toHaveLength(1);
    expect(useReleaseCheckStore.getState().noticeVersion).toBeNull();
  });

  it("respects the launch preference and keeps automatic failures quiet", async () => {
    mockCommands({ check_github_release: () => ({
      status: "failed",
      attemptedAtUtc: "2026-09-10T00:00:00.000Z",
    }) });
    await startAutomaticReleaseCheck({ ...data, config: { checkGithubReleasesAtLaunch: false } });
    expect(invokeCalls.some(({ command }) => command === "check_github_release")).toBe(false);

    useReleaseCheckStore.setState({ automaticStarted: false });
    await startAutomaticReleaseCheck(data);
    expect(invokeCalls.filter(({ command }) => command === "check_github_release")).toHaveLength(1);
    expect(useReleaseCheckStore.getState().noticeVersion).toBeNull();
    expect(useReleaseCheckStore.getState().manualResult).toBeNull();
  });

  it("shows an automatic newer release without opening GitHub", async () => {
    mockCommands({ check_github_release: () => ({
      status: "newer",
      version: "0.2.0",
      attemptedAtUtc: "2026-09-10T00:00:00.000Z",
    }) });
    await startAutomaticReleaseCheck(data);
    expect(useReleaseCheckStore.getState().noticeVersion).toBe("0.2.0");
    expect(openUrl).not.toHaveBeenCalled();
    await useReleaseCheckStore.getState().openNoticeRelease();
    expect(openUrl).toHaveBeenCalledWith(LATEST_RELEASE_PAGE);
  });

  it("joins a manual action to the launch request and presents that result once", async () => {
    let resolve!: (value: unknown) => void;
    mockCommands({ check_github_release: () => new Promise((done) => { resolve = done; }) });
    const automatic = startAutomaticReleaseCheck(data);
    const manual = useReleaseCheckStore.getState().checkManual();
    resolve({
      status: "newer",
      version: "0.2.0",
      attemptedAtUtc: "2026-09-10T00:00:00.000Z",
    });
    await Promise.all([automatic, manual]);
    expect(invokeCalls.filter(({ command }) => command === "check_github_release")).toHaveLength(1);
    expect(useReleaseCheckStore.getState().manualResult)
      .toEqual({ status: "newer", version: "0.2.0" });
    expect(useReleaseCheckStore.getState().noticeVersion).toBeNull();
  });

  it("presents manual current and failed outcomes", async () => {
    mockCommands({ check_github_release: () => ({
      status: "current",
      publishedVersion: "0.1.0",
      attemptedAtUtc: "2026-09-10T00:00:00.000Z",
    }) });
    await useReleaseCheckStore.getState().checkManual();
    expect(useReleaseCheckStore.getState().manualResult).toEqual({ status: "current" });
    mockCommands({ check_github_release: () => ({
      status: "failed",
      attemptedAtUtc: "2026-09-10T00:01:00.000Z",
    }) });
    await useReleaseCheckStore.getState().checkManual();
    expect(useReleaseCheckStore.getState().manualResult).toEqual({ status: "failed" });
  });
});
