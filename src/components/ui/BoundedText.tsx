// Text to be read, inside a height limit that scrolls (interface-styling-
// conventions, "Text to be read is never shortened"). The caller gives the
// limit (a `max-h-*` class); this keeps the overflow reachable by keyboard.
//
// macOS WebKit does not make a scrolling box focusable on its own, so while
// the text overflows the box becomes a named, focusable region whose arrow
// and page keys scroll it; App.css's covering rule draws no ring for it, and
// the brightened thumb is its focus cue. While everything fits it stays plain
// text and adds no tab stop. Measured after every render (the text may change
// while the box is already at its limit) and whenever the box resizes.

import { useCallback, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { useI18n } from "../../i18n/I18nContext";

export default function BoundedText({
  className = "",
  children,
}: {
  /** Must carry the height limit, such as `max-h-32`. */
  className?: string;
  children: ReactNode;
}) {
  const { t } = useI18n();
  const box = useRef<HTMLDivElement>(null);
  const [overflowing, setOverflowing] = useState(false);

  const measure = useCallback(() => {
    const element = box.current;
    if (element !== null) setOverflowing(element.scrollHeight > element.clientHeight);
  }, []);

  useLayoutEffect(measure);

  useLayoutEffect(() => {
    const element = box.current;
    if (element === null || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, [measure]);

  const region = overflowing
    ? { role: "region", tabIndex: 0, "aria-label": t("common.fullText") }
    : {};
  return (
    <div ref={box} {...region} className={`overflow-y-auto ${className}`}>
      {children}
    </div>
  );
}
