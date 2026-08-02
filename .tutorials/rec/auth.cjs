// Pair ONCE with a fresh one-time token, then persist the session so every
// scenario recording can open an already-paired context.
const { chromium } = require("/Users/tarek.k/Documents/GitHub/agentstack/node_modules/playwright");
const fs = require("node:fs");

const OUT = process.env.TUT_OUT;
const PAIR = process.env.TUT_PAIR;

(async () => {
  fs.mkdirSync(OUT, { recursive: true });
  const b = await chromium.launch({ headless: true });
  const c = await b.newContext({ viewport: { width: 1280, height: 800 }, colorScheme: "dark" });
  const p = await c.newPage();
  await p.goto(PAIR, { waitUntil: "domcontentloaded" });
  await p.waitForTimeout(6000);
  const text = await p.innerText("body");
  if (/Pair with this environment/i.test(text) && /Invalid|Enter a pairing token/i.test(text)) {
    console.log("FAILED to pair. Page says:", text.slice(0, 200).replace(/\n+/g, " | "));
    await b.close();
    process.exit(1);
  }
  await c.storageState({ path: OUT + "/auth.json" });
  console.log("PAIRED. state saved ->", OUT + "/auth.json");
  console.log("landed on:", p.url());
  await b.close();
})();
