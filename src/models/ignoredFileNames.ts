// One Settings row of the ignored file names, checked by the core's rule
// (`visibility::is_file_name`) so a row that would fail the save is marked
// before it. Both rules are tested against tests/fixtures/ignored-file-names.json.

// Rust's `char::is_whitespace`, which `str::trim` removes on save. JavaScript's
// own trim differs at U+0085 and U+FEFF.
const WHITESPACE = "[\\t\\n\\v\\f\\r \\u0085\\u00a0\\u1680\\u2000-\\u200a\\u2028\\u2029\\u202f\\u205f\\u3000]";
const EDGE_WHITESPACE = new RegExp(`^${WHITESPACE}+|${WHITESPACE}+$`, "g");

/** A row as the core saves it; a blank row becomes "" and is dropped. */
export function trimIgnoredFileName(row: string): string {
  return row.replace(EDGE_WHITESPACE, "");
}

/** Whether a trimmed, non-blank row is the whole name of a file: not a path,
 * `.` or `..`, and holding no NUL or line break. */
export function isFileName(name: string): boolean {
  return name !== "" && name !== "." && name !== ".." && !/[/\\\0\n\r]/.test(name);
}

/** The trimmed row when it is not a file name and so holds Save, else null. */
export function invalidIgnoredFileName(row: string): string | null {
  const name = trimIgnoredFileName(row);
  return name === "" || isFileName(name) ? null : name;
}
