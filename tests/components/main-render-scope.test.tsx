// @vitest-environment happy-dom
//
// A scan tick re-renders the status line, not the whole main window. The
// sidebar stands in for the shell's other children: it re-renders exactly
// when the shell does.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render } from "@testing-library/react";
import type { LoadedAppData } from "../../src/repositories";
import { mockCommands, resetTauriMocks } from "../mocks/tauri";

let sidebarRenders = 0;
vi.mock("../../src/components/Sidebar", () => ({
  default: () => {
    sidebarRenders += 1;
    return null;
  },
}));

const { ReadyApp } = await import("../../src/App");
const { useAppStore } = await import("../../src/state/app-store");
const { useSectionsStore } = await import("../../src/state/sections-store");

const READY_APP_DATA: LoadedAppData = {
  config: { sourceDirs: [], defaultTimezone: "UTC" },
  state: {},
  dataRoot: "/data",
  debugEnabled: false,
  quarantines: [],
};

function progress(done: number) {
  return {
    phase: "hash",
    done,
    total: 100,
    currentPath: null,
    discovered: null,
    bytesDone: null,
    bytesTotal: null,
    failures: 0,
    nextPhase: "extract",
  };
}

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  mockCommands({
    get_section_counts: () => ({ images: [], videos: [], others: [] }),
    get_issues: () => ({ issues: [], total: 0 }),
    binaries_state: () => null,
    patch_state: () => ({}),
    log_event: () => null,
    logging_debug_enabled: () => false,
  });
  useAppStore.setState({ appData: READY_APP_DATA, startupFailure: null, quarantines: [] });
});

afterEach(() => cleanup());

describe("main window render scope", () => {
  it("re-renders only the status line on a progress tick", () => {
    act(() => {
      useSectionsStore.setState((state) => ({
        sourceCheck: { ...state.sourceCheck, running: true, progress: progress(1) },
      }));
    });
    const view = render(<ReadyApp appData={READY_APP_DATA} />);
    const status = () => view.container.querySelector("footer > span")?.textContent ?? "";
    const before = status();
    const renders = sidebarRenders;

    act(() => {
      useSectionsStore.setState((state) => ({
        sourceCheck: { ...state.sourceCheck, progress: progress(50) },
      }));
    });

    expect(status()).not.toBe(before);
    expect(sidebarRenders).toBe(renders);
  });
});
