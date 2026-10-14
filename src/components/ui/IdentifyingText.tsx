// One line of identifying text (interface-styling-conventions, "Labels"): a
// name, title, path or message that lets the user tell things apart, set in
// whatever width its container gives it and ended with one ellipsis when it
// does not fit. It never widens or heightens its container.
//
// A name keeps its beginning. A path keeps its final segment whole and loses
// its middle, because its beginning is usually shared: once it does not fit,
// it renders as its leading folders cut at their end, then the separator and
// final segment (`/Users/na…/IMG_0001.jpg`). CSS draws every marker; the DOM
// keeps the complete value, so copying, assistive technology and type-ahead
// read it whole (text-cleanup-conventions: only the displayed copy is
// shortened).
//
// Where no detail surface already shows the value whole, a cut value becomes
// a button that opens it in a RevealPopover — Enter, a click or a tap. Text
// that fits stays plain text, out of the tab order. Sites whose selection
// detail already shows the value pass `reveal={false}`.

import { useCallback, useId, useLayoutEffect, useRef, useState } from "react";
import RevealPopover from "./RevealPopover";

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
  reveal = true,
  inline = false,
  className = "",
}: {
  value: string;
  /** Keep the final segment and lose the middle, instead of the end. */
  path?: boolean;
  /** False where the selection's detail surface already shows the value. */
  reveal?: boolean;
  /** Sits inside running text instead of filling a line of its own. */
  inline?: boolean;
  className?: string;
}) {
  const id = useId();
  const [cut, setCut] = useState(false);
  const [open, setOpen] = useState(false);
  const box = useRef<HTMLSpanElement | null>(null);
  const face = useRef<HTMLElement | null>(null);
  const head = useRef<HTMLSpanElement | null>(null);
  const tail = useRef<HTMLSpanElement | null>(null);
  const split = path && cut ? finalSegmentStart(value) : -1;

  // The text's own width against the line's, the same quantity whichever
  // way it is rendered: one run, or the cut path's two pieces. A pixel of
  // hysteresis keeps rounding from flipping a value at the very edge.
  const measure = useCallback(() => {
    const line = face.current;
    if (line === null) return;
    const natural = head.current !== null && tail.current !== null
      ? head.current.scrollWidth + tail.current.scrollWidth
      : line.scrollWidth;
    setCut((current) => natural > line.clientWidth - (current ? 1 : 0));
  }, []);

  // Measured after layout whenever the value or its rendering changes, and
  // again whenever the container resizes the line.
  useLayoutEffect(() => {
    measure();
    const element = box.current;
    if (element === null || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, [measure, value, cut, split]);

  // An open reveal follows a live value (a status line's progress) and
  // closes only once the value fits again.
  useLayoutEffect(() => {
    if (!cut) setOpen(false);
  }, [cut]);

  // The box takes the line its container gives; the face inside it fills
  // that line and carries the cut, as plain text or as the reveal button.
  const boxShape = inline
    ? "inline-block min-w-0 max-w-full align-bottom"
    : "block min-w-0";
  const faceShape = `${split >= 0 ? "flex" : "block truncate"} w-full`;
  const text = split >= 0 ? (
    <>
      <span ref={head} className="min-w-0 truncate">{value.slice(0, split)}</span>
      {/* Never shrinks before the folders are gone; only a final segment
          wider than the whole line is cut at its own end. */}
      <span ref={tail} className="max-w-full shrink-0 truncate">{value.slice(split)}</span>
    </>
  ) : value;
  const revealing = reveal && cut;

  return (
    <span ref={box} className={`${boxShape} ${className}`}>
      {revealing ? (
        <button
          ref={(element) => { face.current = element; }}
          id={id}
          type="button"
          aria-haspopup="dialog"
          aria-expanded={open}
          // Named by the value itself: a cut path's two pieces are separate
          // boxes, which a name computed from content would join with a space.
          aria-label={value}
          title={value}
          className={`${faceShape} rounded-sm [text-align:inherit]`}
          onClick={(event) => {
            // The value's own control: a row or card around it keeps its click.
            event.stopPropagation();
            setOpen((current) => !current);
          }}
          onDoubleClick={(event) => event.stopPropagation()}
        >
          {text}
        </button>
      ) : (
        <span ref={(element) => { face.current = element; }} className={faceShape}>
          {text}
        </span>
      )}
      {revealing && open && face.current !== null ? (
        <RevealPopover anchor={face.current} labelledBy={id} onClose={() => setOpen(false)}>
          <span className={path ? "break-all" : "break-words"}>{value}</span>
        </RevealPopover>
      ) : null}
    </span>
  );
}
