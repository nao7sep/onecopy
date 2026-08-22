// Durable JSON stays verbatim on disk; consumers receive only members whose
// shape they can actually use. A TypeScript assertion does not validate JSON
// loaded across IPC and lets one wrong-shape member crash path operations.
export function stringArrayField(
  config: Record<string, unknown> | null,
  key: string,
): string[] {
  const value = config?.[key];
  return Array.isArray(value)
    ? value.filter((member): member is string => typeof member === "string")
    : [];
}
