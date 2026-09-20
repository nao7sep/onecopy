import { createTranslator, type Message, type MessageValues } from "../../src/i18n/translate";
import type { MessageKey } from "../../src/i18n/catalogues";

// Renders a message the way an English interface shows it, so tests can keep
// asserting on the words a user reads. Helpers that now take a translator, or
// one of its formatters, get these; the on-screen-key gate in tests/setup.ts
// covers the other half, catching a key that reaches a rendered surface
// untranslated.

/** A fresh translator. A spec that changes the computer's zone needs one built
 * afterwards, because a translator caches its date formatters. */
export function createEnglish() {
  return createTranslator("en", "en-US");
}

export const english = createEnglish();

export const t = (key: MessageKey, values?: MessageValues): string => english.t(key, values);
export const number = (value: number): string => english.number(value);
export const percent = (ratio: number): string => english.percent(ratio);

export function inEnglish(message: Message | null | undefined): string | null {
  return message ? english.text(message) : null;
}
