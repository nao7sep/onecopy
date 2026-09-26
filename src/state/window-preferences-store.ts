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
// small read model carries only presentation choices those webviews render,
// read from the core's effective configuration; the values before the first
// read are the core's defaults for a new installation.
export const useWindowPreferencesStore = create<WindowPreferencesState>((set) => ({
  enlargeSmallImagesInPreview: true,
  enlargeSmallImagesInQuickView: true,
  videoTranscriptionEnabled: true,
  audioTranscriptionEnabled: true,
  apply: (preferences) => set({
    enlargeSmallImagesInPreview: preferences.enlargeSmallImagesInPreview === true,
    enlargeSmallImagesInQuickView: preferences.enlargeSmallImagesInQuickView === true,
    videoTranscriptionEnabled: preferences.videoTranscriptionEnabled === true,
    audioTranscriptionEnabled: preferences.audioTranscriptionEnabled === true,
  }),
}));
