// The time zones OneCopy offers. A zone is chosen from this list and never
// typed: the value decides how dates without a zone are read, so a misspelling
// would silently re-date a library (timestamp-conventions).
//
// It is not a display zone. The built-in is the `system` token, which follows
// the computer's zone wherever it is used (config-sets-conventions); a chosen
// zone is saved and stays until the user changes it. The first-run wizard
// opens on the computer's zone itself, so finishing setup saves a concrete
// zone, and "System" is offered as a deliberate choice.

/** The setting's value that follows the computer's zone. */
export const SYSTEM_TIME_ZONE = "system";

/** The computer's zone now, for the "System" label and the wizard's start. */
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
