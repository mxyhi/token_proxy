// Run with Node's test runner and Playwright on the module path; see README.md.
// A provider's Failure rest (provider.RestCap, the CLI's `provider
// rest-cap`): its editor has the row after Queue wait, under Limits and
// cost, with Default, No rest and Custom. A save that never touched it
// sends no restCap, so what the CLI set stays; No rest posts -1, Custom
// the minutes typed, Default 0. A signed-in account (Codex) set to no rest
// opens on No rest with Advanced open. At the narrowest windows nothing
// runs off the page. In English, Chinese, Japanese and German.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");

const assets = path.resolve(__dirname, "../assets");
const unfold = async (page) => {
  const adv = page.locator(".editor details.advanced");
  await adv.waitFor({ state: "attached" });
  if (!(await adv.evaluate((d) => d.open))) await adv.locator(":scope > summary").click();
};
const relay = {
  id: "relay", name: "Relay", icon: "generic", chat: "https://relay.example/v1", responses: "", anthropic: "", catalog: "",
  models: [{ id: "x/m", name: "M", on: true }], agents: [], fallback: [], headers: {},
  key: { set: true, masked: "sk-…one" }, keyList: [], balanceToken: { takes: false, set: false }, proxy: "", maxConcurrency: null,
};
const codex = {
  id: "codex", name: "Codex", icon: "codex-color", chat: "", responses: "", anthropic: "", catalog: "",
  models: [{ id: "gpt-6", name: "GPT-6", on: true }], agents: [], fallback: [], headers: {}, keyList: [],
  account: { agent: "codex", agentName: "Codex", user: "me@example.com", plan: "PLUS", logins: [{ user: "me@example.com", plan: "PLUS", active: true, on: true }] },
  proxy: "", maxConcurrency: null, restCap: -1,
};

function serve(lang, posts) {
  const providers = { providers: [relay, codex], presets: [], excluded: [], gateway: { running: true, window: true } };
  const state = { agents: [], profiles: [], settings: { lang, theme: "light" } };
  return async (route) => {
    const url = new URL(route.request().url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:true};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json(state);
    if (url.pathname === "/api/providers") return json(providers);
    if (url.pathname === "/api/groups") return json({ groups: [] });
    if (url.pathname.startsWith("/api/provider/") && route.request().method() === "POST") {
      posts.push({ action: url.pathname.slice("/api/provider/".length), body: route.request().postDataJSON() });
      return json(providers);
    }
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    await route.fulfill({ body: await fs.readFile(file), contentType });
  };
}

const words = {
  en: { label: "Failure rest", off: "No rest" },
  zh: { label: "失败冷却", off: "不冷却" },
  ja: { label: "失敗時の休止", off: "休止しない" },
  de: { label: "Pause nach Fehlern", off: "Keine Pause" },
};

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const lang of ["en", "zh", "ja", "de"]) {
    const w = words[lang];
    const open = async (t, name, width = 900) => {
      const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
      t.after(async () => {
        if (process.env.ARTIFACT_DIR) {
          await fs.mkdir(process.env.ARTIFACT_DIR, { recursive: true });
          await page.screenshot({ path: path.join(process.env.ARTIFACT_DIR, `${engine}-${lang}-${name}-${width}-rest-cap.png`), fullPage: true });
        }
        await browser.close();
      });
      const page = await (await browser.newContext({ viewport: { width, height: 760 }, reducedMotion: "reduce" })).newPage();
      page.setDefaultTimeout(5000);
      page.setDefaultNavigationTimeout(60000);
      const errors = [];
      page.on("pageerror", (e) => errors.push(e.message));
      const posts = [];
      await page.route("**/*", serve(lang, posts));
      await page.goto("http://magpie.test/?view=providers");
      await page.locator(".row.provider", { hasText: name }).click();
      await unfold(page);
      await page.locator(".editor .rest-cap").waitFor();
      await page.waitForTimeout(300); // the editor drawn again as what it asked for comes in
      return { page, errors, posts };
    };
    const mode = (page, v) => page.locator(`.editor .rest-cap .opt[data-seg="${v}"]`);
    const mins = (page) => page.locator(".editor input.rest-cap-min");
    const save = async (page, posts) => {
      const n = posts.length;
      await page.locator(".editor .bar").getByRole("button", { name: await page.evaluate(() => t("Save")), exact: true }).click();
      for (let i = 0; i < 50 && posts.length === n; i++) await page.waitForTimeout(50);
      assert.equal(posts.length, n + 1, "Save posted");
      return posts.at(-1).body;
    };
    const reopen = async (page, name) => {
      await page.locator(".row.provider", { hasText: name }).click();
      await unfold(page);
      await page.locator(".editor .rest-cap").waitFor();
    };

    test(`${engine} ${lang}: the row saves nothing untouched, -1 for no rest, the minutes for custom, 0 for default`, async (t) => {
      const { page, errors, posts } = await open(t, "Relay");
      const label = await page.locator(".editor .rest-cap").evaluate((e) => e.parentElement.previousElementSibling?.textContent || "");
      assert.equal(label.trim(), w.label);
      assert.equal(await mode(page, "").getAttribute("aria-pressed"), "true", "opens on Default");
      assert.equal(await mins(page).isVisible(), false, "no minutes box outside Custom");
      // after Queue wait, under the same heading
      const order = await page.evaluate(() => {
        const q = document.querySelector(".editor input.queue-wait"), r = document.querySelector(".editor .rest-cap");
        return !!(q.compareDocumentPosition(r) & Node.DOCUMENT_POSITION_FOLLOWING);
      });
      assert(order, "after Queue wait");

      let body = await save(page, posts);
      assert(!Object.hasOwn(body, "restCap"), "an untouched row sends nothing, so the CLI's value stays");

      await reopen(page, "Relay");
      await mode(page, "off").click();
      assert.equal(await page.locator(".editor .rest-cap").evaluate((e) => e.parentElement.querySelector(".hint.expl").textContent), await page.evaluate(() => t("The next request is sent to it as usual after such a failure. A refused key (401, 403), quota or credit used up, a rate limit or a model it doesn't serve still rests it")));
      body = await save(page, posts);
      assert.equal(body.restCap, -1);

      await reopen(page, "Relay");
      await mode(page, "custom").click();
      assert.equal(await mins(page).isVisible(), true, "the minutes box shows for Custom");
      await mins(page).fill("5");
      body = await save(page, posts);
      assert.equal(body.restCap, 5);

      await reopen(page, "Relay");
      await mode(page, "custom").click();
      body = await save(page, posts);
      assert.equal(body.restCap, 0, "Custom left empty is the default");

      await reopen(page, "Relay");
      await mode(page, "off").click();
      await mode(page, "").click();
      body = await save(page, posts);
      assert.equal(body.restCap, 0);

      const missing = await page.evaluate(() => [
        "Failure rest", "No rest",
        "After a failure that says nothing of why (a 5xx, a dropped connection), a key or account sits out a minute, twice as long for each failure in a row, at most 10 minutes; rate limited, at most 30",
        "The next request is sent to it as usual after such a failure. A refused key (401, 403), quota or credit used up, a rate limit or a model it doesn't serve still rests it",
        "As the default, but a key or account sits out at most the minutes set here, rate limited too; empty is the default",
      ].flatMap((k) => ["zh", "zh-TW", "ja", "de"].filter((l) => !I18N[l][k]).map((l) => `${l}: ${k}`)));
      assert.deepEqual(missing, [], "every string has its translations");
      assert.equal(await page.locator(".editor select").count(), 0, "no native select");
      assert.deepEqual(errors, []);
    });

    test(`${engine} ${lang}: a signed-in account set to no rest opens on it and saves with its picks`, async (t) => {
      const { page, errors, posts } = await open(t, "Codex");
      assert.equal(await page.locator(".editor details.advanced").evaluate((d) => d.open), true, "Advanced opens on a set rest");
      assert.equal(await mode(page, "off").getAttribute("aria-pressed"), "true");
      assert.equal(await page.locator(".editor .rest-cap .opt.on").textContent(), w.off);
      let body = await save(page, posts);
      assert(!Object.hasOwn(body, "restCap"), "untouched, nothing sent");
      assert.deepEqual(body.models, ["gpt-6"], "its picks go with it");
      await reopen(page, "Codex");
      await mode(page, "").click();
      body = await save(page, posts);
      assert.equal(body.restCap, 0);
      assert.deepEqual(errors, []);
    });

    for (const width of [360, 440]) {
      test(`${engine} ${lang} ${width}px: the row and its minutes stay on the page`, async (t) => {
        const { page, errors } = await open(t, "Relay", width);
        await mode(page, "custom").click();
        const fit = await page.evaluate(() => {
          const rs = [...document.querySelectorAll(".editor .rest-cap, .editor .rest-cap .opt, .editor input.rest-cap-min")].map((e) => e.getBoundingClientRect());
          return { right: Math.max(...rs.map((r) => r.right)), box: document.querySelector(".editor input.rest-cap-min").getBoundingClientRect().width, page: document.documentElement.clientWidth, scroll: document.documentElement.scrollWidth };
        });
        assert(fit.right <= fit.page + 0.5, `the row runs to ${fit.right}, the page is ${fit.page}`);
        assert(fit.box >= 60, `the minutes box is ${fit.box}px`);
        assert(fit.scroll <= fit.page + 0.5, `the page scrolls sideways: ${fit.scroll} > ${fit.page}`);
        assert.deepEqual(errors, []);
      });
    }
  }
}
