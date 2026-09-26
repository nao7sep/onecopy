import { create } from "zustand";

export interface WindowPreferencesInput {
  enlargeSmallImagesInPreview?: unknown;
  enlargeSmallImagesInQuickView?: unknown;
  videoTranscriptionEnabled?: unknown;
  audioTranscriptionEnabled?: unknown;
}

interface WindowPreferencesState {
  enlargeSmallImagesInPreview: boolean;
  // Quick View and true fullscreen are one session (viewing-sessions.md) and
  // therefore share this one setting; without it, fullscreen — an auxiliary
  // webview with no app-config projection of its own — silently fell back to
  // the PREVIEW setting instead (content-presentation.md D3).
  enlargeSmallImagesInQuickView: boolean;
  videoTranscriptionEnabled: boolean;
  audioTranscriptionEnabled: boolean;
  apply: (preferences: WindowPreferencesInput) => void;
}

// Auxiliary webviews do not run Main's one-shot application bootstrap. This
// small read model carries only presentation choices those webviews render.
export const useWindowPreferencesStore = create<WindowPreferencesState>((set) => ({
  enlargeSmallImagesInPreview: true,
  enlargeSmallImagesInQuickView: true,
  videoTranscriptionEnabled: true,
  audioTranscriptionEnabled: true,
  apply: (preferences) => set({
    enlargeSmallImagesInPreview:
      preferences.enlargeSmallImagesInPreview !== false,
    enlargeSmallImagesInQuickView:
      preferences.enlargeSmallImagesInQuickView !== false,
    videoTranscriptionEnabled:
      preferences.videoTranscriptionEnabled !== false,
    audioTranscriptionEnabled:
      preferences.audioTranscriptionEnabled !== false,
  }),
}));
