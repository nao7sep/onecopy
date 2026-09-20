// User-facing timestamp display (timestamp-conventions): an instant is shown as
// a date and time in the computer's zone, formatted for the interface language —
// the computer's regional format when it uses that language, the language's own
// format otherwise. The translator owns that choice; this keeps the parsing and
// the unreadable-input fallback in one place.

import type { Translator } from "../i18n/translate";

export function formatLocalMinute(
  input: string | number,
  dateTime: Translator["dateTime"],
): string {
  const date = new Date(input);
  if (Number.isNaN(date.getTime())) return String(input);
  return dateTime(date);
}
