export type ViewerPresentation = "quick" | "fullscreen";
export type ViewerSequenceScope = "section" | "selection";

import type { ItemDetail, SectionIdentity, SectionItem } from "./items";

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
}

export type ViewerMove = "previous" | "next" | "first" | "last";
