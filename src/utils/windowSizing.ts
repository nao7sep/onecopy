// Window sizing — the single source of truth for the layout's minimum
// dimensions. The window minimum is DERIVED from the pane minimums plus the
// fixed chrome, never hand-typed, so the window and its content can never
// disagree (app-chrome-conventions).
//
// The layout (App) is a vertical stack: a content row of
// [sections sidebar | divider | grid | divider | (preview pane | divider)? |
// right pane] over the status footer — the preview joins the row side-by-side
// when open, because screens are wide. The utility panes persist pixel intent;
// the grid and Preview persist one ratio of their shared center area. Displayed
// widths derive from those intents and the live container, so a small window
// clamps without overwriting what a later large window should restore.

// Sections sidebar: default and minimum (month labels stop truncating
// usefully below the minimum).
export const SIDEBAR_DEFAULT_WIDTH = 256;
export const SIDEBAR_MIN_WIDTH = 180;

// Right pane: default and minimum (copy paths and the destination tree).
export const RIGHT_PANE_DEFAULT_WIDTH = 288;
export const RIGHT_PANE_MIN_WIDTH = 220;

// In-window preview pane (only when open): floored where it stops being a
// preview at all.
export const PREVIEW_PANE_MIN_WIDTH = 260;

// Equal peer intent. At a narrow window the unequal grid/Preview minimums can
// make the rendered split look unequal; maximizing restores this ratio because
// only a divider drag may change it.
export const PREVIEW_PANE_DEFAULT_RATIO = 0.5;

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
export const HEADER_HEIGHT = 48;

// Status footer height (one text row: py-1 + text-xs ≈ 24px).
export const FOOTER_HEIGHT = 24;

/** The preview pane, when open, is REAL reserved content like the others, so it
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

/** Derives rendered widths from durable layout intent.
 *
 * Left and right are fixed utility-pane pixel intents. They yield toward their
 * minimums, proportionally to headroom, only when the center pair needs room.
 * Preview receives its persisted share of the remaining grid/Preview peer
 * area, then both center panes enforce their own minimum. None of these live
 * clamps writes back to intent. `previewRatio` null means Preview is closed. */
export function derivePaneWidths(
  leftIntent: number,
  rightIntent: number,
  containerWidth: number,
  previewRatio: number | null = null,
): { left: number; right: number; preview: number } {
  const previewOpen = previewRatio !== null;
  const splitters = previewOpen ? 3 : 2;
  const centerMinimum = GRID_MIN_WIDTH + (previewOpen ? PREVIEW_PANE_MIN_WIDTH : 0);
  const utilityMinimum = SIDEBAR_MIN_WIDTH + RIGHT_PANE_MIN_WIDTH;
  const utilityAvailable = Math.max(
    utilityMinimum,
    containerWidth - splitters * SPLITTER_WIDTH - centerMinimum,
  );
  const wants = [
    Math.max(SIDEBAR_MIN_WIDTH, leftIntent),
    Math.max(RIGHT_PANE_MIN_WIDTH, rightIntent),
  ];
  const wanted = wants[0] + wants[1];
  let [left, right] = wants;
  if (wanted > utilityAvailable) {
    const rooms = [left - SIDEBAR_MIN_WIDTH, right - RIGHT_PANE_MIN_WIDTH];
    const room = rooms[0] + rooms[1];
    const overflow = Math.min(wanted - utilityAvailable, room);
    if (room > 0) {
      const leftShrink = Math.min(rooms[0], Math.round((overflow * rooms[0]) / room));
      const rightShrink = Math.min(rooms[1], overflow - leftShrink);
      left -= leftShrink;
      right -= rightShrink;
    }
  }
  if (!previewOpen) return { left, right, preview: 0 };

  const peerWidth = Math.max(
    GRID_MIN_WIDTH + PREVIEW_PANE_MIN_WIDTH,
    containerWidth - splitters * SPLITTER_WIDTH - left - right,
  );
  const ratio =
    Number.isFinite(previewRatio) && previewRatio > 0 && previewRatio < 1
      ? previewRatio
      : PREVIEW_PANE_DEFAULT_RATIO;
  const preview = Math.min(
    peerWidth - GRID_MIN_WIDTH,
    Math.max(PREVIEW_PANE_MIN_WIDTH, Math.round(peerWidth * ratio)),
  );
  return { left, right, preview };
}
