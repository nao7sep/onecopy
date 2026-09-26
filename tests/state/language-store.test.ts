// @vitest-environment happy-dom

// D-L9: if the appearance read never completes (a slow core, or
// window-appearance's own bound), the window must not paint in English
// regardless of what the computer's language actually is — it falls back to
// the browser's own preferred-locale resolution instead of a fixed default.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

async function withLanguages(languages: string[], run: () => Promise<void>): Promise<void> {
  const original = Object.getOwnPropertyDescriptor(window.navigator, "languages");
  Object.defineProperty(window.navigator, "languages", {
    configurable: true,
    get: () => languages,
  });
  try {
    await run();
  } finally {
    if (original) Object.defineProperty(window.navigator, "languages", original);
  }
}

beforeEach(() => vi.resetModules());
afterEach(() => vi.restoreAllMocks());

describe("language store default", () => {
  it("starts from the browser's own preferred locale, not a fixed English default", async () => {
    await withLanguages(["ja-JP", "en-US"], async () => {
      vi.resetModules();
      const { useLanguageStore } = await import("../../src/state/language-store");
      expect(useLanguageStore.getState().language).toBe("ja");
      expect(useLanguageStore.getState().systemLanguage).toBe("ja");
    });
  });

  it("falls back to English when nothing preferred is supported", async () => {
    await withLanguages(["xx-XX"], async () => {
      vi.resetModules();
      const { useLanguageStore } = await import("../../src/state/language-store");
      expect(useLanguageStore.getState().language).toBe("en");
    });
  });

  it("resolves every Chinese and Portuguese locale the same way the Rust core does", async () => {
    await withLanguages(["zh-TW"], async () => {
      vi.resetModules();
      const { useLanguageStore } = await import("../../src/state/language-store");
      expect(useLanguageStore.getState().language).toBe("zh-Hans");
    });
    await withLanguages(["pt-PT"], async () => {
      vi.resetModules();
      const { useLanguageStore } = await import("../../src/state/language-store");
      expect(useLanguageStore.getState().language).toBe("pt-BR");
    });
  });
});
