// @vitest-environment happy-dom
//
// The setup wizard and substituted-volume gate cover the whole window but are
// not ModalShells, so nothing registered them on the modal stack. That
// left the main window's command layer live behind them, which broke two
// things at once: Backspace trashed the selected photo invisibly, and the
// command layer's own preventDefault on the bubbled keydown cancelled Enter
// activation on the overlays' buttons — Next, Finish and scan, and Check again
// were all dead to Enter. A merely missing source is a nonblocking notice.
//
// `hasOpenModal()` is the single predicate the command layer consults, so
// asserting it is asserting the fix.

import { beforeEach, afterEach, describe, expect, it, vi } from "vitest";
import { render, cleanup, act, fireEvent } from "@testing-library/react";
import {
  MissingSourcesNotice,
  SubstitutedSourceGate,
} from "../../src/components/SourceAvailability";
import Wizard from "../../src/components/Wizard";
import { hasOpenModal } from "../../src/utils/modalStack";
import { useWizardStore } from "../../src/state/wizard-store";
import { mockCommands, resetTauriMocks } from "../mocks/tauri";

beforeEach(() => {
  resetTauriMocks();
  mockCommands({
    patch_state: () => ({}),
    save_config: () => ({}),
    validate_timezone: () => true,
    check_source_dirs: () => ({ missing: [], substituted: [] }),
  });
});

afterEach(() => cleanup());

describe("the presence gate", () => {
  it("silences the command layer for an unsafe substituted volume", () => {
    expect(hasOpenModal()).toBe(false);
    render(
      <SubstitutedSourceGate
        substituted={["/Volumes/Photos"]}
        unknown={false}
        onRecheck={() => {}}
        onReconfigure={() => {}}
      />,
    );
    expect(hasOpenModal()).toBe(true);
  });

  it("releases the command layer once it closes", () => {
    const view = render(
      <SubstitutedSourceGate
        substituted={["/Volumes/Photos"]}
        unknown={false}
        onRecheck={() => {}}
        onReconfigure={() => {}}
      />,
    );
    view.unmount();
    expect(hasOpenModal()).toBe(false);
  });

  it("blocks the same way when the check itself failed, without showing a stale or empty list (R3-07)", () => {
    const view = render(
      <SubstitutedSourceGate
        substituted={[]}
        unknown={true}
        onRecheck={() => {}}
        onReconfigure={() => {}}
      />,
    );
    expect(hasOpenModal()).toBe(true);
    expect(view.queryByRole("list")).toBeNull();
    view.unmount();
  });

  it("keeps missing sources nonblocking and exposes both recovery actions", () => {
    const recheck = vi.fn();
    const reconfigure = vi.fn();
    const view = render(
      <MissingSourcesNotice
        missing={["/Volumes/Photos"]}
        onRecheck={recheck}
        onReconfigure={reconfigure}
      />,
    );

    expect(hasOpenModal()).toBe(false);
    expect(view.container.textContent).toContain("keeps indexed items and cached previews visible");
    fireEvent.click(view.getByRole("button", { name: "Check again" }));
    fireEvent.click(view.getByRole("button", { name: "Settings…" }));
    expect(recheck).toHaveBeenCalledOnce();
    expect(reconfigure).toHaveBeenCalledOnce();
  });
});

describe("the setup wizard", () => {
  beforeEach(() => {
    useWizardStore.setState({ step: 1, dirs: [], timezone: "UTC", error: null });
  });

  it("separates required preparation from optional features without adding an install page", () => {
    const view = render(<Wizard />);
    for (const step of [1, 2, 3, 4] as const) {
      act(() => useWizardStore.setState({ step }));
      expect(view.container.textContent).toContain(`Step ${step} of 4`);
    }
    expect(view.container.textContent).toContain("OneCopy always prepares");
    expect(view.container.textContent).toContain("Additional features");
    expect(view.container.textContent).toContain("Finish and scan");
  });

  it("offers Cancel on every page of a re-run, and never on a first run", () => {
    // Being deep in the wizard is no reason to walk back out first (developer,
    // 2026-08-17). A FIRST run stays completable-only: nothing exists behind
    // it to cancel back to.
    const view = render(<Wizard />);
    for (const step of [1, 2, 3, 4] as const) {
      act(() => useWizardStore.setState({ step, reconfigure: true }));
      expect(view.container.textContent).toContain("Cancel");
      act(() => useWizardStore.setState({ step, reconfigure: false }));
      expect(view.container.textContent).not.toContain("Cancel");
    }
  });

  it("silences the command layer while it is open", () => {
    expect(hasOpenModal()).toBe(false);
    render(<Wizard />);
    expect(hasOpenModal()).toBe(true);
  });

  it("releases the command layer once it closes", () => {
    const view = render(<Wizard />);
    view.unmount();
    expect(hasOpenModal()).toBe(false);
  });
});
