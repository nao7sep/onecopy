/** Stable backend identity for one logical library item. */
export interface DestinationItemIdentity {
  readonly hash: string | null;
  readonly pathId: number | null;
}

/**
 * One immutable Move/Copy intent.
 *
 * A drag may finish long before the user answers the Move/Copy question, and
 * the grid may refresh or change selection in between. The receiver must
 * therefore carry the exact logical items that began the interaction rather
 * than rereading whichever rows happen to be selected later.
 */
export interface DestinationSelection {
  readonly items: readonly DestinationItemIdentity[];
  readonly blockedNameCount: number;
  readonly anchorKey: string | null;
  readonly shownKeys: readonly string[];
}

export interface PendingDestinationDrop {
  readonly path: string;
  readonly selection: DestinationSelection;
}

/** Transient visual truth for the app-owned pointer gesture. It is never
 * operation input: the immutable DestinationSelection above remains the only
 * authority for Move/Copy. */
export interface DestinationDragPresentation {
  readonly clientX: number;
  readonly clientY: number;
  readonly sourceLabel: string;
  readonly itemCount: number;
  readonly thumbHash: string | null;
}
