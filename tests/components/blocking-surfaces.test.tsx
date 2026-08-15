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
    useWizardStore.setState({ step: 1, dirs: [], timezone: "UTC" });
  });

  it("counts only the numbered steps, never the ffmpeg offer", () => {
    // The design is "three steps and one offer". The offer is step 4 in the
    // flow's own counter, so a hardcoded "of 3" rendered "Step 4 of 3" — a
    // number that cannot be true and reads as a bug the moment it is seen.
    const view = render(<Wizard dataRoot="/tmp/onecopy" />);
    expect(view.container.textContent).toContain("Step 1 of 3");

    for (const step of [2, 3]) {
      act(() => useWizardStore.setState({ step }));
      expect(view.container.textContent).toContain(`Step ${step} of 3`);
    }

    act(() => useWizardStore.setState({ step: 4 }));
    expect(view.container.textContent).not.toContain("of 3");
    expect(view.container.textContent).toContain("Optional");
  });

  it("silences the command layer while it is open", () => {
    expect(hasOpenModal()).toBe(false);
    render(<Wizard dataRoot="/tmp/onecopy" />);
    expect(hasOpenModal()).toBe(true);
  });

  it("releases the command layer once it closes", () => {
    const view = render(<Wizard dataRoot="/tmp/onecopy" />);
    view.unmount();
    expect(hasOpenModal()).toBe(false);
  });
});
