import { useEffect, useRef, type RefObject } from "react";

/** Report the actual card area's shape so rendering and arrow navigation use
 * one layout, including Main's headers and user-resized window. */
export function useComparisonLayout(
  area: RefObject<HTMLElement | null>,
  active: boolean,
  report: (aspect: number) => void,
): void {
  const reportRef = useRef(report);
  reportRef.current = report;
  useEffect(() => {
    const element = area.current;
    if (!active || element === null) return;
    const measure = () => {
      if (element.clientWidth > 0 && element.clientHeight > 0) {
        reportRef.current(element.clientWidth / element.clientHeight);
      }
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, [area, active]);
}
