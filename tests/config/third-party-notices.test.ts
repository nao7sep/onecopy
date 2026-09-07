import { readFileSync } from "node:fs";
import { join } from "node:path";
import { expect, it } from "vitest";

const rootFile = (path: string) => join(process.cwd(), path);

it("packages the native and copyleft notices and their source routes", () => {
  const config = JSON.parse(
    readFileSync(rootFile("src-tauri/tauri.conf.json"), "utf8"),
  ) as { bundle?: { resources?: Record<string, string> } };
  const notices = readFileSync(rootFile("THIRD_PARTY_NOTICES"), "utf8");
  const windowsPackager = readFileSync(rootFile("scripts/package.ps1"), "utf8");

  expect(config.bundle?.resources?.["../THIRD_PARTY_NOTICES"]).toBe(
    "THIRD_PARTY_NOTICES.txt",
  );
  for (const source of [
    "cssparser/0.36.0",
    "cssparser-macros/0.6.1",
    "dtoa-short/0.3.5",
    "option-ext/0.2.0",
    "selectors/0.36.1",
    "microsoft/onnxruntime/tree/v1.28.0",
    "whisper-rs-sys/0.15.0",
    "libwebp-sys/0.9.6",
  ]) {
    expect(notices).toContain(source);
  }
  expect(notices).toContain("ONNX Runtime third-party notices");
  expect(notices).toContain("Copyright (c) 2023-2024 The ggml authors");
  expect(notices).toContain("Copyright (c) 2010, Google Inc.");
  expect(windowsPackager).toContain('"THIRD_PARTY_NOTICES"');
});
