import { describe, expect, it } from "vitest";
import { CATALOGUES } from "../../src/i18n/catalogues";
import { LANGUAGES, type Language } from "../../src/i18n/languages";

// Localization-conventions: within one language, one concept keeps one term.
// English's own catalogue can (and does) use different words for the same
// idea across contexts ("section" vs "this section"); a translation must not
// let that surface as two unrelated words in the reader's language. This
// gate pins the "section" concept, which drifted independently in two
// languages (zh-Hans "分区" vs "分组", de "Bereich" vs "Abschnitt") because a
// later batch of notice strings was translated without the term already
// established by the Sections sidebar and the Recheck controls.

type Entry = string | Record<string, string>;
type Catalogue = Record<string, Entry>;

const catalogues = CATALOGUES as unknown as Record<Language, Catalogue>;

function text(entry: Entry | undefined): string {
  if (entry === undefined) return "";
  return typeof entry === "string" ? entry : Object.values(entry).join(" ");
}

// The word (or, for CJK, the compound) each language settled on for "section"
// wherever the app already uses it unambiguously: the Sections sidebar
// heading and the Recheck-this-section controls. Every other place the
// concept appears — the recheck-your-section family of background-work
// notices — must reuse the same word rather than a synonym.
const SECTION_TERM: Record<Language, string> = {
  en: "section",
  ja: "セクション",
  "zh-Hans": "分组",
  ko: "섹션",
  // Stems, not full words, for languages where the sidebar heading is plural
  // ("Secciones") but the recheck-notice family is singular ("sección"):
  // matching the shared stem catches a drifted synonym without tripping on
  // ordinary singular/plural inflection.
  es: "secci",
  "pt-BR": "seç",
  fr: "section",
  de: "Abschnitt",
  it: "sezio",
  ru: "раздел",
};

// English keys whose translated text must carry the section term above.
const SECTION_CONCEPT_KEYS = [
  "sidebar.sections",
  "grid.recheck",
  "grid.recheckHint",
  "reason.recheckSection",
  "notice.fileReadFailed",
  "notice.metadataReadFailed",
  "notice.previewFailed",
  "notice.videoPosterFailed",
  "notice.videoStripFailed",
  "notice.faceScoreFailed",
  "notice.transcriptFailed",
  "notice.derivedPrepFailed",
];

describe("glossary: one concept keeps one term per language", () => {
  it.each(LANGUAGES)("%s uses one word for \"section\" across the recheck-notice family", (language) => {
    const term = SECTION_TERM[language].toLowerCase();
    for (const key of SECTION_CONCEPT_KEYS) {
      const value = text(catalogues[language][key]).toLowerCase();
      expect(value.includes(term), `${language} ${key}: expected "${term}" in ${JSON.stringify(catalogues[language][key])}`).toBe(true);
    }
  });
});

// "Deleted files" names OneCopy's recoverable storage in the Trash surface's
// title; everything Browse and Restore say about that storage reuses the same
// word (a stem, where the language inflects it), so a reader never meets a
// second name for one place.
const DELETED_FILES_TERM: Record<Language, string> = {
  en: "deleted files",
  ja: "削除済みファイル",
  "zh-Hans": "已删除的文件",
  ko: "삭제된 파일",
  es: "eliminad",
  "pt-BR": "apagad",
  fr: "supprimé",
  de: "gelöscht",
  it: "eliminat",
  ru: "удалённ",
};

const DELETED_FILES_CONCEPT_KEYS = [
  "trash.title",
  "deletedFiles.title",
  "deletedFiles.loadFailed",
  "deletedFiles.loading",
  "deletedFiles.empty",
  "deletedFiles.noMatches",
  "deletedFiles.searchLabel",
  "restoreReview.changed",
  "restoreReview.companionsLeft",
  "restoreReview.skipAlreadyThere",
  "restoreReview.skipMissing",
  "activity.actionRestoreFiles",
  "notice.restoreFailed",
  "notice.restoreOccupied",
  "notice.restoreChanged",
  "notice.restoreMissing",
  "notice.restoreOtherDrive",
  "notice.restoreFolderBlocked",
  "notice.restoreUnplaceable",
  "notice.restoreOutcomeUnknown",
];

describe("glossary: Deleted files keeps its name", () => {
  it.each(LANGUAGES)("%s names deleted-file storage one way", (language) => {
    const term = DELETED_FILES_TERM[language].toLowerCase();
    for (const key of DELETED_FILES_CONCEPT_KEYS) {
      const value = text(catalogues[language][key]).toLowerCase();
      expect(value.includes(term), `${language} ${key}: expected "${term}" in ${JSON.stringify(catalogues[language][key])}`).toBe(true);
    }
  });
});

// "Restore" is one word per language everywhere OneCopy offers, reports or
// explains bringing a file back from Deleted files (a stem where the
// language inflects it), so it never reads as a second action.
const RESTORE_TERM: Record<Language, string> = {
  en: "restor",
  ja: "復元",
  "zh-Hans": "恢复",
  ko: "복원",
  es: "restaur",
  "pt-BR": "restaur",
  fr: "restaur",
  de: "wiederher",
  it: "ripristin",
  ru: "восстан",
};

const RESTORE_CONCEPT_KEYS = [
  "deletedFiles.restore",
  "deletedFiles.restoreFailed",
  "deletedFiles.revealRestored",
  "deletedFiles.unrecorded",
  "restoreReview.title",
  "restoreReview.renameAndRestore",
  "restoreReview.renamed",
  "restoreReview.target",
  "restoreReview.companionUnpaired",
  "mutation.restoreComplete",
  "mutation.restoreCancelled",
  "mutation.restoreWithFailures",
  "mutation.restoreStopped",
  "mutation.planningRestore",
  "mutation.restoring",
  "mutation.factRestored",
  "activity.actionRestoreFiles",
  "notice.restoreFailed",
  "notice.restoreOccupied",
  "notice.restoreChanged",
  "notice.restoreMissing",
  "notice.restoreOutcomeUnknown",
];

describe("glossary: Restore keeps its name", () => {
  it.each(LANGUAGES)("%s names Restore one way", (language) => {
    const term = RESTORE_TERM[language].toLowerCase();
    for (const key of RESTORE_CONCEPT_KEYS) {
      const value = text(catalogues[language][key]).toLowerCase();
      expect(value.includes(term), `${language} ${key}: expected "${term}" in ${JSON.stringify(catalogues[language][key])}`).toBe(true);
    }
  });
});
