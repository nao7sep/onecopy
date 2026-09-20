import type { ReactNode } from "react";
import { useLanguageStore } from "../state/language-store";
import { I18nProvider } from "./I18nContext";
import { formattingLocale } from "./languages";

// Binds the language the core resolved to the translator every window uses.
// The store is filled by the appearance read each window awaits before its
// first paint, and refilled when a saved change invalidates it, so a language
// change reaches open windows without a reload.
export function InterfaceLanguage({ children }: { children: ReactNode }) {
  const language = useLanguageStore((s) => s.language);
  const systemLocale = useLanguageStore((s) => s.systemLocale);
  return (
    <I18nProvider language={language} locale={formattingLocale(language, systemLocale)}>
      {children}
    </I18nProvider>
  );
}
