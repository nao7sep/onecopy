import type { RefObject } from "react";
import { useModalLayer } from "./useModalLayer";

// Root gates share focus containment and background isolation with dialogs,
// but cannot be dismissed: their own actions establish a usable main view.
export function useBlockingSurface(surfaceRef: RefObject<HTMLElement | null>): void {
  useModalLayer(surfaceRef, () => undefined, true);
}
