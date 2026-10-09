import { createContext, useContext, useEffect, useMemo, type ReactNode } from "react";
import { type Language } from "./languages";
import { useLanguageStore } from "../state/language-store";
import { createTranslator, type Translator } from "./translate";

// English until a provider says otherwise, so a component rendered on its own
// (in a test, say) still has text.
const I18nContext = createContext<Translator>(createTranslator("en"));

export function I18nProvider({
  language,
  locale,
  children,
  // False for a preview nested inside the window's own provider (the setup
  // wizard previewing a choice before Finish, R5.5 D-L3): only its own
  // subtree should speak the previewed language, so <html lang> stays on the
  // language actually in effect, as do the surfaces outside that subtree
  // (`documentTranslator` reads the language store).
  manageDocumentLanguage = true,
}: {
  language: Language;
  locale: string;
  children: ReactNode;
  manageDocumentLanguage?: boolean;
}) {
  const translator = useMemo(() => createTranslator(language, locale), [language, locale]);

  // <html lang> picks the right glyphs for Chinese, Japanese and Korean text.
  useEffect(() => {
    if (manageDocumentLanguage) document.documentElement.lang = language;
  }, [language, manageDocumentLanguage]);

  return <I18nContext.Provider value={translator}>{children}</I18nContext.Provider>;
}

export function useI18n(): Translator {
  return useContext(I18nContext);
}

// For surfaces outside the provider: the language actually in effect in this
// window, which the language store holds from the first appearance read on
// (before any provider has declared <html lang>). The wizard's preview
// provider never changes it.
export function documentTranslator(): Translator {
  return createTranslator(useLanguageStore.getState().language);
}
