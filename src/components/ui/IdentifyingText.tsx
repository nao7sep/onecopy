import { displayNativePath } from "../../utils/nativePath";

// One line of identifying text (interface-styling-conventions, "Labels"): a
// name, title, path or message that lets the user tell things apart, set in
// whatever width its container gives it and ended with one ellipsis when it
// does not fit. It never widens or heightens its container.
//
// A name keeps its beginning. A path keeps its final segment and loses its
// middle, because its beginning is usually shared: it is always set as its
// leading folders, which give way first and end with the marker, then the
// separator and final segment (`/Users/na…/IMG_0001.jpg`). CSS draws every
// marker and the DOM keeps the complete value (text-cleanup-conventions: only
// the displayed copy is shortened); the tooltip carries it for a pointer.

/** Where a path is cut: before its last separator, so the separator travels
 * with the final segment. -1 when there is no folder part to lose. */
export function finalSegmentStart(path: string): number {
  const trimmed = path.replace(/[/\\]+$/, "");
  const cut = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return cut > 0 ? cut : -1;
}

export default function IdentifyingText({
  value,
  path = false,
  title = value,
  className = "",
}: {
  value: string;
  /** Keep the final segment and lose the middle, instead of the end. */
  path?: boolean;
  /** The tooltip; the value itself unless the site names it otherwise. */
  title?: string;
  className?: string;
}) {
  const split = path ? finalSegmentStart(value) : -1;
  const displayTitle = displayNativePath(title);
  if (split < 0) {
    return <span title={displayTitle} className={`block min-w-0 truncate ${className}`}>{displayNativePath(value)}</span>;
  }
  return (
    <span title={displayTitle} className={`flex min-w-0 ${className}`}>
      <span className="min-w-0 truncate">{displayNativePath(value.slice(0, split))}</span>
      {/* Gives way only once the folders are gone, and is then cut at its
          own end rather than spilling past the line. */}
      <span className="max-w-full shrink-0 truncate">{displayNativePath(value.slice(split))}</span>
    </span>
  );
}
