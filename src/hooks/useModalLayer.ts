import { useEffect, useRef, type RefObject } from "react";
import { isComposingEvent } from "./useComposing";
import { resolveInitialFocus, resolveTrapTarget } from "../utils/focusTrap";
import { isTopmostModal, popModal, pushModal } from "../utils/modalStack";
import { acquireScrollLock, releaseScrollLock } from "../utils/scrollLock";

/** Shared behavior for both framed dialogs and full-window transient layers. */
export function useModalLayer(
  surfaceRef: RefObject<HTMLElement | null>,
  onClose: () => void,
  closeDisabled = false,
): void {
  const tokenRef = useRef<object>({});
  const onCloseRef = useRef(onClose);
  const closeDisabledRef = useRef(closeDisabled);
  onCloseRef.current = onClose;
  closeDisabledRef.current = closeDisabled;

  useEffect(() => {
    const token = tokenRef.current;
    const opener = document.activeElement as HTMLElement | null;
    pushModal(token);
    acquireScrollLock();
    const raf = requestAnimationFrame(() => {
      const surface = surfaceRef.current;
      if (surface !== null && isTopmostModal(token)) resolveInitialFocus(surface).focus();
    });
    const onKeyDown = (event: KeyboardEvent) => {
      if (!isTopmostModal(token)) return;
      if (event.key === "Escape") {
        if (isComposingEvent(event)) return;
        event.preventDefault();
        event.stopPropagation();
        if (!closeDisabledRef.current) onCloseRef.current();
      } else if (event.key === "Tab") {
        const surface = surfaceRef.current;
        if (surface === null) return;
        const target = resolveTrapTarget(surface, document.activeElement, event.shiftKey);
        if (target !== null) {
          event.preventDefault();
          target.focus();
        }
      }
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => {
      cancelAnimationFrame(raf);
      window.removeEventListener("keydown", onKeyDown, true);
      releaseScrollLock();
      popModal(token);
      opener?.focus();
    };
  }, [surfaceRef]);
}
