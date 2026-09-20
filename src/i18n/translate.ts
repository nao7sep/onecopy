import { Fragment, createElement, type ReactNode } from "react";
import { CATALOGUES, type Catalogue, type MessageKey } from "./catalogues";
import type { Language } from "./languages";

// A value is plain text, a number formatted for the locale, or another
// message, which renders in the same language (a reason inside a sentence).
export type MessageValue = string | number | Message;

export type MessageValues = { [name: string]: MessageValue };

// Text held in state (toasts, dialogs, results, load failures) is a key plus
// values, never a finished string, so it renders in whatever language is
// current when it is shown.
export type Message = {
  key: MessageKey;
  values?: MessageValues;
};

export function message(key: MessageKey, values?: MessageValues): Message {
  return values === undefined ? { key } : { key, values };
}

const PLACEHOLDER = /\{(\w+)\}/g;

export type Translator = {
  language: Language;
  locale: string;
  t: (key: MessageKey, values?: MessageValues) => string;
  // Like t, but a placeholder may be filled with markup (a <code> path, say).
  rich: (key: MessageKey, values: Record<string, ReactNode>) => ReactNode;
  text: (message: Message) => string;
  number: (value: number) => string;
  percent: (ratio: number) => string;
  // An instant, as a date and time in the given IANA zone (the computer's zone
  // when none is given).
  dateTime: (date: Date, timeZone?: string | null) => string;
  // A month section's heading, such as "March 2016".
  monthYear: (year: number, month: number) => string;
  // Names run together the way the language lists them ("a, b, c").
  list: (items: readonly string[]) => string;
};

export function createTranslator(language: Language, locale: string = language): Translator {
  const catalogue: Catalogue = CATALOGUES[language];
  const numberFormat = new Intl.NumberFormat(locale);
  const percentFormat = new Intl.NumberFormat(locale, { style: "percent", maximumFractionDigits: 0 });
  const dateTimeFormats = new Map<string, Intl.DateTimeFormat>();
  const monthYearFormat = new Intl.DateTimeFormat(locale, { year: "numeric", month: "long", timeZone: "UTC" });
  const listFormat = new Intl.ListFormat(locale, { type: "conjunction", style: "narrow" });
  const pluralRules = new Intl.PluralRules(language);

  function template(key: MessageKey, values: MessageValues | undefined): string {
    const entry = catalogue[key];
    if (typeof entry === "string") {
      return entry;
    }
    // A key the catalogue does not carry shows as itself rather than taking the
    // window down; the catalogue gate and the on-screen-key check both fail on
    // it, so it cannot reach a release unnoticed.
    if (entry === undefined || entry === null) {
      return key;
    }
    // A plural entry holds one form per CLDR category the language uses; the
    // catalogue gate guarantees the category the rules select is present.
    const count = typeof values?.count === "number" ? values.count : 0;
    const forms = entry as Record<string, string>;
    return forms[pluralRules.select(count)] ?? forms.other;
  }

  function format(value: MessageValue): string {
    if (typeof value === "number") return numberFormat.format(value);
    if (typeof value === "string") return value;
    return t(value.key, value.values);
  }

  function t(key: MessageKey, values?: MessageValues): string {
    return template(key, values).replace(PLACEHOLDER, (whole, name: string) =>
      values !== undefined && name in values ? format(values[name]) : whole,
    );
  }

  function rich(key: MessageKey, values: Record<string, ReactNode>): ReactNode {
    const parts = template(key, undefined).split(PLACEHOLDER);
    // split with a capture group alternates literal text and placeholder names.
    return parts.map((part, index) =>
      index % 2 === 0
        ? part
        : createElement(Fragment, { key: index }, part in values ? values[part] : `{${part}}`),
    );
  }

  return {
    language,
    locale,
    t,
    rich,
    text: (message) => t(message.key, message.values),
    number: (value) => numberFormat.format(value),
    percent: (ratio) => percentFormat.format(ratio),
    dateTime: (date, timeZone) => {
      const zone = timeZone ?? "";
      let format = dateTimeFormats.get(zone);
      if (format === undefined) {
        format = new Intl.DateTimeFormat(locale, {
          dateStyle: "medium",
          timeStyle: "short",
          timeZone: timeZone ?? undefined,
        });
        dateTimeFormats.set(zone, format);
      }
      return format.format(date);
    },
    monthYear: (year, month) => monthYearFormat.format(new Date(Date.UTC(year, month - 1, 1))),
    list: (items) => listFormat.format(items),
  };
}
