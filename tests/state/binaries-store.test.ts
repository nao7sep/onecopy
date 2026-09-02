import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  useBinariesStore,
  type BinaryInstallResult,
  type DependencyState,
} from "../../src/state/binaries-store";
import {
  invokeCalls,
  mockCommands,
  resetTauriMocks,
} from "../mocks/tauri";

function entry(
  id: string,
  status: DependencyState["status"],
  installedVersion: string | null = null,
): DependencyState {
  return {
    id,
    label: id,
    kind: id === "ffmpeg" ? "binary" : "model",
    status,
    installedVersion,
    facts: {
      latestKnownVersion: installedVersion,
      lastCheckedAtUtc: null,
    },
    path: `/managed/${id}`,
    requiredForCore: id === "ffmpeg",
    checkable: id === "ffmpeg",
    released: null,
  };
}

function seed(entries: DependencyState[]): void {
  useBinariesStore.setState({
    entries,
    loading: false,
    loadError: null,
    installing: {},
    installHistory: {},
    errors: {},
    checking: false,
    checkingId: null,
    checkingOperationId: null,
    checkCancelling: false,
    cooldownUntil: 0,
    lastCheckOutcome: null,
    lastCheckOutcomeLevel: null,
    modalOpen: true,
  });
}

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  seed([
    entry("ffmpeg", "not-installed"),
    entry("whisper-large-v3-turbo", "not-installed"),
  ]);
  mockCommands({
    publish_notification: () => ({}),
    binaries_cancel: () => true,
  });
});

describe("managed-tool terminal ownership", () => {
  it("applies the authoritative row before making a completed operation idle", async () => {
    const installed = entry("ffmpeg", "up-to-date", "9.1");
    mockCommands({
      binaries_install: ({ operationId }) => ({
        outcome: "installed",
        operationId,
        state: installed,
      }),
    });

    await useBinariesStore.getState().install("ffmpeg");

    expect(useBinariesStore.getState().entries[0]).toEqual(installed);
    expect(useBinariesStore.getState().installing.ffmpeg).toBeUndefined();
    expect(useBinariesStore.getState().installHistory.ffmpeg.at(-1)).toEqual({
      phase: "result",
      text: "Installed",
    });
    expect(invokeCalls.filter((call) => call.command === "binaries_state")).toEqual([]);
  });

  it("keeps completion when the modal is replaced during the operation", async () => {
    let finish!: (result: BinaryInstallResult) => void;
    let operationId = "";
    mockCommands({
      binaries_install: (args) => {
        operationId = String(args.operationId);
        return new Promise<BinaryInstallResult>((resolve) => {
          finish = resolve;
        });
      },
    });

    const installing = useBinariesStore.getState().install("ffmpeg");
    await Promise.resolve();
    useBinariesStore.getState().setModalOpen(false);
    finish({
      outcome: "installed",
      operationId,
      state: entry("ffmpeg", "up-to-date", "9.2"),
    });
    await installing;
    useBinariesStore.getState().setModalOpen(true);

    expect(useBinariesStore.getState().entries[0]?.status).toBe("up-to-date");
    expect(useBinariesStore.getState().installing.ffmpeg).toBeUndefined();
  });

  it("retains partial artifact facts and the operation error together", async () => {
    mockCommands({
      binaries_install: ({ operationId }) => ({
        outcome: "failed",
        operationId,
        state: entry("ffmpeg", "installed-unchecked", null),
        error: "version sidecar could not be saved",
      }),
    });

    await useBinariesStore.getState().install("ffmpeg");

    expect(useBinariesStore.getState().entries[0]?.status).toBe("installed-unchecked");
    expect(useBinariesStore.getState().errors.ffmpeg).toBe(
      "The managed-tool installation could not finish. Try again.",
    );
    expect(useBinariesStore.getState().installing.ffmpeg).toBeUndefined();
  });

  it("settles concurrent entries independently without replacing another result", async () => {
    const finishes = new Map<string, (result: BinaryInstallResult) => void>();
    const operationIds = new Map<string, string>();
    mockCommands({
      binaries_install: (args) => {
        const id = String(args.id);
        operationIds.set(id, String(args.operationId));
        return new Promise<BinaryInstallResult>((resolve) => {
          finishes.set(id, resolve);
        });
      },
    });

    const ffmpeg = useBinariesStore.getState().install("ffmpeg");
    const whisper = useBinariesStore.getState().install("whisper-large-v3-turbo");
    await Promise.resolve();
    finishes.get("ffmpeg")?.({
      outcome: "installed",
      operationId: operationIds.get("ffmpeg")!,
      state: entry("ffmpeg", "up-to-date", "9.3"),
    });
    await ffmpeg;

    expect(useBinariesStore.getState().installing.ffmpeg).toBeUndefined();
    expect(
      useBinariesStore.getState().installing["whisper-large-v3-turbo"],
    ).toBeDefined();

    finishes.get("whisper-large-v3-turbo")?.({
      outcome: "installed",
      operationId: operationIds.get("whisper-large-v3-turbo")!,
      state: entry("whisper-large-v3-turbo", "up-to-date", "model-pin"),
    });
    await whisper;

    expect(useBinariesStore.getState().entries.map((item) => item.status)).toEqual([
      "up-to-date",
      "up-to-date",
    ]);
  });

  it("correlates cancellation and applies its authoritative terminal row", async () => {
    let finish!: (result: BinaryInstallResult) => void;
    let operationId = "";
    mockCommands({
      binaries_install: (args) => {
        operationId = String(args.operationId);
        return new Promise<BinaryInstallResult>((resolve) => {
          finish = resolve;
        });
      },
    });

    const installing = useBinariesStore.getState().install("ffmpeg");
    await Promise.resolve();
    await useBinariesStore.getState().cancel("ffmpeg");
    expect(invokeCalls.filter((call) => call.command === "binaries_cancel")).toEqual([
      {
        command: "binaries_cancel",
        args: { id: "ffmpeg", operationId },
      },
    ]);
    finish({
      outcome: "cancelled",
      operationId,
      state: entry("ffmpeg", "not-installed"),
    });
    await installing;

    expect(useBinariesStore.getState().installing.ffmpeg).toBeUndefined();
    expect(useBinariesStore.getState().errors.ffmpeg).toBeUndefined();
    expect(useBinariesStore.getState().installHistory.ffmpeg.at(-1)?.text).toBe(
      "Cancelled",
    );
  });

  it("does not let a refresh begun before installation restore a stale action", async () => {
    let finishLoad!: (entries: DependencyState[]) => void;
    mockCommands({
      binaries_state: () => new Promise<DependencyState[]>((resolve) => {
        finishLoad = resolve;
      }),
      binaries_install: ({ operationId }) => ({
        outcome: "installed",
        operationId,
        state: entry("ffmpeg", "up-to-date", "9.4"),
      }),
    });

    const loading = useBinariesStore.getState().load();
    await Promise.resolve();
    await useBinariesStore.getState().install("ffmpeg");
    finishLoad([
      entry("ffmpeg", "not-installed"),
      entry("whisper-large-v3-turbo", "not-installed"),
    ]);
    await loading;

    expect(useBinariesStore.getState().entries[0]?.status).toBe("up-to-date");
    expect(useBinariesStore.getState().loading).toBe(false);
  });

  it("rejects a late terminal result after a newer attempt owns the row", async () => {
    let finish!: (result: BinaryInstallResult) => void;
    let oldOperationId = "";
    mockCommands({
      binaries_install: (args) => {
        oldOperationId = String(args.operationId);
        return new Promise<BinaryInstallResult>((resolve) => {
          finish = resolve;
        });
      },
    });

    const oldAttempt = useBinariesStore.getState().install("ffmpeg");
    await Promise.resolve();
    useBinariesStore.setState({
      installing: {
        ffmpeg: {
          operationId: "newer-attempt",
          progress: null,
          cancelling: false,
        },
      },
    });
    finish({
      outcome: "installed",
      operationId: oldOperationId,
      state: entry("ffmpeg", "up-to-date", "obsolete-result"),
    });
    await oldAttempt;

    expect(useBinariesStore.getState().installing.ffmpeg?.operationId).toBe(
      "newer-attempt",
    );
    expect(useBinariesStore.getState().entries[0]?.status).toBe("not-installed");
  });

  it("applies an authoritative check response without a second status request", async () => {
    vi.useFakeTimers();
    try {
      const checked = entry("ffmpeg", "up-to-date", "9.5");
      seed([
        entry("ffmpeg", "installed-unchecked", "9.5"),
        entry("whisper-large-v3-turbo", "not-installed"),
      ]);
      mockCommands({
        binaries_check: () => ({
          outcome: "completed",
          states: [
            checked,
            entry("whisper-large-v3-turbo", "not-installed"),
          ],
        }),
      });

      const checking = useBinariesStore.getState().checkAll();
      await Promise.resolve();
      await vi.advanceTimersByTimeAsync(700);
      await checking;

      expect(useBinariesStore.getState().entries[0]).toEqual(checked);
      expect(invokeCalls.filter((call) => call.command === "binaries_state")).toEqual([]);
      expect(invokeCalls.find((call) => call.command === "binaries_check")?.args).toEqual({
        id: "ffmpeg",
        operationId: expect.any(String),
      });
    } finally {
      vi.useRealTimers();
    }
  });
});
