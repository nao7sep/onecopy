import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { loadCatalogue } from "../../src/i18n/catalogues";
import { LANGUAGES, LANGUAGE_NAMES, type Language } from "../../src/i18n/languages";
import { createTranslator } from "../../src/i18n/translate";

// The catalogue gate. English defines the key set; every other language must
// carry every key, keep every placeholder, supply exactly its own CLDR plural
// forms, and never leave English standing in for a translation. A string that
// is genuinely the same in a language is listed below and checked both ways.

type Entry = string | Record<string, string>;
type Catalogue = Record<string, Entry>;

// Read through the app's own imports, so a change to any catalogue selects
// this gate as a related test.
const catalogues = Object.fromEntries(
  await Promise.all(LANGUAGES.map(async (language) => [language, await loadCatalogue(language)])),
) as unknown as Record<Language, Catalogue>;

const english = catalogues.en;
const translations = LANGUAGES.filter((language) => language !== "en");

// Keys whose text is the same word in that language as in English.
const SAME_AS_ENGLISH: Partial<Record<Language, readonly string[]>> = {
  de: [
    "settings.groupingNormal",
    "app.zoom",
    "app.detailsTab",
    "settings.languageSystem",
    "settings.timezoneSystem",
    "settings.audio",
    "settings.videos",
    "section.videos",
    "quarantine.ok",
    "binaries.build",
    "binaries.title",
    "binaries.downloadSize",
    "about.version",
    "common.name",
    "metadata.name",
    "shortcuts.groupApp",
    "item.transcriptBadge",
    "tool.ffmpeg",
    "acceleration.modeMetal",
    "activity.durationMs",
    "records.levelInfo",
    "records.levelDebug",
    "records.details",
  ],
  es: [
    "settings.groupingNormal",
    "app.zoom",
    "nativeMenu.zoom",
    "settings.audio",
    "settings.videos",
    "section.videos",
    "status.videoCount",
    "item.transcriptBadge",
    "metadata.seconds",
    "tool.ffmpeg",
    "acceleration.modeMetal",
    "activity.durationMs",
    "records.levelError",
  ],
  fr: [
    "settings.groupingNormal",
    "deletedFiles.copies",
    "nativeMenu.services",
    "app.zoom",
    "app.destinationsTab",
    "destinations.title",
    "quarantine.ok",
    "settings.audio",
    "settings.notifications",
    "binaries.build",
    "about.version",
    "preview.copyCount",
    "metadata.date",
    "metadata.dimensions",
    "metadata.copies",
    "sidebar.sections",
    "common.date",
    "textPreview.copies",
    "status.imageCount",
    "scan.sourceProgress",
    "section.images",
    "activity.actionTranscript",
    "shortcuts.groupConfirmations",
    "tool.ffmpeg",
    "acceleration.transcription",
    "acceleration.modeMetal",
    "item.transcriptBadge",
    "activity.durationMs",
    "records.kindNotice",
    "records.message",
    "records.action",
  ],
  it: [
    "app.zoom",
    "nativeMenu.file",
    "quarantine.ok",
    "settings.audio",
    "binaries.build",
    "trash.rootSummary",
    "status.videoCount",
    "item.file",
    "item.transcriptBadge",
    "mutation.factFiles",
    "metadata.seconds",
    "tool.ffmpeg",
    "acceleration.modeMetal",
    "activity.durationMs",
    "records.levelInfo",
    "records.levelDebug",
  ],
  "pt-BR": [
    "settings.groupingNormal",
    "app.zoom",
    "nativeMenu.zoom",
    "quarantine.ok",
    "mutation.factItems",
    "metadata.seconds",
    "tool.ffmpeg",
    "acceleration.modeMetal",
    "activity.durationMs",
  ],
  ru: ["tool.ffmpeg", "acceleration.modeMetal", "activity.durationMs"],
  ja: ["quarantine.ok", "tool.ffmpeg", "acceleration.modeMetal", "activity.durationMs"],
  ko: ["tool.ffmpeg", "acceleration.modeMetal", "activity.durationMs"],
  "zh-Hans": ["tool.ffmpeg", "acceleration.modeMetal", "activity.durationMs"],
};

// The hidden-character-conventions set, plus the no-break spaces, figure space,
// word joiner and soft hyphen that look like ordinary text in a diff.
const LITERAL_HIDDEN =
  /[\u0000-\u0008\u000B\u000C\u000E-\u001F\u007F\u00A0\u00AD\u2007\u200B-\u200F\u2028\u2029\u202A-\u202F\u2060\u2066-\u2069\uFEFF]/gu;

// Each control that shows a running status in place of its command, as
// [command, status] keys.
const COMMAND_STATUS_PAIRS: readonly (readonly [string, string])[] = [
  ["app.checkSources", "app.checkingSources"],
  ["app.cancelOperation", "common.cancelling"],
  ["app.stopCheck", "app.stopping"],
  ["about.checkRelease", "about.checking"],
  ["binaries.checkForUpdates", "binaries.checking"],
  ["common.cancel", "common.cancelling"],
  ["issues.showMore", "issues.loadingMore"],
  ["settings.identifyScreens", "settings.identifyingScreens"],
  ["settings.rebuildRequest", "settings.rebuilding"],
  ["settings.save", "common.saving"],
  ["wizard.finish", "wizard.finishing"],
  ["work.pause", "common.saving"],
  ["work.resume", "common.saving"],
];

// Languages whose typography puts a no-break space before an ellipsis.
const SPACED_ELLIPSIS: readonly Language[] = ["de"];

function forms(entry: Entry): string[] {
  return typeof entry === "string" ? [entry] : Object.values(entry);
}

function placeholders(text: string): string[] {
  return [...text.matchAll(/\{(\w+)\}/g)].map((match) => match[1]).sort();
}

function hasWords(text: string): boolean {
  return /\p{L}/u.test(text.replace(/\{\w+\}/g, ""));
}

describe("catalogues", () => {
  it("has one catalogue per language and no others", () => {
    const files = readdirSync(join(process.cwd(), "src/i18n/locales")).filter((name) => name.endsWith(".json"));
    expect(files.map((name) => name.replace(/\.json$/, "")).sort()).toEqual([...LANGUAGES].sort());
  });

  it.each(translations)("%s has exactly the English keys", (language) => {
    const keys = Object.keys(catalogues[language]);
    const englishKeys = Object.keys(english);
    expect(englishKeys.filter((key) => !keys.includes(key)), "missing").toEqual([]);
    expect(keys.filter((key) => !englishKeys.includes(key)), "extra").toEqual([]);
  });

  it.each(LANGUAGES)("%s entries are non-empty, trimmed text", (language) => {
    for (const [key, entry] of Object.entries(catalogues[language])) {
      for (const text of forms(entry)) {
        expect(text.length, key).toBeGreaterThan(0);
        expect(text, key).toBe(text.trim());
      }
    }
  });

  it.each(LANGUAGES)("%s plural entries use exactly the language's CLDR categories", (language) => {
    const categories = new Intl.PluralRules(language).resolvedOptions().pluralCategories;
    for (const [key, entry] of Object.entries(catalogues[language])) {
      const englishEntry = english[key];
      if (englishEntry === undefined) continue;
      expect(typeof entry, key).toBe(typeof englishEntry);
      if (typeof entry !== "string") {
        expect(Object.keys(entry).sort(), key).toEqual([...categories].sort());
      }
    }
  });

  it.each(translations)("%s keeps every placeholder", (language) => {
    for (const [key, englishEntry] of Object.entries(english)) {
      const expected = placeholders(typeof englishEntry === "string" ? englishEntry : englishEntry.other);
      for (const text of forms(catalogues[language][key] ?? "")) {
        expect(placeholders(text), `${key}: ${text}`).toEqual(expected);
      }
    }
  });

  it.each(translations)("%s translates everything not listed as the same word", (language) => {
    const allowed = SAME_AS_ENGLISH[language] ?? [];
    const copied = Object.keys(english).filter((key) => {
      const englishForms = forms(english[key]);
      const theirs = forms(catalogues[language][key] ?? "");
      return theirs.some((text) => hasWords(text) && englishForms.includes(text));
    });
    expect(copied.filter((key) => !allowed.includes(key)), "untranslated").toEqual([]);
    expect(allowed.filter((key) => !copied.includes(key)), "listed but translated").toEqual([]);
  });

  // Parsing turns a `\u00a0` escape into the same character as a literal one,
  // so this reads the files themselves: every hidden, no-break or soft-hyphen
  // character must be written as an escape (localization-conventions,
  // hidden-character-conventions).
  it.each(LANGUAGES)("%s writes every hidden or no-break character as an escape", (language) => {
    const source = readFileSync(join(process.cwd(), `src/i18n/locales/${language}.json`), "utf8");
    const literal = source.split("\n").flatMap((line, index) =>
      [...line.matchAll(LITERAL_HIDDEN)].map(
        (match) => `line ${index + 1}: U+${match[0].codePointAt(0)!.toString(16).toUpperCase().padStart(4, "0")}`,
      ),
    );
    expect(literal).toEqual([]);
  });

  // localization-conventions, Ellipsis: one U+2026, never three periods, and
  // a translation carries it exactly when English does. German spaces it with
  // a no-break space, as Duden does; every other language attaches it.
  it.each(LANGUAGES)("%s writes each ellipsis as one character with its language's spacing", (language) => {
    const spaced = SPACED_ELLIPSIS.includes(language);
    for (const [key, entry] of Object.entries(catalogues[language])) {
      for (const text of forms(entry)) {
        expect(text.includes("..."), `${key}: ${text}`).toBe(false);
        for (const match of text.matchAll(/(.)?…/gu)) {
          const before = match[1] ?? "";
          if (spaced) expect(before, `${key}: ${text}`).toBe("\u00a0");
          else expect(/\s/u.test(before), `${key}: ${text}`).toBe(false);
        }
      }
    }
  });

  it.each(translations)("%s has an ellipsis exactly where English does", (language) => {
    const mismatched = Object.keys(english).filter((key) => {
      const expected = forms(english[key]).some((text) => text.includes("…"));
      return forms(catalogues[language][key] ?? "").some((text) => text.includes("…") !== expected);
    });
    expect(mismatched).toEqual([]);
  });

  // A control that swaps its command for a running status must read
  // differently in every language, or a press looks ignored
  // (localization-conventions, Ellipsis). A clash rewords the command.
  it.each(LANGUAGES)("%s tells each command from the status that replaces it", (language) => {
    for (const key of COMMAND_STATUS_PAIRS.flat()) expect(english[key], key).toBeDefined();
    const clashes = COMMAND_STATUS_PAIRS.filter(([command, status]) => {
      const statuses = forms(catalogues[language][status] ?? "");
      return forms(catalogues[language][command] ?? "").some((text) => statuses.includes(text));
    });
    expect(clashes).toEqual([]);
  });

  it("names every language differently, in its own words", () => {
    const names = LANGUAGES.map((language) => LANGUAGE_NAMES[language]);
    expect(new Set(names).size).toBe(LANGUAGES.length);
  });

  // Every control that opens About must read exactly like the macOS About
  // item (app-chrome-conventions, App identity): the in-app menu item
  // (`app.about`) and the dialog's own title (`about.title`) are the same
  // words as the native menu's `nativeMenu.about`, with the app name filled
  // in, and never carry a trailing ellipsis the native item does not have.
  it.each(LANGUAGES)("%s's About controls read exactly like the macOS About item", (language) => {
    const expected = createTranslator(language).t("nativeMenu.about", { app: "OneCopy" });
    expect(catalogues[language]["app.about"]).toBe(expected);
    expect(catalogues[language]["about.title"]).toBe(expected);
  });
});
