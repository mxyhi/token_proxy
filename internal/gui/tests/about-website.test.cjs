const assert = require("node:assert/strict");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");
const { fixture } = require("./fixtures/caller-keys.cjs");
const { openPart, watchSettingsReads, settingsDrawn } = require("./reader.cjs");

// The fork's Website row opens Token Proxy releases, in every language
// and at the tray panel's width.
const words = {
  en: "Website", zh: "官网", "zh-TW": "官網", ja: "公式サイト", de: "Website",
};

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const lang of Object.keys(words)) {
    for (const width of [1100, 420]) {
      test(`${engine} ${lang} ${width}: About links the website`, async (t) => {
        const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
        t.after(() => browser.close());
        const page = await (await browser.newContext({ viewport: { width, height: 800 }, reducedMotion: "reduce" })).newPage();
        page.setDefaultTimeout(6000);
        page.setDefaultNavigationTimeout(60000);
        const errors = [];
        page.on("pageerror", (e) => errors.push(e.message));
        await page.route("**/*", fixture(lang, "light", []));
        await watchSettingsReads(page);
        await page.goto("http://magpie.test/");
        await page.locator("#prefs").click();
        await settingsDrawn(page);
        await openPart(page, "about");
        const row = page.locator("#about .row", { has: page.locator(".who", { hasText: words[lang] }) });
        await row.waitFor();
        assert.equal(await row.locator("code").textContent(), "mxyhi/token_proxy");
        const opened = page.waitForRequest((r) => r.url().includes("/api/open"));
        await row.locator("button.text").click();
        assert.equal(JSON.parse((await opened).postData()).url, "https://github.com/mxyhi/token_proxy/releases");
        assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true, "About wider than the window");
        assert.deepEqual(errors, []);
      });
    }
  }
}
