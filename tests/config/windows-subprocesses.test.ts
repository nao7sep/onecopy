import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

describe("Windows managed subprocesses", () => {
  it("keeps every bounded command-line tool off the desktop", () => {
    const source = readFileSync("src-tauri/src/subprocess.rs", "utf8");

    expect(source).toContain("use std::os::windows::process::CommandExt;");
    expect(source).toContain("const CREATE_NO_WINDOW: u32 = 0x0800_0000;");
    expect(source).toContain("command.creation_flags(CREATE_NO_WINDOW);");
  });
});
