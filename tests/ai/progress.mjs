export function jsonLineEvents(onEvent) {
  let pending = "";
  const consume = (line) => {
    if (!line.trim()) return;
    try {
      onEvent(JSON.parse(line));
    } catch {
      // Incidental native output is diagnostic-only and may contain local
      // details, so the harness never relays it to the shareable console.
    }
  };
  return {
    push(chunk) {
      pending += chunk;
      const lines = pending.split(/\r?\n/);
      pending = lines.pop() ?? "";
      lines.forEach(consume);
    },
    finish() {
      consume(pending);
      pending = "";
    },
  };
}
