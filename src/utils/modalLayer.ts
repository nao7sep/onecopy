import { isComposingEvent } from "../hooks/useComposing";
import { resolveFooterArrowTarget, resolveInitialFocus, resolveTrapTarget } from "./focusTrap";
import { isTopmostModal, popModal, pushModal } from "./modalStack";
import { acquireScrollLock, releaseScrollLock } from "./scrollLock";

/** DOM-owned mechanics shared by React dialogs and the independent recovery root. */
export function mountModalLayer(surface: HTMLElement, {
  token, opener = null, onClose, closeDisabled = () => false,
  footerArrowNavigation = false, returnFocus, recovery = false,
}: {
  token: object;
  opener?: HTMLElement | null;
  onClose: (event: KeyboardEvent) => void;
  closeDisabled?: () => boolean;
  footerArrowNavigation?: boolean;
  returnFocus?: () => HTMLElement | null;
  recovery?: boolean;
}): () => void {
  pushModal(token, surface, opener, recovery);
  acquireScrollLock();
  if (isTopmostModal(token) && !surface.contains(document.activeElement)) {
    resolveInitialFocus(surface).focus();
  }
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
      if (!event.repeat && !closeDisabled()) onClose(event);
    } else if (event.key === "Tab") {
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
    popModal(token, returnFocus?.())?.focus();
  };
}
