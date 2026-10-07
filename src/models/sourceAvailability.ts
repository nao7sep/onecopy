/** Directory ancestry uses path components, including Windows path spelling. */
export function pathUnavailable(path: string, unavailableRoots: readonly string[]): boolean {
  const normalize = (value: string) => {
    const slash = value.replace(/\\/g, "/").replace(/^\/\/\?\/UNC\//i, "//").replace(/^\/\/\?\//, "").replace(/\/+$/, "");
    return (/^[a-z]:/i.test(slash) || slash.startsWith("//")) ? slash.toLowerCase() : slash;
  };
  const candidate = normalize(path);
  return unavailableRoots.some((root) => {
    const parent = normalize(root);
    return candidate === parent || candidate.startsWith(`${parent}/`);
  });
}

export function allCopiesUnavailable(directories: readonly string[], unavailableRoots: readonly string[]): boolean {
  return directories.length > 0 && directories.every((path) => pathUnavailable(path, unavailableRoots));
}
