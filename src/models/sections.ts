// Mirrors queries::SectionCounts / MonthSection on the Rust side.

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

/** The Undated section's display label (the design's wording). */
export function monthLabel(month: string): string {
  return month === "undated" ? "Undated" : month;
}
