// One environment observer per webview. A zone change invalidates presentation,
// never persisted date evidence or completed/failed file work.
const listeners = new Set<() => void>();
let snapshot = "";
let timer: ReturnType<typeof setInterval> | null = null;

function currentZone(): string {
  // Include the offset so a running webview also notices a clock transition.
  return `${Intl.DateTimeFormat().resolvedOptions().timeZone}:${new Date().getTimezoneOffset()}`;
}

function check(): void {
  const next = currentZone();
  if (next === snapshot) return;
  snapshot = next;
  for (const listener of listeners) listener();
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
