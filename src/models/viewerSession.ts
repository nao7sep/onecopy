export type ViewerPresentation = "quick" | "fullscreen";
export type ViewerSequenceScope = "section" | "selection";

import { identityKey, type ItemDetail, type SectionIdentity, type SectionItem, type SectionLocation, type SortChoice } from "./items";

export interface ViewerSequenceSnapshot {
  token: string;
  member: SectionIdentity;
  item: SectionItem;
  detail: ItemDetail;
  index: number;
  length: number;
  sectionIndex: number;
  scope: ViewerSequenceScope;
}

export interface ActiveViewerSession extends ViewerSequenceSnapshot {
  presentation: ViewerPresentation;
  main: ViewerMainRelationship;
}

export interface ViewerMainProjection {
  section: SectionLocation | null;
  sort: SortChoice;
  revision: number;
}

export interface ViewerMainRelationship {
  projection: ViewerMainProjection;
  selectedKeys: readonly string[];
  frozenPositionsValid: boolean;
}

/** Frozen ordinals are not positions in a replaced Main projection. */
export function viewerMainIndex(
  session: ActiveViewerSession,
  main: ViewerMainProjection & { loading: boolean; positions: ReadonlyMap<string, number> },
): number | null {
  const previous = session.main.projection;
  if (main.loading || main.section?.kind !== previous.section?.kind ||
    main.section?.month !== previous.section?.month || main.revision !== previous.revision ||
    main.sort.order !== previous.sort.order || main.sort.desc !== previous.sort.desc) return null;
  return main.positions.get(identityKey(session.member)) ??
    (session.main.frozenPositionsValid ? session.sectionIndex : null);
}

export type ViewerMove = "previous" | "next" | "first" | "last";
