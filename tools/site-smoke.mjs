#!/usr/bin/env node
// Browser smoke test for the AgentStack docs site.
//
// Serves docs/ over HTTP (or hits BASE_URL) and, with Playwright + Chromium,
// checks every key page at phone and desktop widths for:
//   - zero console errors
//   - no document-level horizontal overflow
// plus tutorial-specific structure (lesson heading in view, exactly one
// lesson pane visible per nav button).
//
// Run in CI via:
//   npm i playwright@1.54.0 --no-save
//   npx playwright@1.54.0 install --with-deps chromium
//   SMOKE_SELF_TEST=overflow node tools/site-smoke.mjs   # must self-catch
//   node tools/site-smoke.mjs                            # the real run
//
// Exits nonzero and lists every failure as "page @ WxH : reason".

import http from "node:http";
import { readFile, stat } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const DOCS = path.resolve(__dirname, "..", "docs");

const PAGES = [
  "index.html",
  "start.html",
  "docs.html",
  "cookbook.html",
  "examples.html",
  "reference.html",
  "enforcement.html",
  "tutorial/",
];

const WIDTHS = [
  { w: 390, h: 844, label: "390x844" },
  { w: 1280, h: 900, label: "1280x900" },
];

const CONTENT_TYPES = {
  ".html": "text/html; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".jpg": "image/jpeg",
  ".gif": "image/gif",
  ".ico": "image/x-icon",
  ".json": "application/json; charset=utf-8",
  ".xml": "application/xml; charset=utf-8",
  ".woff": "font/woff",
  ".woff2": "font/woff2",
};

// --- inline static server over docs/ ---------------------------------------
function startServer() {
  return new Promise((resolve) => {
    const server = http.createServer(async (req, res) => {
      try {
        let urlPath = decodeURIComponent((req.url || "/").split("?")[0]);
        if (urlPath.endsWith("/")) urlPath += "index.html";
        // Resolve within DOCS, block traversal.
        const abs = path.normalize(path.join(DOCS, urlPath));
        if (!abs.startsWith(DOCS)) {
          res.writeHead(403).end("forbidden");
          return;
        }
        let target = abs;
        try {
          const s = await stat(abs);
          if (s.isDirectory()) target = path.join(abs, "index.html");
        } catch {
          /* fall through to readFile error */
        }
        const body = await readFile(target);
        const ext = path.extname(target).toLowerCase();
        res.writeHead(200, {
          "content-type": CONTENT_TYPES[ext] || "application/octet-stream",
        });
        res.end(body);
      } catch {
        res.writeHead(404).end("not found");
      }
    });
    server.listen(0, "127.0.0.1", () => {
      const { port } = server.address();
      resolve({ server, base: `http://127.0.0.1:${port}/` });
    });
  });
}

// --- assertions ------------------------------------------------------------
async function assertNoOverflow(page) {
  return page.evaluate(() => {
    const el = document.documentElement;
    return {
      scrollWidth: el.scrollWidth,
      innerWidth: window.innerWidth,
      overflow: el.scrollWidth > window.innerWidth + 1,
    };
  });
}

async function checkTutorial(page, widthLabel, failures) {
  // The initially-visible lesson (user-facing "lesson 1") heading must sit
  // within the viewport horizontally.
  const headingBox = await page.evaluate(() => {
    const pane = document.getElementById("lesson-0");
    const h = pane && pane.querySelector("h1, h2");
    if (!h) return null;
    const r = h.getBoundingClientRect();
    return { left: r.left, innerWidth: window.innerWidth };
  });
  if (!headingBox) {
    failures.push(`tutorial/ @ ${widthLabel} : lesson-1 heading not found`);
  } else if (!(headingBox.left >= 0 && headingBox.left < headingBox.innerWidth)) {
    failures.push(
      `tutorial/ @ ${widthLabel} : lesson-1 heading off-screen (left=${Math.round(headingBox.left)}, innerWidth=${headingBox.innerWidth})`,
    );
  }

  const navCount = await page.evaluate(
    () => document.querySelectorAll("#lessonNav .navbtn").length,
  );
  if (navCount !== 11) {
    failures.push(`tutorial/ @ ${widthLabel} : expected 11 nav buttons, found ${navCount}`);
    return;
  }

  for (let i = 0; i < 11; i++) {
    await page.evaluate((idx) => {
      document.querySelectorAll("#lessonNav .navbtn")[idx].click();
    }, i);
    // Let the pane-swap settle.
    await page.waitForTimeout(60);
    const vis = await page.evaluate(() => {
      const panes = Array.from(document.querySelectorAll(".lesson-pane"));
      const shown = panes.filter((p) => p.offsetParent !== null).map((p) => p.id);
      return { count: shown.length, ids: shown };
    });
    if (vis.count !== 1) {
      failures.push(
        `tutorial/ @ ${widthLabel} : nav ${i + 1} -> ${vis.count} lesson-panes visible (${vis.ids.join(",") || "none"}), expected exactly 1`,
      );
    } else if (vis.ids[0] !== `lesson-${i}`) {
      failures.push(
        `tutorial/ @ ${widthLabel} : nav ${i + 1} -> showed ${vis.ids[0]}, expected lesson-${i}`,
      );
    }
  }
}

// --- main ------------------------------------------------------------------
async function main() {
  const selfTest = process.env.SMOKE_SELF_TEST === "overflow";
  const failures = [];

  let playwright;
  try {
    playwright = await import("playwright");
  } catch (e) {
    console.error(
      "site-smoke: Playwright is not available. In CI run `npm i playwright@1.54.0 --no-save` and `npx playwright install chromium` first.",
    );
    console.error(String(e && e.message ? e.message : e));
    process.exit(2);
  }
  const { chromium } = playwright;

  let server = null;
  let base = process.env.BASE_URL;
  if (!base) {
    const started = await startServer();
    server = started.server;
    base = started.base;
  }
  if (!base.endsWith("/")) base += "/";

  const browser = await chromium.launch();

  // In self-test mode we exercise only the first page: inject a 2000px-wide
  // div, then verify the overflow assertion catches it. The whole point is to
  // prove the checker can fail — so a caught overflow means SUCCESS here.
  const pages = selfTest ? [PAGES[0]] : PAGES;
  const widths = selfTest ? [WIDTHS[0]] : WIDTHS;
  let selfTestCaught = false;

  for (const rel of pages) {
    for (const width of widths) {
      const context = await browser.newContext({
        viewport: { width: width.w, height: width.h },
      });
      const page = await context.newPage();
      const consoleErrors = [];
      page.on("console", (msg) => {
        if (msg.type() === "error") consoleErrors.push(msg.text());
      });
      page.on("pageerror", (err) => consoleErrors.push(String(err)));

      const url = base + rel;
      try {
        await page.goto(url, { waitUntil: "load", timeout: 30000 });
      } catch (e) {
        failures.push(`${rel} @ ${width.label} : navigation failed (${e.message})`);
        await context.close();
        continue;
      }

      if (selfTest) {
        await page.evaluate(() => {
          const d = document.createElement("div");
          d.style.cssText =
            "width:2000px;height:10px;position:relative;background:red";
          document.body.appendChild(d);
        });
      }

      const o = await assertNoOverflow(page);
      if (o.overflow) {
        const msg = `${rel} @ ${width.label} : horizontal overflow (scrollWidth=${o.scrollWidth} > innerWidth=${o.innerWidth})`;
        failures.push(msg);
        if (selfTest) selfTestCaught = true;
      }

      if (!selfTest && rel === "tutorial/") {
        await checkTutorial(page, width.label, failures);
      }

      if (consoleErrors.length) {
        failures.push(
          `${rel} @ ${width.label} : ${consoleErrors.length} console error(s): ${consoleErrors.slice(0, 3).join(" | ")}`,
        );
      }

      await context.close();
    }
  }

  await browser.close();
  if (server) await new Promise((r) => server.close(r));

  if (selfTest) {
    if (selfTestCaught) {
      console.log("site-smoke self-test: OK (injected 2000px overflow was caught)");
      process.exit(0);
    }
    console.error("site-smoke self-test FAILED: injected overflow was NOT caught");
    process.exit(1);
  }

  if (failures.length) {
    console.error(`\nsite-smoke: ${failures.length} failure(s):\n`);
    for (const f of failures) console.error(`  FAIL ${f}`);
    process.exit(1);
  }
  console.log("site-smoke: OK (all pages clean at both widths; tutorial nav verified)");
  process.exit(0);
}

main().catch((e) => {
  console.error("site-smoke: unexpected error:", e);
  process.exit(2);
});
