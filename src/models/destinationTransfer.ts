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
  readonly anchorKey: string | null;
  readonly shownKeys: readonly string[];
}

export interface PendingDestinationDrop {
  readonly path: string;
  readonly selection: DestinationSelection;
}

export interface DestinationConflict {
  readonly path: string;
  readonly incomingBytes: number;
  readonly existingBytes: number | null;
  readonly withinSelection: boolean;
  readonly preservedPaths: readonly string[];
}

export interface PendingDestinationConflicts {
  readonly destDir: string;
  readonly mode: "move-trash-rest" | "move-delete-rest" | "copy";
  readonly selection: DestinationSelection;
  readonly planToken: string;
  readonly conflicts: readonly DestinationConflict[];
  readonly overwriteAllowed: boolean;
}
