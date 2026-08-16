// Window sizing — the single source of truth for the layout's minimum
// dimensions. The window minimum is DERIVED from the pane minimums plus the
// fixed chrome, never hand-typed, so the window and its content can never
// disagree (app-chrome-conventions).
//
// The layout (App) is a vertical stack: a content row of
// [sections sidebar | divider | grid | divider | (preview pane | divider)? |
// right pane] over the status footer — the preview joins the row side-by-side
// when open, because screens are wide. Every adjustable pane persists the
// user's INTENT in pixels (written only on a drag); the displayed width is
// the intent clamped against what fits right now, and a window resize
// re-derives without ever rewriting the intent.

// Sections sidebar: default and minimum (month labels stop truncating
// usefully below the minimum).
export const SIDEBAR_DEFAULT_WIDTH = 256;
export const SIDEBAR_MIN_WIDTH = 180;

// Right pane: default and minimum (copy paths and the destination tree).
export const RIGHT_PANE_DEFAULT_WIDTH = 288;
export const RIGHT_PANE_MIN_WIDTH = 220;

// In-window preview pane (only when open): wide enough by default that a
// landscape photo reads, floored where it stops being a preview at all.
export const PREVIEW_PANE_DEFAULT_WIDTH = 480;
export const PREVIEW_PANE_MIN_WIDTH = 260;

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

/** The preview pane, when open, is REAL fixed content like the others, so it
 * raises the window minimum for exactly as long as it is on screen — App
 * re-applies setMinSize when it opens or closes. */
export function computeMinWindowWidth(previewOpen = false): number {
  return (
    SIDEBAR_MIN_WIDTH +
    SPLITTER_WIDTH +
    GRID_MIN_WIDTH +
    SPLITTER_WIDTH +
    RIGHT_PANE_MIN_WIDTH +
    (previewOpen ? PREVIEW_PANE_MIN_WIDTH + SPLITTER_WIDTH : 0)
  );
}

// The footer is the one full-width fixed band, reserved before the content row
// (app-chrome: fixed chrome is never the thing that gets clipped). The title
// section is inside the sidebar and is covered by the content row's own
// minimum, so it deliberately does not appear here.
export function computeMinWindowHeight(): number {
  return CONTENT_MIN_HEIGHT + FOOTER_HEIGHT;
}

/** Clamps the adjustable panes' INTENTS against the live container width.
 * When everything fits beside the grid's minimum the intents pass through;
 * when not, every pane shrinks toward its minimum proportionally to its
 * headroom above it (several adjustable panes, so a one-sided clamp cannot
 * work). The intents are never modified — only the display derives.
 * `previewIntent` null means the preview pane is closed and takes nothing. */
export function clampPaneWidths(
  leftIntent: number,
  rightIntent: number,
  containerWidth: number,
  previewIntent: number | null = null,
): { left: number; right: number; preview: number } {
  const mins = [
    SIDEBAR_MIN_WIDTH,
    RIGHT_PANE_MIN_WIDTH,
    ...(previewIntent !== null ? [PREVIEW_PANE_MIN_WIDTH] : []),
  ];
  const wants = [
    Math.max(SIDEBAR_MIN_WIDTH, leftIntent),
    Math.max(RIGHT_PANE_MIN_WIDTH, rightIntent),
    ...(previewIntent !== null ? [Math.max(PREVIEW_PANE_MIN_WIDTH, previewIntent)] : []),
  ];
  const splitters = previewIntent !== null ? 3 : 2;
  const available = containerWidth - GRID_MIN_WIDTH - splitters * SPLITTER_WIDTH;
  const wanted = wants.reduce((a, b) => a + b, 0);
  const result = (values: number[]) => ({
    left: values[0],
    right: values[1],
    preview: values[2] ?? 0,
  });
  if (wanted <= available) {
    return result(wants);
  }
  const overflow = wanted - Math.max(available, mins.reduce((a, b) => a + b, 0));
  const rooms = wants.map((w, i) => w - mins[i]);
  const room = rooms.reduce((a, b) => a + b, 0);
  if (room <= 0) {
    return result(mins);
  }
  // Proportional-to-headroom, sequential so rounding never busts a minimum.
  let remaining = overflow;
  const values = wants.map((want, i) => {
    const shrink = Math.min(rooms[i], Math.round((overflow * rooms[i]) / room));
    const taken = Math.min(shrink, remaining);
    remaining -= taken;
    return Math.max(mins[i], want - taken);
  });
  return result(values);
}
