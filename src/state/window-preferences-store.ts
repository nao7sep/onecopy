import { create } from "zustand";

export interface WindowPreferencesInput {
  enlargeSmallImages?: unknown;
  scoreFaces?: unknown;
  videoTranscriptionEnabled?: unknown;
  audioTranscriptionEnabled?: unknown;
}

interface WindowPreferencesState {
  enlargeSmallImages: boolean;
  scoreFaces: boolean;
  videoTranscriptionEnabled: boolean;
  audioTranscriptionEnabled: boolean;
  apply: (preferences: WindowPreferencesInput) => void;
}

// Auxiliary webviews do not run Main's one-shot application bootstrap. This
// small read model carries only presentation choices those webviews render,
// read from the core's effective configuration; the values before the first
// read are the core's defaults for a new installation.
export const useWindowPreferencesStore = create<WindowPreferencesState>((set) => ({
  enlargeSmallImages: true,
  scoreFaces: true,
  videoTranscriptionEnabled: true,
  audioTranscriptionEnabled: true,
  apply: (preferences) => set({
    enlargeSmallImages: preferences.enlargeSmallImages === true,
    scoreFaces: preferences.scoreFaces === true,
    videoTranscriptionEnabled: preferences.videoTranscriptionEnabled === true,
    audioTranscriptionEnabled: preferences.audioTranscriptionEnabled === true,
  }),
}));
