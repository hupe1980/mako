// A long shell command must scroll inside its code block at narrow viewport widths.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { chromium } from "playwright";

const output = new URL("../public/", import.meta.url);
const browser = await chromium.launch();
try {
  for (const file of ["index.html", "de/index.html"]) {
    const html = await readFile(new URL(file, output), "utf8");
    const page = await browser.newPage({ javaScriptEnabled: false });
    const css = await readFile(new URL("style.css", output));
    await page.route("**/*", async route => {
      const url = new URL(route.request().url());
      if (url.pathname.endsWith("/style.css")) {
        await route.fulfill({ contentType: "text/css", body: css });
      } else {
        await route.abort();
      }
    });
    for (const width of [320, 1280]) {
      await page.setViewportSize({ width, height: 860 });
      await page.setContent(html);
      const metrics = await page.evaluate(() => ({
        grid: getComputedStyle(document.querySelector(".why-grid")).display,
        pageWidth: document.documentElement.scrollWidth,
      }));
      assert.equal(metrics.grid, "grid", `${file} must load the published stylesheet`);
      assert.equal(metrics.pageWidth, width, `${file} overflows at ${width}px`);
    }
    await page.close();
  }
  console.log("Landing pages fit 320px and 1280px in English and German.");
} finally {
  await browser.close();
}
