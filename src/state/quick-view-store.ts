import { create } from "zustand";
import type { Message } from "../i18n/translate";
import { identityKey } from "../models/items";
import type {
  ActiveViewerSession,
  ViewerPresentation,
  ViewerSequenceSnapshot,
  ViewerMainRelationship,
  ViewerMainProjection,
} from "../models/viewerSession";

/** The exact member a viewer deletion review names and a confirmed deletion
 * acts on, frozen when the review is requested. A refresh that advances the
 * sequence never retargets it. */
export interface ViewerDeleteReview {
  kind: "trash" | "permanent";
  key: string;
  fileName: string;
}

interface QuickViewState {
  session: ActiveViewerSession | null;
  pendingDelete: ViewerDeleteReview | null;
  failure: Message | null;
  currentKey: () => string | null;
  start: (snapshot: ViewerSequenceSnapshot, presentation: ViewerPresentation, main: ViewerMainRelationship) => void;
  attachMainProjection: (projection: ViewerMainProjection) => void;
  update: (snapshot: ViewerSequenceSnapshot) => void;
  setPresentation: (presentation: ViewerPresentation) => void;
  requestDelete: (kind: ViewerDeleteReview["kind"]) => void;
  cancelDelete: () => void;
  setFailure: (failure: Message | null) => void;
  close: () => void;
}

export const useQuickViewStore = create<QuickViewState>((set, get) => ({
  session: null,
  pendingDelete: null,
  failure: null,
  currentKey: () => {
    const session = get().session;
    return session === null ? null : identityKey(session.member);
  },
  start: (snapshot, presentation, main) => {
    set({ session: { ...snapshot, presentation, main }, pendingDelete: null, failure: null });
  },
  attachMainProjection: (projection) => {
    const session = get().session;
    if (session !== null) set({ session: { ...session, main: {
      ...session.main, projection, frozenPositionsValid: false,
    } } });
  },
  update: (snapshot) => {
    const session = get().session;
    if (session !== null && session.token === snapshot.token) {
      set({ session: { ...snapshot, presentation: session.presentation, main: session.main } });
    }
  },
  setPresentation: (presentation) => {
    const session = get().session;
    if (session !== null) set({ session: { ...session, presentation } });
  },
  requestDelete: (kind) => {
    const session = get().session;
    if (session === null) return;
    set({
      pendingDelete: {
        kind,
        key: identityKey(session.member),
        fileName: session.item.fileName,
      },
    });
  },
  cancelDelete: () => set({ pendingDelete: null }),
  setFailure: (failure) => set({ failure }),
  close: () => set({ session: null, pendingDelete: null, failure: null }),
}));
