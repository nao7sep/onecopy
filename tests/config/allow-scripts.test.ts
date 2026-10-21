// @vitest-environment node
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

type LockedPackage = { version?: string; resolved?: string; hasInstallScript?: boolean; link?: boolean };
type Packages = Record<string, LockedPackage>;
type Policy = Record<string, boolean>;

const manifest = JSON.parse(readFileSync(new URL("../../package.json", import.meta.url), "utf8")) as {
  allowScripts: Policy;
  packageManager: string;
};
const lockfile = JSON.parse(readFileSync(new URL("../../package-lock.json", import.meta.url), "utf8")) as {
  packages: Packages;
};

// npm trusts the registry tarball URL rather than the unpacked manifest's
// name/version. An exact approval cannot match when that URL is missing.
// This app reviews each version separately, including explicit denials.
function approvalProblems(policy: Policy, packages: Packages): string[] {
  const problems: string[] = [];
  const installed = new Set<string>();
  for (const [path, entry] of Object.entries(packages)) {
    const marker = path.lastIndexOf("node_modules/");
    if (marker < 0 || entry.link) continue;
    if (!entry.version) {
      if (entry.hasInstallScript) problems.push(`${path}: missing locked version`);
      continue;
    }
    const name = path.slice(marker + "node_modules/".length);
    const identity = `${name}@${entry.version}`;
    installed.add(identity);
    if (!entry.hasInstallScript) continue;
    const basename = name.slice(name.lastIndexOf("/") + 1);
    let trusted = false;
    if (entry.resolved) {
      try {
        const url = new URL(entry.resolved);
        trusted = ["https:", "http:"].includes(url.protocol)
          && decodeURIComponent(url.pathname).endsWith(`/${name}/-/${basename}-${entry.version}.tgz`);
      } catch {
        // An invalid source cannot prove a registry identity.
      }
    }
    if (!trusted) problems.push(`${identity}: missing or inconsistent registry identity`);
    if (typeof policy[identity] !== "boolean") problems.push(`${identity}: missing decision`);
  }
  for (const [key, value] of Object.entries(policy)) {
    if (!installed.has(key)) problems.push(`${key}: not an exact installed identity`);
    if (typeof value !== "boolean") problems.push(`${key}: decision must be boolean`);
  }
  return problems;
}

describe("install-script approvals", () => {
  it("installs the manifest-selected npm before every CI dependency install", () => {
    expect(manifest.packageManager).toMatch(/^npm@12\.\d+\.\d+$/);
    const workflow = readFileSync(new URL("../../.github/workflows/release.yml", import.meta.url), "utf8");
    const installs = workflow.split("- run: npm ci").slice(0, -1);
    expect(installs.length).toBeGreaterThan(0);
    for (const beforeInstall of installs) {
      expect(beforeInstall).toContain(`npm install --global "$(node -p "require('./package.json').packageManager")"`);
    }
  });

  it("decides every locked script using an exact installed registry identity", () => {
    expect(approvalProblems(manifest.allowScripts, lockfile.packages)).toEqual([]);
  });

  it("supports scoped packages installed below another dependency", () => {
    const packages = {
      "node_modules/parent/node_modules/@scope/tool": {
        version: "1.2.3",
        resolved: "https://registry.npmjs.org/@scope/tool/-/tool-1.2.3.tgz",
        hasInstallScript: true,
      },
    };
    expect(approvalProblems({ "@scope/tool@1.2.3": false }, packages)).toEqual([]);
  });

  it.each([
    { resolved: undefined },
    { resolved: "not a URL" },
    { resolved: "https://registry.npmjs.org/fsevents/-/fsevents-2.3.2.tgz" },
    { resolved: "https://registry.npmjs.org/another-package/-/another-package-2.3.3.tgz" },
  ])("rejects an approval whose lock source cannot prove its identity: $resolved", (change) => {
    const path = "node_modules/fsevents";
    const packages = { ...lockfile.packages, [path]: { ...lockfile.packages[path], ...change } };
    expect(approvalProblems(manifest.allowScripts, packages)).toContain(
      "fsevents@2.3.3: missing or inconsistent registry identity",
    );
  });

  it.each(["fsevents", "fsevents@*", "fsevents@^2.3.3", "fsevents@2.3.2"])(
    "rejects a broad or stale decision: %s", (key) => {
      const policy = { ...manifest.allowScripts };
      delete policy["fsevents@2.3.3"];
      policy[key] = false;
      expect(approvalProblems(policy, lockfile.packages)).toContain(`${key}: not an exact installed identity`);
      expect(approvalProblems(policy, lockfile.packages)).toContain("fsevents@2.3.3: missing decision");
    },
  );
  it("recognizes nested scoped packages and rejects missing versions", () => {
    const path = "node_modules/parent/node_modules/@scope/tool";
    const entry = {
      version: "1.2.3",
      resolved: "https://registry.npmjs.org/@scope/tool/-/tool-1.2.3.tgz",
      hasInstallScript: true,
    };
    expect(approvalProblems({ "@scope/tool@1.2.3": false }, { [path]: entry })).toEqual([]);
    expect(approvalProblems({}, { [path]: { ...entry, version: undefined } })).toContain(
      `${path}: missing locked version`,
    );
  });
});
