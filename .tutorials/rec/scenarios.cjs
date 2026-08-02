const L = require("./lib.cjs");
const { record, say, titleCard, clickIt, openPanel, openManage, tab, tryClick, closeDialog, reveal } = L;

const POP = '[role="dialog"]';
const DLG = '[role="dialog"]';

// Which scenarios to run: node scenarios.cjs 01 03  (default: all)
const pick = process.argv.slice(2);
const want = (id) => pick.length === 0 || pick.includes(id);

const S = [];
const scenario = (id, title, subtitle, fn) => S.push({ id, title, subtitle, fn });

// ── 01 ───────────────────────────────────────────────────────────────────────
scenario("01-where-things-stand", "Where things stand", "The chip that tells you if your setup is ready", async (page) => {
  await say(page, "Every thread header carries one AgentStack chip.", 2600);
  await say(page, "Its word is the whole status: Ready, or Needs you.", 2800);
  await openPanel(page);
  await say(page, "One glance: what changed, and the single next step.", 3200);
  await say(page, "Nothing is running behind your back — content that changed is inert until you look at it.", 3600);
  await say(page, "Two ways in: review it now, or open Manage for the detail.", 3000);
});

// ── 02 ───────────────────────────────────────────────────────────────────────
scenario("02-the-yes", "The yes", "Reviewing exactly what you are approving", async (page) => {
  await openPanel(page);
  await say(page, "This project pinned some content, and that content changed on disk.", 3400);
  await say(page, "So it went inert again. That is the rule: a byte changes, you review again.", 3600);
  const review = page.locator(POP + " button", { hasText: /Review this project/i }).first();
  await tryClick(page, review, "The review entry is not on this popover.");
  await say(page, "The card opens with what this project would be allowed to run here.", 3800);
  await say(page, "The exact command, spelled out — nothing hidden behind a name.", 3600);
  await reveal(page, 300);
  await say(page, "Then every skill, each with its own line and its pin state.", 3600);
  await reveal(page, 320);
  await say(page, "Where the bytes changed since your last yes, it says so — and says there is no approved snapshot left to compare against.", 5000);
  await reveal(page, 340);
  await say(page, "Your yes covers the whole list at once. There is no per-item opt-out,", 3800);
  await say(page, "because you approve exactly what you reviewed — to leave something out, remove it and review again.", 4600);
  await reveal(page, 320);
  await say(page, "And the footer counts it plainly: one server, one command it runs, four skills.", 4200);
  await say(page, "Approve, or close and leave it exactly as inert as it was.", 3600);
  await closeDialog(page);
});

// ── 03 ───────────────────────────────────────────────────────────────────────
scenario("03-status", "Status", "Is it ready, and what is the one next action", async (page) => {
  await openPanel(page);
  await openManage(page);
  await tab(page, "Status");
  await say(page, "Status answers one question: is this project ready?", 3200);
  await say(page, "When it is not, it names the single next action — never a list of findings with no path.", 4400);
  await reveal(page, 260);
  await say(page, "Under that: what is live, and what changed since it was last reviewed.", 3800);
  await reveal(page, 300);
  await say(page, "Drift is listed per CLI, in plain words — what no longer matches, and what would be removed.", 4600);
  await reveal(page, 320);
  await say(page, "Whether secrets resolve, and how the setup is delivered.", 3600);
  await say(page, "Files on disk here — the default. Each CLI reads a config AgentStack writes.", 4200);
});

// ── 04 ───────────────────────────────────────────────────────────────────────
scenario("04-drift", "Drift", "Your edits versus the manifest — you pick the truth", async (page) => {
  await openPanel(page);
  await openManage(page);
  await tab(page, "Status");
  await say(page, "Status flags it: your coding tool configs changed outside AgentStack.", 3600);
  // The drift section has its own Review button — distinct from the trust card.
  const driftReview = page
    .locator(DLG + " *", { hasText: /^Drift$/ })
    .locator("xpath=following::button[normalize-space()='Review'][1]")
    .first();
  const opened = await tryClick(page, driftReview, "");
  if (!opened) {
    const anyReview = page.locator(DLG + " button", { hasText: /^Review$/ }).last();
    await tryClick(page, anyReview, "");
  }
  await say(page, "When a config changes outside AgentStack, nothing is silently overwritten.", 3800);
  await say(page, "You are shown both truths and asked which one to keep.", 3400);
  await say(page, "Keep edits pulls what is on disk back into the project.", 3200);
  await say(page, "Re-render writes the project's version back out — and it is undoable.", 3600);
  await say(page, "Machine-wide files are called out separately: a change there is not scoped to this repo.", 4400);
  await reveal(page, 300);
  await say(page, "Every affected file is named, with how many lines moved either way.", 4200);
  await reveal(page, 300);
  await say(page, "Seven coding tools here — you see all of them before you choose.", 4000);
  await closeDialog(page);
});

// ── 05 ───────────────────────────────────────────────────────────────────────
scenario("05-toolsets", "Toolsets", "Name what a task needs, switch in one click", async (page) => {
  await openPanel(page);
  await openManage(page);
  await tab(page, "Toolsets");
  await say(page, "A toolset is a named bundle of the skills and servers one task needs.", 3800);
  await say(page, "The rail lists yours, with what each one holds.", 3000);
  await say(page, "Use switches to it — your CLIs pick it up.", 3200);
  await say(page, "Creating one never activates it. Naming and using stay separate.", 3600);
});

// ── 06 ───────────────────────────────────────────────────────────────────────
scenario("06-library", "The library", "Reuse capabilities across every project", async (page) => {
  await openPanel(page);
  await openManage(page);
  await tab(page, "Toolsets");
  await say(page, "On the right is your library — skills and servers you can reuse anywhere.", 3800);
  const filter = page.locator(DLG + ' input[type="search"], ' + DLG + " input").first();
  if (await filter.isVisible().catch(() => false)) {
    await clickIt(page, filter, 400);
    await filter.type("testing", { delay: 110 });
    await say(page, "Filter to find one fast.", 2400);
    await filter.fill("");
    await page.waitForTimeout(700);
  }
  await say(page, "Add puts a capability into a toolset — it asks which one, then confirms before writing.", 4200);
  const add = page.locator(DLG + " button", { hasText: /^Add$/ }).first();
  await tryClick(page, add, "");
  await say(page, "It asks where it should go.", 2600);
  const dest = page.locator(DLG + " button", { hasText: /^rust$/i }).first();
  await tryClick(page, dest, "");
  await say(page, "Then one confirm — it re-locks and re-renders, and says so before it writes anything.", 4200);
  await say(page, "And when it lands, the toolset updates immediately.", 3000);
  await closeDialog(page);
});

// ── 07 ───────────────────────────────────────────────────────────────────────
scenario("07-activity", "Activity", "What the agents actually did", async (page) => {
  await openPanel(page);
  await openManage(page);
  await tab(page, "Activity");
  await say(page, "Activity is the honest record: what ran, what it reached, what was refused.", 4200);
  await say(page, "These are real guard denials — writes outside the workspace, stopped before they happened.", 4600);
  await reveal(page, 280);
  await say(page, "Each line names the tool, the reason, and how long ago.", 3800);
  await reveal(page, 300);
  await say(page, "Tool arguments are recorded as digests, never values — secrets stay out of the log.", 4400);
  await reveal(page, 300);
  await say(page, "Below the calls: workflow runs — none declared in this project.", 3800);
  await say(page, "When an agent loads a skill on demand, that lands here too, with the reason it gave.", 4400);
});

// ── 08 ───────────────────────────────────────────────────────────────────────
scenario("08-protection", "More protection", "Stronger modes, and sharing a setup", async (page) => {
  await openPanel(page);
  await openManage(page);
  await tab(page, "Protection");
  await say(page, "Stronger modes live behind one door, so they never crowd the everyday path.", 4200);
  await say(page, "Normal setup already fails closed. These layers add checks on top.", 4000);
  await say(page, "The guard blocks destructive commands before they run.", 3600);
  await reveal(page, 150);
  await say(page, "The machine policy is a ceiling every project runs under — it can only narrow.", 4400);
  await reveal(page, 170);
  await say(page, "A locked run pins content and records evidence for that one run.", 3800);
  await reveal(page, 150);
  await say(page, "The sandbox adds container isolation; lockdown also enforces the network route.", 4400);
  await reveal(page, 220);
  await say(page, "And sharing is signing — a bundle others review before anything activates.", 4200);
  await say(page, "The panel shows the exact commands. The terminal stays the authority.", 4000);
});

// ── 09 ───────────────────────────────────────────────────────────────────────
scenario("09-undo", "Undo", "Every material change is reversible", async (page) => {
  await openPanel(page);
  await openManage(page);
  await tab(page, "Status");
  await say(page, "Every change AgentStack writes is recorded — so it can be taken back.", 3600);
  // Bring the action row into view, then open the ledger, then scroll again:
  // "Recorded changes" renders below the fold of the dialog.
  const dlg = page.locator(DLG).first();
  await dlg.hover().catch(() => {});
  await page.mouse.wheel(0, 700);
  await page.waitForTimeout(900);
  await say(page, "At the foot of Status: Undo a change.", 2800);
  const undo = page.locator(DLG + " button", { hasText: /Undo a change/i }).first();
  await tryClick(page, undo, "");
  await page.waitForTimeout(1500);
  await page.mouse.wheel(0, 700);
  await page.waitForTimeout(1200);
  await say(page, "The recorded writes, newest first — what changed, how many files, and when.", 4000);
  await say(page, "Here is the mode switch that re-rendered thirteen files across every CLI.", 4200);
  await page.mouse.wheel(0, 240);
  await say(page, "Each entry says whether it can be reverted from here.", 3400);
  await say(page, "Changes made elsewhere on this machine are shown but not undone from this panel —", 4000);
  await say(page, "and one already undone is labelled so, because a revert is itself a recorded change.", 4200);
  await say(page, "The full ledger, and every revert, lives in the terminal: agentstack undo", 4000);
});

(async () => {
  for (const s of S) {
    if (!want(s.id.slice(0, 2))) continue;
    await record(s.id, async (page) => {
      await titleCard(page, s.title, s.subtitle);
      await s.fn(page);
      await say(page, "", 600);
    });
  }
  process.exit(0);
})();
