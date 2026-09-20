// The time zones OneCopy offers. A zone is chosen from this list and never
// typed: the value decides how dates without a zone are read, so a misspelling
// would silently re-date a library (timestamp-conventions).
//
// It is not a display zone and never follows the computer after first launch:
// the first-launch wizard preselects the computer's zone and the value then
// stays until the user changes it.

// A saved zone the platform no longer lists still appears, so a hand-edited or
// retired value is visible rather than silently swapped for another zone.
export function timeZoneOptions(saved: string | null | undefined): string[] {
  const supported =
    typeof Intl.supportedValuesOf === "function" ? Intl.supportedValuesOf("timeZone") : [];
  const zones = new Set<string>(supported);
  zones.add("UTC");
  if (saved !== null && saved !== undefined && saved.trim() !== "") zones.add(saved);
  return [...zones].sort((a, b) => a.localeCompare(b, "en"));
}

// The computer's own zone, which first-launch setup starts from.
export function computerTimeZone(): string {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
  } catch {
    return "UTC";
  }
}
