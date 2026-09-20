// Mirrors queries::SectionCounts / MonthSection on the Rust side.

import type { MessageKey } from "../i18n/catalogues";

export interface MonthSection {
  /** `"2016-03"`, or `"undated"` for the trailing section. */
  month: string;
  count: number;
}

export interface SectionCounts {
  images: MonthSection[];
  videos: MonthSection[];
  others: MonthSection[];
}

/** The Undated section's display label (the design's wording), or null for a
 * dated section, whose `"2016-03"` already names itself in every language. */
export function monthLabel(month: string): MessageKey | null {
  return month === "undated" ? "section.undated" : null;
}
