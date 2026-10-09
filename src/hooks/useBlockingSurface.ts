// Registers a blocking overlay that is not a ModalShell — the setup wizard
// and the volume-presence gate — on the modal stack.
//
// Both cover the window completely, so the main window's command layer must
// go quiet exactly as it does under a modal. Without this, `hasOpenModal()`
// stays false behind them and two things break at once: Backspace trashes the
// selected photo invisibly, and the command layer's own `preventDefault` on
// the bubbled keydown cancels Enter activation on the overlay's buttons — so
// Next, Finish and scan, and Check source folders are all dead to Enter while an
// unrelated surface opens behind the overlay instead.
//
// These surfaces are deliberately NOT routed through ModalShell: neither is
// dismissable, and ModalShell exists to give a surface a Close affordance.

import { useEffect, useState } from "react";
import { popModal, pushModal } from "../utils/modalStack";

// A blocking surface also owns keyboard focus while it is open: it focuses
// its own first control, and when it closes, focus returns to what held it
// before, unless something else has taken it meanwhile.
export function useBlockingSurface(): void {
  // Read on the first render, before the surface's own control takes focus.
  const [opener] = useState(() =>
    document.activeElement instanceof HTMLElement ? document.activeElement : null,
  );
  useEffect(() => {
    const token = {};
    pushModal(token);
    return () => {
      popModal(token);
      // React runs this before the surface leaves the document, so focus is
      // checked once it has.
      queueMicrotask(() => {
        const current = document.activeElement;
        const unclaimed = current === null || current === document.body || !current.isConnected;
        if (opener !== null && opener !== document.body && opener.isConnected && unclaimed) {
          opener.focus();
        }
      });
    };
  }, [opener]);
}
