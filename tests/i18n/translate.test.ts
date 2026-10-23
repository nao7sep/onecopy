import { describe, expect, it, vi } from "vitest";
import type { CatalogueEntry, MessageKey } from "../../src/i18n/catalogues";
import { createTranslator, message } from "../../src/i18n/translate";

// The translator itself: placeholder filling, locale formatting, and the
// message-descriptor shape stores keep instead of finished sentences.

describe("createTranslator", () => {
  it("fills placeholders from the language's own catalogue", () => {
    expect(createTranslator("ja").t("nativeMenu.about", { app: "OneCopy" })).toBe(
      "OneCopy について",
    );
    expect(createTranslator("de").t("nativeMenu.about", { app: "OneCopy" })).toBe(
      "Über OneCopy",
    );
  });

  it("renders a descriptor held in state at display time", () => {
    const held = message("settings.language");
    expect(createTranslator("fr").text(held)).toBe("Langue");
    expect(createTranslator("ko").text(held)).toBe("언어");
  });

  it("formats numbers, percentages and lists for the locale", () => {
    const german = createTranslator("de", "de-DE");
    expect(german.number(1234567)).toBe("1.234.567");
    expect(german.percent(0.25)).toBe("25\u00a0%");
    expect(german.list(["a", "b", "c"])).toBe("a, b und c");
    expect(createTranslator("en").list(["a", "b", "12 more"])).toBe("a, b, and 12 more");
  });

  it("formats an instant in the zone it is given", () => {
    const tokyo = createTranslator("ja", "ja-JP").dateTime(
      new Date("2026-03-01T15:30:00Z"),
      "Asia/Tokyo",
    );
    expect(tokyo).toContain("2026");
    expect(tokyo).toContain("0:30");
  });

  it("names a month section in the language", () => {
    expect(createTranslator("en", "en-US").monthYear(2016, 3)).toBe("March 2016");
    expect(createTranslator("ru", "ru-RU").monthYear(2016, 3)).toBe("март 2016 г.");
  });

  it("uses English until the requested catalogue has loaded", async () => {
    // Suite setup preloads every language. Fresh modules exercise the actual
    // cold catalogue owner without adding a production reset API.
    vi.resetModules();
    const { createTranslator: freshTranslator } = await import("../../src/i18n/translate");
    const { loadCatalogue } = await import("../../src/i18n/catalogues");
    expect(freshTranslator("de").t("nativeMenu.about", { app: "OneCopy" })).toBe("About OneCopy");
    await loadCatalogue("de");
    expect(freshTranslator("de").t("nativeMenu.about", { app: "OneCopy" })).toBe("Über OneCopy");
  });

  it("exposes a missing entry as its key without throwing", async () => {
    vi.resetModules();
    const { createTranslator: freshTranslator } = await import("../../src/i18n/translate");
    const { loadCatalogue } = await import("../../src/i18n/catalogues");
    const catalogue = await loadCatalogue("de") as Partial<Record<MessageKey, CatalogueEntry>>;
    const key = "nativeMenu.about";
    const entry = catalogue[key];
    delete catalogue[key];
    try {
      // Loaded catalogues must be complete; the final key is deliberately
      // visible to the rendered-key gate if that contract is broken.
      expect(freshTranslator("de").t(key, { app: "OneCopy" })).toBe(key);
      expect(freshTranslator("en").t(key, { app: "OneCopy" })).toBe("About OneCopy");
    } finally {
      catalogue[key] = entry;
    }
  });
});
