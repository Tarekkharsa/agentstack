// Tutorial recording harness for the AgentStack panel in t3code (web).
// One Playwright context per scenario => one native .webm, converted to .mp4.
// Captions are injected into the page so narration is burned into the footage.
const { chromium } = require("/Users/tarek.k/Documents/GitHub/agentstack/node_modules/playwright");
const fs = require("node:fs");
const path = require("node:path");

const OUT = process.env.TUT_OUT || "/Users/tarek.k/Documents/GitHub/agentstack/.tutorials";
const PAIR = process.env.TUT_PAIR;
const VIEW = { width: 1040, height: 720 };
const OUTSIZE = { width: 1560, height: 1080 }; // 1.5x capture => crisp text

const CAP_CSS =
  "position:fixed;left:0;right:0;bottom:0;z-index:2147483647;" +
  "background:linear-gradient(to top,rgba(8,9,11,.96) 55%,rgba(8,9,11,0));" +
  "color:#f2f4f7;font:600 21px/1.45 -apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif;" +
  "padding:70px 40px 26px;pointer-events:none;letter-spacing:-.01em;" +
  "text-shadow:0 1px 3px rgba(0,0,0,.8)";

const TITLE_CSS =
  "position:fixed;inset:0;z-index:2147483646;background:#0b0c0e;color:#fff;" +
  "display:flex;flex-direction:column;align-items:center;justify-content:center;gap:14px;" +
  "font:-apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif;text-align:center";

async function ensureCaption(page) {
  await page.evaluate((css) => {
    if (!document.getElementById("__cap")) {
      const el = document.createElement("div");
      el.id = "__cap";
      el.style.cssText = css;
      document.body.appendChild(el);
    }
  }, CAP_CSS);
}

// Narrate: set caption text, hold for `ms` so a viewer can read it.
async function say(page, text, ms = 2800) {
  await ensureCaption(page);
  await page.evaluate((t) => {
    const e = document.getElementById("__cap");
    if (e) e.textContent = t;
  }, text);
  await page.waitForTimeout(ms);
}

async function titleCard(page, title, subtitle, ms = 2600) {
  await page.evaluate(
    ({ css, title, subtitle }) => {
      const w = document.createElement("div");
      w.id = "__title";
      w.style.cssText = css;
      const h = document.createElement("div");
      h.textContent = title;
      h.style.cssText = "font-size:44px;font-weight:700;letter-spacing:-.02em";
      const s = document.createElement("div");
      s.textContent = subtitle;
      s.style.cssText = "font-size:20px;font-weight:500;color:#9aa1ab;max-width:640px";
      w.appendChild(h);
      w.appendChild(s);
      document.body.appendChild(w);
    },
    { css: TITLE_CSS, title, subtitle }
  );
  await page.waitForTimeout(ms);
  await page.evaluate(() => document.getElementById("__title")?.remove());
  await page.waitForTimeout(400);
}

// Move a visible pointer dot to an element and click it, so the video shows intent.
async function installCursor(page) {
  await page.evaluate(() => {
    if (document.getElementById("__cur")) return;
    const c = document.createElement("div");
    c.id = "__cur";
    c.style.cssText =
      "position:fixed;width:22px;height:22px;border-radius:50%;z-index:2147483647;" +
      "background:rgba(96,165,250,.35);border:2px solid #60a5fa;pointer-events:none;" +
      "transform:translate(-50%,-50%);transition:left .45s cubic-bezier(.4,0,.2,1),top .45s cubic-bezier(.4,0,.2,1),opacity .2s;opacity:0";
    document.body.appendChild(c);
  });
}

async function pointAt(page, locator) {
  const box = await locator.boundingBox();
  if (!box) return null;
  const x = box.x + box.width / 2;
  const y = box.y + box.height / 2;
  await page.evaluate(
    ({ x, y }) => {
      const c = document.getElementById("__cur");
      if (c) {
        c.style.opacity = "1";
        c.style.left = x + "px";
        c.style.top = y + "px";
      }
    },
    { x, y }
  );
  await page.waitForTimeout(700);
  return { x, y };
}

async function clickIt(page, locator, holdAfter = 1200) {
  await pointAt(page, locator);
  await page.evaluate(() => {
    const c = document.getElementById("__cur");
    if (c) c.style.transform = "translate(-50%,-50%) scale(.7)";
  });
  await page.waitForTimeout(160);
  await locator.click({ timeout: 15000 });
  await page.evaluate(() => {
    const c = document.getElementById("__cur");
    if (c) c.style.transform = "translate(-50%,-50%) scale(1)";
  });
  await page.waitForTimeout(holdAfter);
}

// The chip's accessible name is "AgentStack — needs you"; its visible text is
// just the state word, and there are two matches (one is off-screen), so take
// the last visible one.
function chipOf(page) {
  return page.locator('button[aria-label*="AgentStack" i]:visible').last();
}
const POPOVER = '[role="dialog"]';

async function openPanel(page) {
  const chip = chipOf(page);
  await chip.waitFor({ state: "visible", timeout: 20000 });
  await clickIt(page, chip, 1100);
}

async function openManage(page) {
  const manage = page.locator(POPOVER + ' button', { hasText: /Manage/i }).first();
  await clickIt(page, manage, 1800);
}

async function tab(page, name) {
  const dlg = page.locator('[role="dialog"]').first();
  const t = dlg.locator("button", { hasText: new RegExp("^" + name, "i") }).first();
  if (await t.isVisible().catch(() => false)) await clickIt(page, t, 1500);
}

// Click if present; narrate and carry on if not, so one missing affordance
// never kills a recording.
async function tryClick(page, locator, missingNote) {
  if (await locator.isVisible().catch(() => false)) {
    await clickIt(page, locator, 1400);
    return true;
  }
  if (missingNote) await say(page, missingNote, 1800);
  return false;
}

// Scroll INSIDE the panel dialog. Content routinely renders below the fold, so
// narrating without this plays over a half-visible view.
async function reveal(page, amount = 420, settle = 900) {
  const dlg = page.locator('[role="dialog"]').first();
  await dlg.hover().catch(() => {});
  await page.mouse.wheel(0, amount);
  await page.waitForTimeout(settle);
}

async function closeDialog(page) {
  const x = page.locator('[role="dialog"] button[aria-label*="Close" i], [role="dialog"] button:has-text("Close")').first();
  if (await x.isVisible().catch(() => false)) await clickIt(page, x, 800);
  else await page.keyboard.press("Escape");
  await page.waitForTimeout(700);
}

async function record(name, fn) {
  const raw = path.join(OUT, "raw");
  fs.mkdirSync(raw, { recursive: true });
  const browser = await chromium.launch({ headless: true, args: ["--force-color-profile=srgb"] });
  const ctx = await browser.newContext({
    viewport: VIEW,
    deviceScaleFactor: 2,
    recordVideo: { dir: raw, size: OUTSIZE },
    storageState: OUT + "/auth.json",
    colorScheme: "dark",
  });
  const page = await ctx.newPage();
  let err = null;
  try {
    await page.goto(process.env.TUT_APP, { waitUntil: "domcontentloaded", timeout: 45000 });
    await page.waitForTimeout(3500);
    await ensureCaption(page);
    await installCursor(page);
    await fn(page);
    await page.waitForTimeout(900);
  } catch (e) {
    err = e;
    try {
      await say(page, "— recording stopped: " + String(e.message).slice(0, 90), 1500);
    } catch {}
  }
  const video = page.video();
  await ctx.close();
  await browser.close();
  if (video) {
    const src = await video.path();
    const dest = path.join(raw, name + ".webm");
    fs.renameSync(src, dest);
    console.log((err ? "PARTIAL " : "OK      ") + name + " -> " + dest);
  }
  if (err) console.log("   reason: " + err.message.split("\n")[0]);
  return !err;
}

module.exports = { record, say, titleCard, clickIt, pointAt, openPanel, openManage, ensureCaption, tab, tryClick, closeDialog, reveal, chipOf, POPOVER, OUT };
