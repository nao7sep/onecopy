import { create } from "zustand";
import {
  isLanguage,
  type Language,
} from "../i18n/languages";

export interface LanguageInput {
  language?: unknown;
  systemLanguage?: unknown;
  systemLocale?: unknown;
}

interface LanguageState {
  /** The language this window speaks, as the core resolved it. */
  language: Language;
  /** The computer's language, for the Settings choice that follows it. */
  systemLanguage: Language;
  /** The computer's first preferred locale, for regional date and number
   * formats when it shares the interface language. */
  systemLocale: string | null;
  apply: (input: LanguageInput) => void;
}

// Every window learns the language from the same appearance read it already
// awaits before its first paint, so no window shows English first, and a saved
// change reaches open windows through that read's invalidation.
export const useLanguageStore = create<LanguageState>((set) => ({
  language: "en",
  systemLanguage: "en",
  systemLocale: null,
  apply: (input) => set({
    language: isLanguage(input.language) ? input.language : "en",
    systemLanguage: isLanguage(input.systemLanguage) ? input.systemLanguage : "en",
    systemLocale: typeof input.systemLocale === "string" ? input.systemLocale : null,
  }),
}));
