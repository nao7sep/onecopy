import { useSyncExternalStore } from "react";
import { displayZoneSnapshot, subscribeDisplayZone } from "../repositories/display-zone";

/** Re-render local date labels without remounting their controls or readers. */
export function useDisplayZone(): string {
  return useSyncExternalStore(subscribeDisplayZone, displayZoneSnapshot);
}
