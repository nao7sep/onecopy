import { create } from "zustand";
import type { Message } from "../i18n/translate";
import { identityKey } from "../models/items";
import type {
  ActiveViewerSession,
  ViewerSequenceSnapshot,
  ViewerMainRelationship,
  ViewerMainProjection,
} from "../models/viewerSession";

/** The exact member a fullscreen view deletion review names and a confirmed deletion
 * acts on, frozen when the review is requested. A refresh that advances the
 * sequence never retargets it. */
export interface FullscreenViewDeleteReview {
  kind: "trash" | "permanent";
  key: string;
  fileName: string;
}

interface FullscreenViewState {
  session: ActiveViewerSession | null;
  pendingDelete: FullscreenViewDeleteReview | null;
  failure: Message | null;
  currentKey: () => string | null;
  start: (snapshot: ViewerSequenceSnapshot, main: ViewerMainRelationship) => void;
  attachMainProjection: (projection: ViewerMainProjection) => void;
  update: (snapshot: ViewerSequenceSnapshot) => void;
  requestDelete: (kind: FullscreenViewDeleteReview["kind"]) => void;
  cancelDelete: () => void;
  setFailure: (failure: Message | null) => void;
  close: () => void;
}

// The fullscreen view's session lives in memory only and is never restored
// after a restart.
export const useFullscreenViewStore = create<FullscreenViewState>((set, get) => ({
  session: null,
  pendingDelete: null,
  failure: null,
  currentKey: () => {
    const session = get().session;
    return session === null ? null : identityKey(session.member);
  },
  start: (snapshot, main) => {
    set({ session: { ...snapshot, main }, pendingDelete: null, failure: null });
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
      set({ session: { ...snapshot, main: session.main } });
    }
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
