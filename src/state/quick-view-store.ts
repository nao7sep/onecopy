import { create } from "zustand";
import {
  moveViewerSession,
  reconcileViewerSession,
  viewerCurrentKey,
  type ViewerMove,
  type ViewerMember,
  type ViewerSession,
} from "../models/viewerSession";

interface QuickViewState {
  session: ViewerSession | null;
  pendingDelete: "trash" | "permanent" | null;
  currentKey: () => string | null;
  start: (session: ViewerSession) => void;
  move: (move: ViewerMove) => void;
  reconcile: (liveMembers: ViewerMember[]) => void;
  setPresentation: (presentation: ViewerSession["presentation"]) => void;
  requestDelete: (kind: "trash" | "permanent") => void;
  cancelDelete: () => void;
  close: () => void;
}

export const useQuickViewStore = create<QuickViewState>((set, get) => ({
  session: null,
  pendingDelete: null,
  currentKey: () => viewerCurrentKey(get().session),
  start: (session) => set({ session, pendingDelete: null }),
  move: (move) => {
    const session = get().session;
    if (session !== null) set({ session: moveViewerSession(session, move) });
  },
  reconcile: (liveMembers) => {
    const session = get().session;
    if (session !== null) {
      set({ session: reconcileViewerSession(session, liveMembers), pendingDelete: null });
    }
  },
  setPresentation: (presentation) => {
    const session = get().session;
    if (session !== null) set({ session: { ...session, presentation } });
  },
  requestDelete: (pendingDelete) => set({ pendingDelete }),
  cancelDelete: () => set({ pendingDelete: null }),
  close: () => set({ session: null, pendingDelete: null }),
}));
