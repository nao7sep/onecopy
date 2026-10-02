import en from "./locales/en.json";
import type { Language } from "./languages";

// English defines the key set; every other catalogue carries every key, with
// plural entries keyed by the language's own CLDR categories. The catalogue
// gate (tests/i18n/catalogues.test.ts) checks keys, placeholders, plural forms,
// untranslated English, and literal hidden characters.
export type MessageKey = keyof typeof en;

export type CatalogueEntry = string | Readonly<Record<string, string>>;

export type Catalogue = Readonly<Record<MessageKey, CatalogueEntry>>;

// One dynamic import per catalogue, so each language is its own chunk and
// only the interface language and English are loaded (localization-stack
// conventions). English is static: it is the fallback.
const LOADERS: Readonly<Record<Language, () => Promise<Catalogue>>> = {
  en: () => Promise.resolve(en),
  de: () => import("./locales/de.json").then((module) => module.default),
  es: () => import("./locales/es.json").then((module) => module.default),
  fr: () => import("./locales/fr.json").then((module) => module.default),
  it: () => import("./locales/it.json").then((module) => module.default),
  "pt-BR": () => import("./locales/pt-BR.json").then((module) => module.default),
  ru: () => import("./locales/ru.json").then((module) => module.default),
  ja: () => import("./locales/ja.json").then((module) => module.default),
  ko: () => import("./locales/ko.json").then((module) => module.default),
  "zh-Hans": () => import("./locales/zh-Hans.json").then((module) => module.default),
};

const loaded = new Map<Language, Catalogue>([["en", en]]);

/** Loads a language's catalogue once; a language change awaits this. */
export async function loadCatalogue(language: Language): Promise<Catalogue> {
  const known = loaded.get(language);
  if (known !== undefined) return known;
  const catalogue = await LOADERS[language]();
  loaded.set(language, catalogue);
  return catalogue;
}

/** The catalogue a translator reads: the language's once loaded, else English. */
export function catalogueFor(language: Language): Catalogue {
  return loaded.get(language) ?? en;
}
