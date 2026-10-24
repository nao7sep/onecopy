import { useLayoutEffect, useRef, type RefObject } from "react";
import { mountModalLayer } from "../utils/modalLayer";

/** Shared behavior for both framed dialogs and full-window transient layers. */
export function useModalLayer(
  surfaceRef: RefObject<HTMLElement | null>,
  onClose: (event: KeyboardEvent) => void,
  closeDisabled = false,
  footerArrowNavigation = false,
  returnFocus?: () => HTMLElement | null,
  layerToken?: object,
): object {
  const tokenRef = useRef<object>(layerToken ?? {});
  // Capture before child autoFocus runs during the mount commit.
  const openerRef = useRef(document.activeElement as HTMLElement | null);
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
    const opener = openerRef.current;
    return mountModalLayer(surface, {
      token, opener,
      onClose: (event) => onCloseRef.current(event),
      closeDisabled: () => closeDisabledRef.current,
      footerArrowNavigation,
      returnFocus: () => returnFocusRef.current?.() ?? null,
    });
  }, [surfaceRef, footerArrowNavigation]);
  return tokenRef.current;
}
