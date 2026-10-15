// One environment observer per webview. A zone change invalidates presentation,
// never persisted date evidence or completed/failed file work.
//
// The display zone is the `displayTimezone` setting: System, which follows
// the computer's zone, or a chosen zone. Every window learns the setting from
// its appearance read; under System the webview's own zone is the computer's.
const listeners = new Set<() => void>();
let snapshot = "";
let timer: ReturnType<typeof setInterval> | null = null;
// A chosen zone, or null for System.
let chosen: string | null = null;

function currentZone(): string {
  // Include the offset so a running webview also notices a clock transition.
  // The computer's zone counts under a chosen zone too: System entries name it.
  const computer = `${Intl.DateTimeFormat().resolvedOptions().timeZone}:${new Date().getTimezoneOffset()}`;
  return chosen === null ? computer : `${chosen} ${computer}`;
}

function check(): void {
  const next = currentZone();
  if (next === snapshot) return;
  snapshot = next;
  for (const listener of listeners) listener();
}

/** The zone user-facing times are shown in: a chosen zone, or undefined for
 * System, which formats in the computer's zone (UTC when it has none). */
export function displayTimeZone(): string | undefined {
  return chosen ?? undefined;
}

/** Takes the saved setting. A missing or unknown value, or a zone this
 * webview cannot format in, means System. */
export function applyDisplayZoneSetting(setting: unknown): void {
  let next: string | null = null;
  if (typeof setting === "string" && setting !== "system") {
    try {
      new Intl.DateTimeFormat("en", { timeZone: setting });
      next = setting;
    } catch {
      next = null;
    }
  }
  chosen = next;
  check();
}

export function displayZoneSnapshot(): string {
  return snapshot;
}

export function subscribeDisplayZone(listener: () => void): () => void {
  if (listeners.size === 0) {
    snapshot = currentZone();
    window.addEventListener("focus", check);
    document.addEventListener("visibilitychange", check);
    // There is no portable webview timezone-change event. Check while active
    // too, without querying the library until the actual environment changes.
    timer = setInterval(check, 60_000);
  }
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
    if (listeners.size !== 0) return;
    window.removeEventListener("focus", check);
    document.removeEventListener("visibilitychange", check);
    if (timer !== null) clearInterval(timer);
    timer = null;
  };
}
