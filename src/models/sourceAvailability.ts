/** Normalize only Windows spelling: Unix backslashes and case are identity. */
function normalizePath(value: string): string {
  const windows = /^[a-z]:/i.test(value) || value.startsWith("\\\\") || value.startsWith("//");
  const path = windows
    ? value.replace(/\\/g, "/").replace(/^\/\/\?\/UNC\//i, "//").replace(/^\/\/\?\//, "").toLowerCase()
    : value;
  return path.replace(/\/+$/, "");
}

/** Directory ancestry uses path components, including Windows path spelling. */
export function pathUnavailable(path: string, unavailableRoots: readonly string[]): boolean {
  const candidate = normalizePath(path);
  return unavailableRoots.some((root) => {
    const parent = normalizePath(root);
    return candidate === parent || candidate.startsWith(`${parent}/`);
  });
}

export function allCopiesUnavailable(directories: readonly string[], unavailableRoots: readonly string[]): boolean {
  return directories.length > 0 && directories.every((path) => pathUnavailable(path, unavailableRoots));
}
