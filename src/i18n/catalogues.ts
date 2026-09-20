import de from "./locales/de.json";
import en from "./locales/en.json";
import es from "./locales/es.json";
import fr from "./locales/fr.json";
import it from "./locales/it.json";
import ja from "./locales/ja.json";
import ko from "./locales/ko.json";
import ptBR from "./locales/pt-BR.json";
import ru from "./locales/ru.json";
import zhHans from "./locales/zh-Hans.json";
import type { Language } from "./languages";

// English defines the key set; every other catalogue carries every key, with
// plural entries keyed by the language's own CLDR categories. The catalogue
// gate (tests/i18n/catalogues.test.ts) checks keys, placeholders, plural forms,
// untranslated English, and literal hidden characters.
export type MessageKey = keyof typeof en;

export type CatalogueEntry = string | Readonly<Record<string, string>>;

export type Catalogue = Readonly<Record<MessageKey, CatalogueEntry>>;

export const CATALOGUES: Readonly<Record<Language, Catalogue>> = {
  en,
  de,
  es,
  fr,
  it,
  "pt-BR": ptBR,
  ru,
  ja,
  ko,
  "zh-Hans": zhHans,
};
