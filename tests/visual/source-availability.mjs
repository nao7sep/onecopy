// Synthetic frontend surfaces only; no native app or personal files.
import os from "node:os";
import path from "node:path";
const { chromium } = await import(process.env.ONECOPY_PLAYWRIGHT_MODULE ?? "playwright");
const base = process.env.ONECOPY_FIXTURE_URL ?? "http://127.0.0.1:28867";
const output = process.env.ONECOPY_SCREENSHOT_DIR ?? os.tmpdir();
const browser = await chromium.launch({ headless: true });
try {
  for (const language of ["en", "ja"]) for (const scheme of ["light", "dark"]) {
    const page = await browser.newPage({ viewport: { width: 1000, height: 650 }, colorScheme: scheme });
    for (const surface of ["details", "issues"]) {
      await page.goto(`${base}/tests/fixtures/source-availability.html?language=${language}&surface=${surface}`);
      await page.locator(surface === "issues" ? '[role="dialog"]' : 'aside dd').first().waitFor();
      await page.screenshot({ path: path.join(output, `onecopy-source-${surface}-${language}-${scheme}.png`) });
    }
    await page.close();
  }
} finally { await browser.close(); }
