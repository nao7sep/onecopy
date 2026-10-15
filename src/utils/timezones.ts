// The time zones OneCopy offers, for both zone settings. A zone is chosen from
// this list and never typed: the interpretation zone decides how dates without
// a zone are read, so a misspelling would silently re-date a library
// (timestamp-conventions, Time zones).
//
// The interpretation zone's built-in is the computer's zone at first launch,
// concrete: setup opens on it and saves it, and it stays until the user
// changes it. The display zone's built-in is System, which follows the
// computer. Both offer System, named with the zone it resolves to now.

/** The setting's value that follows the computer's zone. */
export const SYSTEM_TIME_ZONE = "system";

/** The computer's zone now, for the "System" entries and the wizard's start. */
export function computerTimeZone(): string {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
  } catch {
    return "UTC";
  }
}

// A saved zone the platform no longer lists still appears, so a hand-edited or
// retired value is visible rather than silently swapped for another zone.
export function timeZoneOptions(saved: string | null | undefined): string[] {
  const supported =
    typeof Intl.supportedValuesOf === "function" ? Intl.supportedValuesOf("timeZone") : [];
  const zones = new Set<string>(supported);
  zones.add("UTC");
  if (saved !== null && saved !== undefined && saved.trim() !== "" && saved !== SYSTEM_TIME_ZONE) {
    zones.add(saved);
  }
  return [...zones].sort((a, b) => a.localeCompare(b, "en"));
}
