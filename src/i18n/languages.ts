// The interface languages OneCopy ships, in the order the Settings picker
// lists them after System: Latin-script languages alphabetically by their own
// names, then Cyrillic, then CJK. Each tag names a catalogue in ./locales, and
// the Rust core embeds the same catalogues (src-tauri/src/i18n.rs), which
// resolves the computer's language to one of these tags.
export const LANGUAGES = ["en", "de", "es", "fr", "it", "pt-BR", "ru", "ja", "ko", "zh-Hans"] as const;

export type Language = (typeof LANGUAGES)[number];

// The saved choice. System follows the computer's language on every launch.
export type LanguagePreference = "system" | Language;

export function isLanguage(value: unknown): value is Language {
  return typeof value === "string" && (LANGUAGES as readonly string[]).includes(value);
}

// A missing, retired, or hand-edited value follows the computer — the same rule
// as the Rust core's normalize_preference, so both halves agree on the language.
export function normalizeLanguagePreference(value: unknown): LanguagePreference {
  return isLanguage(value) ? value : "system";
}

export function effectiveLanguage(preference: LanguagePreference, systemLanguage: Language): Language {
  return preference === "system" ? systemLanguage : preference;
}

// The same resolution the Rust core's match_locale/system_language apply, so
// a browser-side fallback (D-L9: the appearance read timing out or failing)
// agrees with what the core would have said, instead of defaulting to
// English. Every Chinese locale resolves to Simplified Chinese and every
// Portuguese one to Brazilian Portuguese.
function matchLocale(locale: string): Language | null {
  const primary = locale.split(/[-_.@]/)[0]?.toLowerCase() ?? "";
  if (primary === "zh") return "zh-Hans";
  if (primary === "pt") return "pt-BR";
  return (LANGUAGES as readonly string[]).includes(primary) ? (primary as Language) : null;
}

export function matchSystemLanguage(locales: readonly string[]): Language {
  for (const locale of locales) {
    const matched = matchLocale(locale);
    if (matched !== null) return matched;
  }
  return "en";
}

// Dates and numbers follow the computer's regional format when it is in the
// interface language (British English dates for an en-GB computer), and the
// interface language's own format otherwise.
export function formattingLocale(language: Language, systemLocale: string | null): string {
  if (systemLocale === null) {
    return language;
  }
  try {
    const system = new Intl.Locale(systemLocale).maximize();
    const target = new Intl.Locale(language).maximize();
    const sameLanguage = system.language === target.language && system.script === target.script;
    return sameLanguage && Intl.DateTimeFormat.supportedLocalesOf([systemLocale]).length > 0
      ? systemLocale
      : language;
  } catch {
    return language;
  }
}
