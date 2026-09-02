// @vitest-environment happy-dom
//
// The Managed tools modal after the 2026-08-17 rework. Its whole point is
// that the registry holds two different LIFECYCLES and must not blur them:
// ffmpeg is resolved live from upstream (real version, real "latest", a
// check worth running), while the models are chosen by the app build (no
// upstream, so never "Up to date", never a "checked at" stamp — but they do
// show how old the artifact is). Plus: installs run in parallel, so one
// row's download never disables another's.

import { beforeEach, afterEach, describe, expect, it, vi } from "vitest";
import { render, cleanup, act } from "@testing-library/react";
import BinariesModal from "../../src/components/BinariesModal";
import { useBinariesStore, type DependencyState } from "../../src/state/binaries-store";
import { invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";

const downloading = {
  operationId: "download-operation",
  progress: {
    phase: "download" as const,
    done: 100 * 1_048_576,
    total: 1_207 * 1_048_576,
    nextPhase: "verify" as const,
  },
  cancelling: false,
};

function entry(
  id: string,
  status: DependencyState["status"],
  over: Partial<DependencyState> = {},
): DependencyState {
  const isBinary = id === "ffmpeg";
  return {
    id,
    label: id,
    kind: isBinary ? "binary" : "model",
    status,
    // Read from the artifact, not from facts — so it stands beside them, not
    // inside them.
    installedVersion: status === "not-installed" ? null : isBinary ? "9.0" : "1fc70f774d38",
    facts: {
      latestKnownVersion: status === "not-installed" ? null : isBinary ? "9.0" : "1fc70f774d38",
      lastCheckedAtUtc: isBinary ? "2026-08-17T00:00:00.000Z" : null,
    },
    path: "",
    requiredForCore: isBinary,
    checkable: isBinary,
    released: isBinary ? null : "2024-10-01",
    ...over,
  };
}

function seed(entries: DependencyState[]): void {
  useBinariesStore.setState({
    modalOpen: true,
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
    entries,
    loading: false,
    loadError: null,
  });
  mockCommands({ binaries_state: () => entries });
}

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  mockCommands({
    binaries_install: ({ id, operationId }) => {
      const current = useBinariesStore.getState().entries.find((item) => item.id === id);
      if (!current) throw new Error(`missing test dependency ${String(id)}`);
      return {
        outcome: "installed",
        operationId,
        state: {
          ...current,
          status: "up-to-date",
          installedVersion: current.installedVersion ?? "installed-test-version",
        },
      };
    },
    binaries_cancel: () => true,
    binaries_check: () => useBinariesStore.getState().entries,
    binaries_state: () => [],
  });
  seed([
    entry("ffmpeg", "not-installed"),
    entry("whisper-large-v3-turbo", "not-installed"),
    entry("ultraface-rfb640", "up-to-date"),
  ]);
});

afterEach(() => cleanup());

const buttons = (label: string) =>
  [...document.querySelectorAll("button")].filter((b) => b.textContent === label);

describe("parallel installs", () => {
  it("keeps every other row's button live while one entry downloads", () => {
    useBinariesStore.setState({
      installing: { "whisper-large-v3-turbo": downloading },
    });
    render(<BinariesModal />);
    const install = buttons("Install");
    expect(install).toHaveLength(1); // ffmpeg's — the model row shows progress
    expect(install[0]!.disabled).toBe(false);
    expect(document.body.textContent).toContain("Downloading — 100 MB / 1.2 GB (8%)");
  });

  it("keeps completed phases readable after a fast install finishes", () => {
    seed([entry("ffmpeg", "up-to-date")]);
    useBinariesStore.setState({
      installHistory: {
        ffmpeg: [
          { phase: "resolve", text: "Resolving — finding the latest build" },
          { phase: "download", text: "Downloading — 84 MB" },
          { phase: "verify", text: "Verifying — checking integrity" },
          { phase: "result", text: "Installed" },
        ],
      },
    });

    render(<BinariesModal />);

    expect(document.body.textContent).toContain("Resolving — finding the latest build");
    expect(document.body.textContent).toContain("Downloading — 84 MB");
    expect(document.body.textContent).toContain("Verifying — checking integrity");
    expect(document.body.textContent).toContain("Installed");
  });

  it("offers cancellation on the running row and sends that entry id", async () => {
    useBinariesStore.setState({
      installing: { "whisper-large-v3-turbo": downloading },
    });
    render(<BinariesModal />);

    await act(async () => buttons("Cancel")[0]!.click());

    expect(invokeCalls.filter((call) => call.command === "binaries_cancel")).toEqual([
      {
        command: "binaries_cancel",
        args: {
          id: "whisper-large-v3-turbo",
          operationId: "download-operation",
        },
      },
    ]);
    expect(document.body.textContent).toContain("Cancelling…");
  });

  it("keeps cancellation visible until the owning operation settles", async () => {
    mockCommands({ binaries_cancel: () => false });
    useBinariesStore.setState({
      installing: { "whisper-large-v3-turbo": downloading },
    });
    render(<BinariesModal />);

    await act(async () => buttons("Cancel")[0]!.click());

    expect(
      useBinariesStore.getState().installing["whisper-large-v3-turbo"]?.cancelling,
    ).toBe(true);
    expect(document.body.textContent).toContain("Cancelling…");
  });
});

describe("registry state", () => {
  it("distinguishes loading, failure, and an ordinary empty registry", () => {
    seed([]);
    useBinariesStore.setState({ loading: true });
    const loading = render(<BinariesModal />);
    expect(document.body.textContent).toContain("Loading managed tools…");
    loading.unmount();

    useBinariesStore.setState({ loading: false, loadError: "Managed tools are unavailable." });
    const failed = render(<BinariesModal />);
    expect(document.body.textContent).toContain("Managed tools are unavailable.");
    expect(document.body.textContent).not.toContain("No managed tools are configured.");
    failed.unmount();

    useBinariesStore.setState({ loadError: null });
    render(<BinariesModal />);
    expect(document.body.textContent).toContain("No managed tools are configured.");
  });

  it("keeps long managed-model identities readable instead of ellipsizing them", () => {
    const label = "Transcription model (Whisper large-v3-turbo)";
    seed([entry("ultraface-rfb640", "not-installed", { label })]);
    render(<BinariesModal />);

    const renderedLabel = [...document.querySelectorAll("span")].find(
      (element) => element.textContent === label,
    );
    expect(renderedLabel?.className).toContain("break-words");
    expect(renderedLabel?.className).not.toContain("truncate");
  });

  it("emphasizes only a missing core prerequisite and explains tool roles nearby", () => {
    seed([
      entry("ffmpeg", "not-installed", { label: "ffmpeg", requiredForCore: true }),
      entry("whisper-large-v3-turbo", "not-installed", {
        label: "Transcription model (Whisper large-v3-turbo)",
        requiredForCore: false,
      }),
    ]);
    render(<BinariesModal />);

    const rows = [...document.querySelectorAll("div.rounded-xl")];
    const ffmpegStatus = rows
      .find((row) => row.textContent?.includes("ffmpeg"))
      ?.querySelector("span.text-warning");
    const modelRow = rows.find((row) => row.textContent?.includes("Whisper large-v3-turbo"));

    expect(ffmpegStatus?.textContent).toBe("Not installed");
    expect(modelRow?.querySelector("span.text-warning")).toBeNull();
    expect(document.body.textContent).toContain("ffmpeg is required for video preparation");
    expect(document.body.textContent).toContain("models add only optional");
  });
});

describe("installing everything", () => {
  it("puts Install all above the list", () => {
    render(<BinariesModal />);
    const installAll = buttons("Install all")[0]!;
    const firstRow = [...document.querySelectorAll("span")].find(
      (el) => el.textContent === "ffmpeg",
    )!;
    expect(
      installAll.compareDocumentPosition(firstRow) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
  });

  it("targets exactly the missing and updatable entries", async () => {
    render(<BinariesModal />);
    await act(async () => buttons("Install all")[0]!.click());
    const installed = invokeCalls
      .filter((c) => c.command === "binaries_install")
      .map((c) => c.args.id);
    expect(installed.sort()).toEqual(["ffmpeg", "whisper-large-v3-turbo"]);
  });
});

// The version is read off the artifact, so a present entry can now be one whose
// artifact would not answer — the binary will not run, or its sidecar is gone.
// That is not absent and is never dressed up as current: the row stays
// installed-unchecked and offers the only move that fixes it.
describe("an entry whose version could not be read", () => {
  it("offers Update, which a merely-unchecked entry does not", () => {
    seed([entry("ffmpeg", "installed-unchecked", { installedVersion: null })]);
    render(<BinariesModal />);
    expect(buttons("Update")).toHaveLength(1);
    expect(document.body.textContent).not.toContain("Up to date");

    seed([entry("ffmpeg", "installed-unchecked")]);
    render(<BinariesModal />);
    expect(buttons("Update")).toHaveLength(0);
  });
});

describe("the two lifecycles", () => {
  it("never claims a model is UP TO DATE, and shows how old it is", () => {
    // A model has no upstream to compare against, so "Up to date" would be a
    // claim about a check nobody ran. It is simply installed — with the
    // artifact's real publication date answering "how old is this?".
    seed([entry("whisper-large-v3-turbo", "up-to-date")]);
    render(<BinariesModal />);
    expect(document.body.textContent).toContain("Installed");
    expect(document.body.textContent).not.toContain("Up to date");
    expect(document.body.textContent).not.toContain("1fc70f774d38");
    expect(document.body.textContent).toContain("Released 2024-10-01");
    // No check is offered for it — there is nothing to ask.
    expect(buttons("Check for updates")).toHaveLength(0);
    expect(document.body.textContent).toContain("Models selected by OneCopy");
    expect(document.body.textContent).toContain(
      "These models are downloaded only when you install them here.",
    );
  });

  it("presents rolling ffmpeg builds by date while retaining comparison state", () => {
    seed([
      entry("ffmpeg", "update-available", {
        installedVersion: "Latest Auto-Build (2026-08-23 13:03)",
        facts: {
          latestKnownVersion: "Latest Auto-Build (2026-08-24 14:04)",
          lastCheckedAtUtc: "2026-08-24T14:05:00.000Z",
        },
      }),
    ]);
    render(<BinariesModal />);

    expect(document.body.textContent).toContain("Build 2026-08-23 13:03");
    expect(document.body.textContent).toContain("2026-08-24 14:04 available");
    expect(document.body.textContent).not.toContain("Latest Auto-Build");
  });

  it("offers the check only on the entry that has an upstream", () => {
    seed([entry("ffmpeg", "up-to-date"), entry("whisper-large-v3-turbo", "up-to-date")]);
    render(<BinariesModal />);
    const check = buttons("Check for updates");
    expect(check).toHaveLength(1);
    // It sits INSIDE the ffmpeg row, not floating above a list it cannot
    // cover — the scope has to be visible, not explained.
    const row = check[0]!.closest("div.rounded-xl");
    expect(row?.textContent).toContain("ffmpeg");
    // ffmpeg may claim up-to-date: it really was compared against upstream.
    expect(row?.textContent).toContain("Up to date");
  });

  it("checks only the checkable entry, never the models", async () => {
    vi.useFakeTimers();
    try {
      seed([entry("ffmpeg", "up-to-date"), entry("whisper-large-v3-turbo", "up-to-date")]);
      render(<BinariesModal />);
      await act(async () => {
        buttons("Check for updates")[0]!.click();
        await vi.advanceTimersByTimeAsync(700);
      });
      const checked = invokeCalls
        .filter((c) => c.command === "binaries_check")
        .map((c) => c.args.id);
      expect(checked).toEqual(["ffmpeg"]);
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("check feedback", () => {
  it("settles a check started immediately after an install result", async () => {
    vi.useFakeTimers();
    try {
      seed([entry("ffmpeg", "up-to-date")]);
      useBinariesStore.setState({
        installHistory: { ffmpeg: [{ phase: "result", text: "Installed" }] },
      });
      render(<BinariesModal />);

      await act(async () => {
        buttons("Check for updates")[0]!.click();
        await vi.advanceTimersByTimeAsync(700);
      });

      expect(useBinariesStore.getState().checking).toBe(false);
      expect(document.body.textContent).toContain("You're up to date");
    } finally {
      vi.useRealTimers();
    }
  });

  it("acknowledges the click visibly and ends in a plain-words outcome", async () => {
    // A real check can finish in tens of milliseconds, which reads as a dead
    // button. The store holds the checking state to a visible floor, then
    // says what it found in words that persist.
    vi.useFakeTimers();
    try {
      seed([entry("ffmpeg", "up-to-date")]);
      render(<BinariesModal />);
      await act(async () => {
        buttons("Check for updates")[0]!.click();
        await vi.advanceTimersByTimeAsync(0);
      });
      expect(document.body.textContent).toContain("Checking…");
      await act(async () => {
        await vi.advanceTimersByTimeAsync(400);
      });
      expect(document.body.textContent).toContain("Checking…");
      await act(async () => {
        await vi.advanceTimersByTimeAsync(300);
      });
      expect(document.body.textContent).toContain("You're up to date");
      expect(buttons("Check for updates")[0]!.disabled).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });

  it("offers cancellation while a manual network check is running", async () => {
    vi.useFakeTimers();
    try {
      let rejectCheck: ((error: Error) => void) | undefined;
      mockCommands({
        binaries_check: () =>
          new Promise<void>((_resolve, reject) => {
            rejectCheck = reject;
          }),
        binaries_cancel: () => {
          rejectCheck?.(new Error("dependency operation cancelled"));
          return true;
        },
      });
      seed([entry("ffmpeg", "up-to-date")]);
      render(<BinariesModal />);

      await act(async () => buttons("Check for updates")[0]!.click());
      await act(async () => buttons("Cancel check")[0]!.click());

      expect(invokeCalls.filter((call) => call.command === "binaries_cancel")).toContainEqual({
        command: "binaries_cancel",
        args: {
          id: "ffmpeg",
          operationId: expect.any(String),
        },
      });
      expect(document.body.textContent).toContain("Cancelling…");
      await act(async () => vi.advanceTimersByTimeAsync(700));
      expect(document.body.textContent).toContain("Check cancelled");
    } finally {
      vi.useRealTimers();
    }
  });
});
