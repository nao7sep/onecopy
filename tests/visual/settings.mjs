// Disposable synthetic fixture only; never connects to a native app or user data.
// Start Vite first, then set ONECOPY_PLAYWRIGHT_MODULE if Playwright is external.
import os from "node:os";
import path from "node:path";
const { chromium } = await import(process.env.ONECOPY_PLAYWRIGHT_MODULE ?? "playwright");
const base = process.env.ONECOPY_FIXTURE_URL ?? "http://127.0.0.1:28867";
const output = process.env.ONECOPY_SCREENSHOT_DIR ?? os.tmpdir();
const browser = await chromium.launch({headless:true});
for (const language of ['en','ja']) for (const scheme of ['light','dark']) {
 const page=await browser.newPage({viewport:{width:1000,height:850},colorScheme:scheme});
 await page.goto(`${base}/tests/fixtures/settings.html?language=${language}`);
 await page.getByRole('dialog').waitFor();
 await page.getByRole('tab').nth(1).click();
 await page.screenshot({path:path.join(output, `onecopy-settings-${process.argv[2] ?? "after"}-${language}-${scheme}.png`)});
 await page.goto(`${base}/tests/fixtures/settings.html?language=${language}&surface=playback`);
 await page.locator('footer button').first().waitFor();
 await page.locator("footer").screenshot({path:path.join(output, `onecopy-playback-${process.argv[2] ?? "after"}-${language}-${scheme}.png`)});
 await page.close();
}
await browser.close();
