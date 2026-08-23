// User-facing timestamp display (timestamp conventions): local time, ISO-ish,
// English, unlocalized — never toLocaleString (format drifts by machine) and
// never the raw serialized ISO/UTC form (wrong zone, storage grammar).

export function formatLocalMinute(input: string | number): string {
  const date = new Date(input);
  if (Number.isNaN(date.getTime())) return String(input);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(
    date.getHours(),
  )}:${pad(date.getMinutes())}`;
}
