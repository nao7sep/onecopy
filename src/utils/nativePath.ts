/** Render native path escapes without changing the identity used by IPC or stores. */
export function displayNativePath(value: string): string {
  return value.replace(/\u0000(?:b([0-9a-f]{2})|w([0-9a-f]{4}))/g,
    (_escape, byte: string | undefined, wide: string | undefined) =>
      byte !== undefined ? `\\x${byte.toUpperCase()}` : `\\u${wide!.toUpperCase()}`);
}
