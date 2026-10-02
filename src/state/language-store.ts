import { create } from "zustand";
import {
  isLanguage,
  matchSystemLanguage,
  type Language,
} from "../i18n/languages";
import { loadCatalogue } from "../i18n/catalogues";

// If the appearance read never completes (a slow core, or window-appearance's
// own 5 s bound), the window must not paint in English regardless of what the
// computer's language actually is (D-L9) — the same fallback the Rust core
// would have resolved, from the browser's own preferred-locale list.
function browserSystemLanguage(): Language {
  const locales = typeof navigator === "undefined" ? [] : navigator.languages ?? [navigator.language];
  return matchSystemLanguage(locales.filter((locale): locale is string => typeof locale === "string"));
}

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
  /** Takes the language once its catalogue is loaded. */
  apply: (input: LanguageInput) => Promise<void>;
}

// Every window learns the language from the same appearance read it already
// awaits before its first paint, so no window shows English first, and a saved
// change reaches open windows through that read's invalidation.
export const useLanguageStore = create<LanguageState>((set) => {
  const fallback = browserSystemLanguage();
  return {
    language: fallback,
    systemLanguage: fallback,
    systemLocale: null,
    apply: async (input) => {
      const language = isLanguage(input.language) ? input.language : fallback;
      await loadCatalogue(language);
      set({
        language,
        systemLanguage: isLanguage(input.systemLanguage) ? input.systemLanguage : fallback,
        systemLocale: typeof input.systemLocale === "string" ? input.systemLocale : null,
      });
    },
  };
});
