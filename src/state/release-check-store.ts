import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { create } from "zustand";
import { log, toErrorFields, type LoadedAppData } from "../repositories";
import { recordActionFailure } from "./notifications-store";

export const LATEST_RELEASE_PAGE =
  "https://github.com/nao7sep/onecopy/releases/latest";
const DAY_MS = 24 * 60 * 60 * 1000;

export type ReleaseCheckOutcome =
  | { status: "newer"; version: string; attemptedAtUtc: string }
  | { status: "current"; publishedVersion: string; attemptedAtUtc: string }
  | { status: "failed"; attemptedAtUtc: string };

export type ManualReleaseResult =
  | { status: "newer"; version: string }
  | { status: "current" }
  | { status: "failed" };

interface ReleaseCheckState {
  /** Process-lifetime launch admission; renderer remounts cannot repeat it. */
  automaticStarted: boolean;
  checking: boolean;
  manualResult: ManualReleaseResult | null;
  noticeVersion: string | null;
  noticeLinkError: string | null;
  checkManual: () => Promise<void>;
  dismissNotice: () => void;
  openNoticeRelease: () => Promise<void>;
}

interface ActiveCheck {
  promise: Promise<ReleaseCheckOutcome>;
  manualRequested: boolean;
}

let activeCheck: ActiveCheck | null = null;

export function releaseCheckEligible(value: unknown, nowMs = Date.now()): boolean {
  if (typeof value !== "string") return true;
  const attemptedMs = Date.parse(value);
  return !Number.isFinite(attemptedMs)
    || attemptedMs > nowMs
    || nowMs - attemptedMs >= DAY_MS;
}

function beginCheck(manual: boolean): ActiveCheck {
  if (activeCheck !== null) {
    if (manual) activeCheck.manualRequested = true;
    return activeCheck;
  }
  const operation: ActiveCheck = {
    manualRequested: manual,
    promise: Promise.resolve(null as never),
  };
  operation.promise = invoke<ReleaseCheckOutcome>("check_github_release")
    .finally(() => {
      if (activeCheck === operation) activeCheck = null;
    });
  activeCheck = operation;
  return operation;
}

export async function openLatestReleasePage(): Promise<void> {
  await openUrl(LATEST_RELEASE_PAGE);
}

export async function startAutomaticReleaseCheck(data: LoadedAppData): Promise<void> {
  if (useReleaseCheckStore.getState().automaticStarted) return;
  useReleaseCheckStore.setState({ automaticStarted: true });
  if (data.config?.checkGithubReleasesAtLaunch === false) return;
  if (!releaseCheckEligible(data.state?.githubReleaseLastAttemptAtUtc)) return;
  const operation = beginCheck(false);
  try {
    const outcome = await operation.promise;
    if (outcome.status === "newer" && !operation.manualRequested) {
      useReleaseCheckStore.setState({ noticeVersion: outcome.version, noticeLinkError: null });
    }
  } catch (error) {
    // Automatic failure is deliberately quiet in UI; the core also records
    // request diagnostics, while this catches timestamp-persistence/IPC loss.
    log.warn("automatic GitHub release check failed", toErrorFields(error));
  }
}

export const useReleaseCheckStore = create<ReleaseCheckState>((set) => ({
  automaticStarted: false,
  checking: false,
  manualResult: null,
  noticeVersion: null,
  noticeLinkError: null,
  checkManual: async () => {
    const operation = beginCheck(true);
    set({ checking: true, manualResult: null });
    try {
      const outcome = await operation.promise;
      set({
        checking: false,
        manualResult: outcome.status === "newer"
          ? { status: "newer", version: outcome.version }
          : { status: outcome.status },
      });
    } catch (error) {
      log.error("manual GitHub release check failed", toErrorFields(error));
      set({ checking: false, manualResult: { status: "failed" } });
    }
  },
  dismissNotice: () => set({ noticeVersion: null, noticeLinkError: null }),
  openNoticeRelease: async () => {
    set({ noticeLinkError: null });
    try {
      await openLatestReleasePage();
    } catch (error) {
      const message = "Couldn’t open the release page. Try again or open it in your browser.";
      log.warn("release notice link open failed", toErrorFields(error));
      recordActionFailure("release-link-open-failed", message, error);
      set({ noticeLinkError: message });
    }
  },
}));
