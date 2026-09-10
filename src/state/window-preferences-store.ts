import { create } from "zustand";

export interface WindowPreferencesInput {
  enlargeSmallImagesInPreview?: unknown;
  videoTranscriptionEnabled?: unknown;
  audioTranscriptionEnabled?: unknown;
}

interface WindowPreferencesState {
  enlargeSmallImagesInPreview: boolean;
  videoTranscriptionEnabled: boolean;
  audioTranscriptionEnabled: boolean;
  apply: (preferences: WindowPreferencesInput) => void;
}

// Auxiliary webviews do not run Main's one-shot application bootstrap. This
// small read model carries only presentation choices those webviews render.
export const useWindowPreferencesStore = create<WindowPreferencesState>((set) => ({
  enlargeSmallImagesInPreview: true,
  videoTranscriptionEnabled: true,
  audioTranscriptionEnabled: true,
  apply: (preferences) => set({
    enlargeSmallImagesInPreview:
      preferences.enlargeSmallImagesInPreview !== false,
    videoTranscriptionEnabled:
      preferences.videoTranscriptionEnabled !== false,
    audioTranscriptionEnabled:
      preferences.audioTranscriptionEnabled !== false,
  }),
}));
