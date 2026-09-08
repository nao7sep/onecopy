import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";

export const SCROLLBAR_HIDE_DELAY_MS = 2_000;
export const SCROLLBAR_PROXIMITY_PX = 28;

const TRACK_INSET_PX = 4;
const MIN_THUMB_HEIGHT_PX = 32;

export interface ScrollbarGeometry {
  overflow: boolean;
  thumbHeight: number;
  thumbTop: number;
  travel: number;
}

export function scrollbarGeometry(
  scrollTop: number,
  scrollHeight: number,
  clientHeight: number,
): ScrollbarGeometry {
  const trackHeight = Math.max(0, clientHeight - TRACK_INSET_PX * 2);
  const maxScroll = Math.max(0, scrollHeight - clientHeight);
  if (maxScroll === 0 || trackHeight === 0) {
    return { overflow: false, thumbHeight: trackHeight, thumbTop: TRACK_INSET_PX, travel: 0 };
  }
  const thumbHeight = Math.min(
    trackHeight,
    Math.max(MIN_THUMB_HEIGHT_PX, (trackHeight * clientHeight) / scrollHeight),
  );
  const travel = Math.max(0, trackHeight - thumbHeight);
  return {
    overflow: true,
    thumbHeight,
    thumbTop: TRACK_INSET_PX + (travel * Math.min(maxScroll, Math.max(0, scrollTop))) / maxScroll,
    travel,
  };
}

export function passiveScrollKey(
  key: string,
  shiftKey: boolean,
  viewportHeight: number,
): number | "start" | "end" | null {
  switch (key) {
    case "ArrowUp":
      return -40;
    case "ArrowDown":
      return 40;
    case "PageUp":
      return -Math.max(40, Math.floor(viewportHeight * 0.9));
    case "PageDown":
      return Math.max(40, Math.floor(viewportHeight * 0.9));
    case "Home":
      return "start";
    case "End":
      return "end";
    case " ":
      return (shiftKey ? -1 : 1) * Math.max(40, Math.floor(viewportHeight * 0.9));
    default:
      return null;
  }
}

export default function PassiveScrollRegion({
  label,
  className = "",
  onScroll,
  children,
}: {
  label: string;
  className?: string;
  onScroll?: React.UIEventHandler<HTMLDivElement>;
  children: React.ReactNode;
}) {
  const viewportRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const hideTimerRef = useRef<number | null>(null);
  const engagementRef = useRef({ focused: false, near: false });
  const dragRef = useRef<{
    pointerId: number;
    startY: number;
    startScrollTop: number;
    maxScroll: number;
    travel: number;
  } | null>(null);
  const [geometry, setGeometry] = useState<ScrollbarGeometry>(() =>
    scrollbarGeometry(0, 0, 0),
  );
  const [visible, setVisible] = useState(false);

  const cancelHide = useCallback(() => {
    if (hideTimerRef.current !== null) {
      window.clearTimeout(hideTimerRef.current);
      hideTimerRef.current = null;
    }
  }, []);

  const show = useCallback(() => {
    cancelHide();
    setVisible(true);
  }, [cancelHide]);

  const scheduleHide = useCallback(() => {
    cancelHide();
    hideTimerRef.current = window.setTimeout(() => {
      hideTimerRef.current = null;
      if (
        engagementRef.current.focused
        || engagementRef.current.near
        || dragRef.current !== null
      ) return;
      setVisible(false);
    }, SCROLLBAR_HIDE_DELAY_MS);
  }, [cancelHide]);

  const refreshGeometry = useCallback(() => {
    const viewport = viewportRef.current;
    if (viewport === null) return;
    setGeometry(
      scrollbarGeometry(
        viewport.scrollTop,
        viewport.scrollHeight,
        viewport.clientHeight,
      ),
    );
  }, []);

  useLayoutEffect(() => {
    refreshGeometry();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(refreshGeometry);
    if (viewportRef.current !== null) observer.observe(viewportRef.current);
    if (contentRef.current !== null) observer.observe(contentRef.current);
    return () => observer.disconnect();
  }, [refreshGeometry]);

  useEffect(() => () => cancelHide(), [cancelHide]);

  const revealTemporarily = useCallback(() => {
    show();
    scheduleHide();
  }, [scheduleHide, show]);

  const releaseDrag = useCallback((event: React.PointerEvent<HTMLDivElement>) => {
    if (dragRef.current?.pointerId !== event.pointerId) return;
    dragRef.current = null;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    if (engagementRef.current.focused || engagementRef.current.near) show();
    else revealTemporarily();
  }, [revealTemporarily, show]);

  return (
    <div
      className={`passive-scroll-shell ${className}`}
      data-scrollbar-visible={visible && geometry.overflow ? "true" : "false"}
      onFocusCapture={() => {
        engagementRef.current.focused = true;
        show();
      }}
      onBlurCapture={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null)) {
          engagementRef.current.focused = false;
          scheduleHide();
        }
      }}
      onPointerMove={(event) => {
        if (dragRef.current !== null) return;
        const distanceFromEnd = event.currentTarget.getBoundingClientRect().right - event.clientX;
        engagementRef.current.near = distanceFromEnd >= 0
          && distanceFromEnd <= SCROLLBAR_PROXIMITY_PX;
        if (engagementRef.current.near) show();
        else scheduleHide();
      }}
      onPointerLeave={() => {
        engagementRef.current.near = false;
        scheduleHide();
      }}
    >
      <div
        ref={viewportRef}
        role="region"
        aria-label={label}
        tabIndex={0}
        className="passive-scroll-viewport rounded-md focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-primary-ring"
        onScroll={(event) => {
          refreshGeometry();
          revealTemporarily();
          onScroll?.(event);
        }}
        onKeyDown={(event) => {
          if (event.target !== event.currentTarget) return;
          const target = viewportRef.current;
          if (target === null) return;
          const action = passiveScrollKey(
            event.key,
            event.shiftKey,
            target.clientHeight,
          );
          if (action === null) return;
          event.preventDefault();
          event.stopPropagation();
          if (action === "start") target.scrollTo({ top: 0 });
          else if (action === "end") target.scrollTo({ top: target.scrollHeight });
          else target.scrollBy({ top: action });
          revealTemporarily();
        }}
      >
        <div ref={contentRef} className="passive-scroll-content">
          {children}
        </div>
      </div>
      {geometry.overflow ? (
        <div
          className="passive-scroll-track"
          aria-hidden="true"
          onPointerDown={(event) => {
            if (event.target !== event.currentTarget) return;
            const viewport = viewportRef.current;
            if (viewport === null) return;
            const track = event.currentTarget.getBoundingClientRect();
            const targetTop = event.clientY - track.top - geometry.thumbHeight / 2;
            const ratio = geometry.travel === 0
              ? 0
              : Math.min(1, Math.max(0, targetTop / geometry.travel));
            viewport.scrollTop = ratio * (viewport.scrollHeight - viewport.clientHeight);
            refreshGeometry();
            revealTemporarily();
          }}
        >
          <div
            className="passive-scroll-thumb"
            style={{
              height: `${geometry.thumbHeight}px`,
              transform: `translateY(${geometry.thumbTop}px)`,
            }}
            onPointerDown={(event) => {
              const viewport = viewportRef.current;
              if (viewport === null) return;
              event.preventDefault();
              dragRef.current = {
                pointerId: event.pointerId,
                startY: event.clientY,
                startScrollTop: viewport.scrollTop,
                maxScroll: Math.max(0, viewport.scrollHeight - viewport.clientHeight),
                travel: geometry.travel,
              };
              event.currentTarget.setPointerCapture(event.pointerId);
              show();
            }}
            onPointerMove={(event) => {
              const drag = dragRef.current;
              const viewport = viewportRef.current;
              if (drag === null || viewport === null || drag.pointerId !== event.pointerId) return;
              const delta = drag.travel === 0
                ? 0
                : ((event.clientY - drag.startY) * drag.maxScroll) / drag.travel;
              viewport.scrollTop = Math.min(
                drag.maxScroll,
                Math.max(0, drag.startScrollTop + delta),
              );
              refreshGeometry();
            }}
            onPointerUp={releaseDrag}
            onPointerCancel={releaseDrag}
          />
        </div>
      ) : null}
    </div>
  );
}
