import { useLayoutEffect, useRef, type RefObject } from "react";
import { isComposingEvent } from "./useComposing";
import { resolveFooterArrowTarget, resolveInitialFocus, resolveTrapTarget } from "../utils/focusTrap";
import { isTopmostModal, popModal, pushModal } from "../utils/modalStack";
import { acquireScrollLock, releaseScrollLock } from "../utils/scrollLock";

/** Shared behavior for both framed dialogs and full-window transient layers. */
export function useModalLayer(
  surfaceRef: RefObject<HTMLElement | null>,
  onClose: () => void,
  closeDisabled = false,
  footerArrowNavigation = false,
  returnFocus?: () => HTMLElement | null,
): object {
  const tokenRef = useRef<object>({});
  const onCloseRef = useRef(onClose);
  const closeDisabledRef = useRef(closeDisabled);
  const returnFocusRef = useRef(returnFocus);
  returnFocusRef.current = returnFocus;
  onCloseRef.current = onClose;
  closeDisabledRef.current = closeDisabled;

  useLayoutEffect(() => {
    const token = tokenRef.current;
    const surface = surfaceRef.current;
    if (surface === null) return;
    const opener = document.activeElement as HTMLElement | null;
    pushModal(token, surface, opener);
    acquireScrollLock();
    if (isTopmostModal(token)) resolveInitialFocus(surface).focus();
    const onKeyDown = (event: KeyboardEvent) => {
      if (!isTopmostModal(token) || event.defaultPrevented || isComposingEvent(event)) return;
      if (footerArrowNavigation && !event.metaKey && !event.ctrlKey && !event.altKey && !event.shiftKey
        && (event.key === "ArrowLeft" || event.key === "ArrowRight")) {
        const target = resolveFooterArrowTarget(surface, document.activeElement, event.key === "ArrowLeft" ? "left" : "right");
        if (target !== null) {
          event.preventDefault();
          event.stopPropagation();
          target.focus();
        }
      } else if (footerArrowNavigation && event.repeat && (event.key === "Enter" || event.key === " ")) {
        event.preventDefault();
        event.stopPropagation();
      }
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        if (!event.repeat && !closeDisabledRef.current) onCloseRef.current();
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
      window.removeEventListener("keydown", onKeyDown, true);
      releaseScrollLock();
      popModal(token, returnFocusRef.current?.())?.focus();
    };
  }, [surfaceRef, footerArrowNavigation]);
  return tokenRef.current;
}
