// Window sizing — the single source of truth for the layout's minimum
// dimensions. The window minimum is DERIVED from the pane minimums plus the
// fixed chrome, never hand-typed, so the window and its content can never
// disagree (app-chrome-conventions).
//
// The layout (App) is a vertical stack: a title band, then a content row of
// [sections sidebar | divider | grid | divider | right pane], then the status
// footer. Both side panes are user-resizable: the persisted value is the
// user's INTENT in pixels (written only on a drag), the displayed width is
// the intent clamped against what fits right now, and a window resize
// re-derives without ever rewriting the intent.

// Sections sidebar: default and minimum (month labels stop truncating
// usefully below the minimum).
export const SIDEBAR_DEFAULT_WIDTH = 256;
export const SIDEBAR_MIN_WIDTH = 180;

// Right pane: default and minimum (copy paths and the destination tree).
export const RIGHT_PANE_DEFAULT_WIDTH = 288;
export const RIGHT_PANE_MIN_WIDTH = 220;

// Grid (fill pane) minimum width: two 160px tiles plus gaps/padding — below
// this the grid is a single column and comparison entry points get cramped.
export const GRID_MIN_WIDTH = 420;

// The drag dividers between the panes.
//
// This is the HIT area, not the line. The line itself is 1px, drawn centred
// inside; at 4px the target was too small to catch reliably with a mouse,
// which reads as "the panes are not resizable" rather than as a near miss.
export const SPLITTER_WIDTH = 9;

// Content row minimum height before the grid's own scrolling takes over.
export const CONTENT_MIN_HEIGHT = 400;

// Title section height: the app name beside the menu trigger.
//
// It sits INSIDE the sidebar column, not across the window, so it does not
// enter the height sum below — a full-width band would have taxed every pane's
// height to host one button and a word, and only the sidebar has room to
// spare. It is still a real reservation within the sidebar, which is why the
// number lives here with the other minimums rather than inline in the markup.
export const HEADER_HEIGHT = 36;

// Status footer height (one text row: py-1 + text-xs ≈ 24px).
export const FOOTER_HEIGHT = 24;

export function computeMinWindowWidth(): number {
  return (
    SIDEBAR_MIN_WIDTH + SPLITTER_WIDTH + GRID_MIN_WIDTH + SPLITTER_WIDTH + RIGHT_PANE_MIN_WIDTH
  );
}

// The footer is the one full-width fixed band, reserved before the content row
// (app-chrome: fixed chrome is never the thing that gets clipped). The title
// section is inside the sidebar and is covered by the content row's own
// minimum, so it deliberately does not appear here.
export function computeMinWindowHeight(): number {
  return CONTENT_MIN_HEIGHT + FOOTER_HEIGHT;
}

/** Clamps the two pane INTENTS against the live container width. When both
 * fit beside the grid's minimum they pass through; when they do not, both
 * shrink toward their minimums proportionally to their headroom above them
 * (two adjustable panes, so a one-sided clamp cannot work). The intents are
 * never modified — only the display derives. */
export function clampPaneWidths(
  leftIntent: number,
  rightIntent: number,
  containerWidth: number,
): { left: number; right: number } {
  const left = Math.max(SIDEBAR_MIN_WIDTH, leftIntent);
  const right = Math.max(RIGHT_PANE_MIN_WIDTH, rightIntent);
  const available = containerWidth - GRID_MIN_WIDTH - 2 * SPLITTER_WIDTH;
  if (left + right <= available) {
    return { left, right };
  }
  const overflow = left + right - Math.max(available, SIDEBAR_MIN_WIDTH + RIGHT_PANE_MIN_WIDTH);
  const leftRoom = left - SIDEBAR_MIN_WIDTH;
  const rightRoom = right - RIGHT_PANE_MIN_WIDTH;
  const room = leftRoom + rightRoom;
  if (room <= 0) {
    return { left: SIDEBAR_MIN_WIDTH, right: RIGHT_PANE_MIN_WIDTH };
  }
  const shrinkLeft = Math.min(leftRoom, (overflow * leftRoom) / room);
  return {
    left: Math.max(SIDEBAR_MIN_WIDTH, Math.round(left - shrinkLeft)),
    right: Math.max(RIGHT_PANE_MIN_WIDTH, Math.round(right - (overflow - shrinkLeft))),
  };
}
