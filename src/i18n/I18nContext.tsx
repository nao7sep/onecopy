import { createContext, useContext, useEffect, useMemo, type ReactNode } from "react";
import { isLanguage, type Language } from "./languages";
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
  // subtree should speak the previewed language, so <html lang> and every
  // surface outside that subtree (the last-resort error boundary, escaped
  // failures, this window's title) stay on the language actually in effect.
  manageDocumentLanguage = true,
}: {
  language: Language;
  locale: string;
  children: ReactNode;
  manageDocumentLanguage?: boolean;
}) {
  const translator = useMemo(() => createTranslator(language, locale), [language, locale]);

  // <html lang> picks the right glyphs for Chinese, Japanese and Korean text and
  // tells the last-resort error boundary, which sits outside this provider,
  // which language to speak.
  useEffect(() => {
    if (manageDocumentLanguage) document.documentElement.lang = language;
  }, [language, manageDocumentLanguage]);

  return <I18nContext.Provider value={translator}>{children}</I18nContext.Provider>;
}

export function useI18n(): Translator {
  return useContext(I18nContext);
}

// For surfaces outside the provider: the language the document last declared.
// Without a document — a store under test, or a worker — English, so reporting
// a failure never depends on having one.
export function documentTranslator(): Translator {
  const declared = typeof document === "undefined" ? "" : document.documentElement.lang;
  return createTranslator(isLanguage(declared) ? declared : "en");
}
