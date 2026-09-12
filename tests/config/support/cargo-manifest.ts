export interface CargoTestTarget {
  name: string;
  path: string;
}

function normalized(source: string): string {
  return source.replace(/\r\n?/g, "\n");
}

export function cargoTableBody(source: string, name: string): string {
  const manifest = normalized(source);
  const heading = `[${name}]\n`;
  const start = manifest.indexOf(heading);
  if (start < 0) {
    throw new Error(`Cargo manifest section [${name}] is missing.`);
  }

  const body = manifest.slice(start + heading.length);
  const next = body.search(/^\[/m);
  return next < 0 ? body : body.slice(0, next);
}

export function cargoSuiteTargets(source: string): CargoTestTarget[] {
  return [...normalized(source).matchAll(
    /^\[\[test\]\]\nname = "([^"]+)"\npath = "(tests\/suites\/[^"]+\.rs)"$/gm,
  )].map((match) => ({ name: match[1]!, path: match[2]! }));
}
