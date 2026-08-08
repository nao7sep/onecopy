// Window sizing — the single source of truth for the layout's minimum
// dimensions. The window minimum is DERIVED from the pane minimums plus the
// fixed chrome, never hand-typed, so the window and its content can never
// disagree (app-chrome-conventions).
//
// The layout (App) is a vertical stack: a content row of
// [sections sidebar | grid | right pane] over the status footer.

// Sections sidebar fixed width (Tailwind w-64 = 16rem = 256px).
export const SIDEBAR_WIDTH = 256;

// Right pane fixed width (Tailwind w-72 = 18rem = 288px).
export const RIGHT_PANE_WIDTH = 288;

// Grid (fill pane) minimum width: two 160px tiles plus gaps/padding — below
// this the grid is a single column and comparison entry points get cramped.
export const GRID_MIN_WIDTH = 420;

// Content row minimum height before the grid's own scrolling takes over.
export const CONTENT_MIN_HEIGHT = 400;

// Status footer height (one text row: py-1 + text-xs ≈ 24px).
export const FOOTER_HEIGHT = 24;

export function computeMinWindowWidth(): number {
  return SIDEBAR_WIDTH + GRID_MIN_WIDTH + RIGHT_PANE_WIDTH;
}

export function computeMinWindowHeight(): number {
  return CONTENT_MIN_HEIGHT + FOOTER_HEIGHT;
}
