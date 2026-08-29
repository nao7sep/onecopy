// @vitest-environment happy-dom
//
// The setup wizard and the volume-presence gate cover the whole window but
// are not ModalShells, so nothing registered them on the modal stack. That
// left the main window's command layer live behind them, which broke two
// things at once: Backspace trashed the selected photo invisibly, and the
// command layer's own preventDefault on the bubbled keydown cancelled Enter
// activation on the overlays' buttons — Next, Finish and scan, and Check
// again were all dead to Enter.
//
// `hasOpenModal()` is the single predicate the command layer consults, so
// asserting it is asserting the fix.

import { beforeEach, afterEach, describe, expect, it } from "vitest";
import { render, cleanup, act } from "@testing-library/react";
import PresenceGate from "../../src/components/PresenceGate";
import Wizard from "../../src/components/Wizard";
import { hasOpenModal } from "../../src/utils/modalStack";
import { useWizardStore } from "../../src/state/wizard-store";
import { mockCommands, resetTauriMocks } from "../mocks/tauri";

beforeEach(() => {
  resetTauriMocks();
  mockCommands({
    patch_state: () => ({}),
    patch_config: () => ({}),
    validate_timezone: () => true,
    check_source_dirs: () => ({ missing: [], substituted: [] }),
  });
});

afterEach(() => cleanup());

describe("the presence gate", () => {
  it("silences the command layer while it blocks work", () => {
    expect(hasOpenModal()).toBe(false);
    render(<PresenceGate missing={["/Volumes/Photos"]} substituted={[]} />);
    expect(hasOpenModal()).toBe(true);
  });

  it("releases the command layer once it closes", () => {
    const view = render(
      <PresenceGate missing={["/Volumes/Photos"]} substituted={[]} />,
    );
    view.unmount();
    expect(hasOpenModal()).toBe(false);
  });
});

describe("the setup wizard", () => {
  beforeEach(() => {
    useWizardStore.setState({ step: 1, dirs: [], timezone: "UTC", error: null });
  });

  it("is exactly two steps, ending in Finish and scan — no install page", () => {
    // The offer page is deliberately GONE (developer, 2026-08-17): Managed
    // tools is the one install surface, funnelled to by the warning chip. A
    // wizard install row would be a second UI drifting from it.
    const view = render(<Wizard />);
    expect(view.container.textContent).toContain("Step 1 of 2");

    act(() => useWizardStore.setState({ step: 2 }));
    expect(view.container.textContent).toContain("Step 2 of 2");
    expect(view.container.textContent).toContain("Finish and scan");
    expect(view.container.textContent).not.toContain("ffmpeg");
  });

  it("offers Cancel on every page of a re-run, and never on a first run", () => {
    // Being deep in the wizard is no reason to walk back out first (developer,
    // 2026-08-17). A FIRST run stays completable-only: nothing exists behind
    // it to cancel back to.
    const view = render(<Wizard />);
    for (const step of [1, 2] as const) {
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
