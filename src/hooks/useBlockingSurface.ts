// Registers a blocking overlay that is not a ModalShell — the setup wizard
// and the volume-presence gate — on the modal stack.
//
// Both cover the window completely, so the main window's command layer must
// go quiet exactly as it does under a modal. Without this, `hasOpenModal()`
// stays false behind them and two things break at once: Backspace trashes the
// selected photo invisibly, and the command layer's own `preventDefault` on
// the bubbled keydown cancels Enter activation on the overlay's buttons — so
// Next, Finish and scan, and Check again are all dead to Enter while an
// unrelated surface opens behind the overlay instead.
//
// These surfaces are deliberately NOT routed through ModalShell: neither is
// dismissable, and ModalShell exists to give a surface a Close affordance.

import { useEffect } from "react";
import { popModal, pushModal } from "../utils/modalStack";

export function useBlockingSurface(): void {
  useEffect(() => {
    const token = {};
    pushModal(token);
    return () => popModal(token);
  }, []);
}
