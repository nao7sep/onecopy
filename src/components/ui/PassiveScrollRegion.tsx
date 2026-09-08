import { useRef } from "react";

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
  const ref = useRef<HTMLDivElement>(null);
  return (
    <div
      ref={ref}
      role="region"
      aria-label={label}
      tabIndex={0}
      className={`overflow-auto rounded-md focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-primary-ring ${className}`}
      onScroll={onScroll}
      onKeyDown={(event) => {
        if (event.target !== event.currentTarget) return;
        const target = ref.current;
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
      }}
    >
      {children}
    </div>
  );
}
