// @vitest-environment happy-dom
//
// Launch puts keyboard focus where work starts: the folder tree when no
// month was open, and a surface that already holds focus keeps it.

import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { act, cleanup, render } from "@testing-library/react";
import { ReadyApp } from "../../src/App";
import { useAppStore } from "../../src/state/app-store";
import { useSectionsStore } from "../../src/state/sections-store";
import { pushModal, popModal } from "../../src/utils/modalStack";
import type { LoadedAppData } from "../../src/repositories";
import { mockCommands, resetTauriMocks } from "../mocks/tauri";

const READY_APP_DATA: LoadedAppData = {
  config: { sourceDirs: [], defaultTimezone: "UTC" },
  state: {},
  dataRoot: "/data",
  debugEnabled: false,
  quarantines: [],
};

const frame = () => new Promise((resolve) => requestAnimationFrame(() => resolve(null)));

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
  useSectionsStore.setState({ counts: { images: [], videos: [], others: [] } });
});

afterEach(() => cleanup());

describe("launch focus", () => {
  it("focuses the folder tree when no month was open", async () => {
    render(<ReadyApp appData={READY_APP_DATA} />);
    await act(frame);
    expect(document.activeElement?.id).toBe("section-tree");
  });

  it("leaves focus with a surface that holds it", async () => {
    const token = {};
    pushModal(token);
    render(<ReadyApp appData={READY_APP_DATA} />);
    await act(frame);
    expect(document.activeElement?.id).not.toBe("section-tree");
    popModal(token);
  });
});
