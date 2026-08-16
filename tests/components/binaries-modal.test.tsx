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
    facts: {
      installedVersion: status === "not-installed" ? null : isBinary ? "9.0" : "1fc70f774d38",
      latestKnownVersion: status === "not-installed" ? null : isBinary ? "9.0" : "1fc70f774d38",
      lastCheckedAtUtc: isBinary ? "2026-08-17T00:00:00.000Z" : null,
    },
    path: "",
    checkable: isBinary,
    released: isBinary ? null : "2024-10-01",
    ...over,
  };
}

function seed(entries: DependencyState[]): void {
  useBinariesStore.setState({
    modalOpen: true,
    installing: {},
    errors: {},
    checking: false,
    cooldownUntil: 0,
    lastCheckOutcome: null,
    entries,
  });
  mockCommands({ binaries_state: () => entries });
}

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  mockCommands({
    binaries_install: () => null,
    binaries_check: () => null,
    binaries_state: () => [],
  });
  seed([
    entry("ffmpeg", "not-installed"),
    entry("clip-vit-b32", "not-installed"),
    entry("whisper-large-v3-turbo", "up-to-date"),
  ]);
});

afterEach(() => cleanup());

const buttons = (label: string) =>
  [...document.querySelectorAll("button")].filter((b) => b.textContent === label);

describe("parallel installs", () => {
  it("keeps every other row's button live while one entry downloads", () => {
    useBinariesStore.setState({
      installing: { "clip-vit-b32": "Downloading — 100 / 335 MB" },
    });
    render(<BinariesModal />);
    const install = buttons("Install");
    expect(install).toHaveLength(1); // ffmpeg's — clip's row shows progress instead
    expect(install[0]!.disabled).toBe(false);
    expect(document.body.textContent).toContain("Downloading — 100 / 335 MB");
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
    expect(installed.sort()).toEqual(["clip-vit-b32", "ffmpeg"]);
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
    expect(document.body.textContent).toContain("Version 1fc70f774d38");
    expect(document.body.textContent).toContain("Released 2024-10-01");
    // No check is offered for it — there is nothing to ask.
    expect(buttons("Check for updates")).toHaveLength(0);
    expect(document.body.textContent).toContain("Included with OneCopy");
  });

  it("offers the check only on the entry that has an upstream", () => {
    seed([entry("ffmpeg", "up-to-date"), entry("clip-vit-b32", "up-to-date")]);
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
});
