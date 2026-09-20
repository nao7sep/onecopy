import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { LANGUAGES } from "./../src/i18n/languages";

// NSIS names its own translations; this maps each interface language to the
// name Tauri passes through, so the installer speaks what the app speaks.
const INSTALLER_LANGUAGES: Record<(typeof LANGUAGES)[number], string> = {
  en: "English",
  de: "German",
  es: "Spanish",
  fr: "French",
  it: "Italian",
  "pt-BR": "PortugueseBR",
  ru: "Russian",
  ja: "Japanese",
  ko: "Korean",
  "zh-Hans": "SimpChinese",
};

describe("Windows installer configuration", () => {
  it("allows the user to choose current-user or all-users installation", () => {
    const config = JSON.parse(
      readFileSync(join(process.cwd(), "src-tauri", "tauri.conf.json"), "utf8"),
    );

    expect(config.bundle?.windows?.nsis?.installMode).toBe("both");
  });

  it("offers the installer in every interface language", () => {
    const config = JSON.parse(
      readFileSync(join(process.cwd(), "src-tauri", "tauri.conf.json"), "utf8"),
    );

    expect(config.bundle?.windows?.nsis?.languages).toEqual(
      LANGUAGES.map((language) => INSTALLER_LANGUAGES[language]),
    );
  });

  it("ships the application licence in installed and portable packages", () => {
    const config = JSON.parse(
      readFileSync(join(process.cwd(), "src-tauri", "tauri.conf.json"), "utf8"),
    );
    const packageScript = readFileSync(
      join(process.cwd(), "scripts", "package.ps1"),
      "utf8",
    );

    expect(config.bundle?.resources?.["../LICENSE"]).toBe("LICENSE.txt");
    expect(packageScript).toContain(
      'Compress-Archive -Path "src-tauri/target/release/onecopy.exe", "LICENSE"',
    );
  });
});
