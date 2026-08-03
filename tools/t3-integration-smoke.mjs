#!/usr/bin/env node
/**
 * Cross-repository AgentStack ↔ T3 Code smoke test.
 *
 * Prerequisites:
 *   cargo build --release
 *   # T3 dependencies already installed
 *   npm i playwright@1.54.0 --no-save --no-package-lock
 *   npx playwright@1.54.0 install chromium
 *
 * Run from this repository:
 *   T3CODE_REPO=/path/to/t3code node tools/t3-integration-smoke.mjs
 *
 * The real AgentStack binary is used by T3's bridge E2E suite. The browser
 * half uses a deterministic protocol fixture so it can force both advertised
 * and withheld feature contracts without changing either repository or the
 * user's AgentStack/T3 state.
 */

import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { access, chmod, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const AGENTSTACK_REPO = path.resolve(HERE, "..");
const T3CODE_REPO = path.resolve(
  process.env.T3CODE_REPO || path.join(AGENTSTACK_REPO, "..", "t3code"),
);
const AGENTSTACK_BIN = path.resolve(
  process.env.AGENTSTACK_BIN || path.join(AGENTSTACK_REPO, "target", "release", "agentstack"),
);
const ANSI_SGR = new RegExp(`${String.fromCharCode(27)}\\[[0-9;]*m`, "g");

async function requirePath(target, message) {
  try {
    await access(target);
  } catch {
    throw new Error(`${message}: ${target}`);
  }
}

function run(command, args, options = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      cwd: options.cwd,
      env: options.env,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk) => {
      stdout += String(chunk);
      if (options.echo) process.stdout.write(chunk);
    });
    child.stderr.on("data", (chunk) => {
      stderr += String(chunk);
      if (options.echo) process.stderr.write(chunk);
    });
    child.once("error", reject);
    child.once("exit", (code, signal) => {
      if (code === 0) {
        resolve({ stdout, stderr });
      } else {
        reject(
          new Error(
            `${command} exited ${code ?? signal ?? "without a status"}\n${stderr || stdout}`,
          ),
        );
      }
    });
  });
}

function waitForPairing(child) {
  return new Promise((resolve, reject) => {
    let output = "";
    const timeout = setTimeout(() => {
      reject(new Error(`T3 did not print a pairing URL.\n${output}`));
    }, 45_000);
    const inspect = (chunk) => {
      output += String(chunk).replaceAll(ANSI_SGR, "");
      const match = /pairingUrl:\s*(http:\/\/\S+)/.exec(output);
      if (match) {
        clearTimeout(timeout);
        resolve(match[1]);
      }
    };
    child.stdout.on("data", inspect);
    child.stderr.on("data", inspect);
    child.once("error", (error) => {
      clearTimeout(timeout);
      reject(error);
    });
    child.once("exit", (code, signal) => {
      clearTimeout(timeout);
      reject(
        new Error(
          `T3 exited before publishing a pairing URL (${code ?? signal ?? "unknown status"}).\n${output}`,
        ),
      );
    });
  });
}

async function stopExactChild(child) {
  if (child.exitCode !== null || child.signalCode !== null) return;
  // The dev runner owns Vite+ workers. A detached process group lets cleanup
  // signal exactly this captured tree without matching or touching any other
  // T3 process on the machine.
  process.kill(-child.pid, "SIGINT");
  await Promise.race([
    new Promise((resolve) => child.once("exit", resolve)),
    new Promise((resolve) => setTimeout(resolve, 5_000)),
  ]);
  if (child.exitCode === null && child.signalCode === null) {
    process.kill(-child.pid, "SIGTERM");
    await Promise.race([
      new Promise((resolve) => child.once("exit", resolve)),
      new Promise((resolve) => setTimeout(resolve, 5_000)),
    ]);
  }
  child.stdout.destroy();
  child.stderr.destroy();
}

async function clickUnique(page, name) {
  const locator = page.getByRole("button", { name, exact: true });
  assert.equal(await locator.count(), 1, `expected one "${name}" button`);
  await locator.click();
}

async function openManage(page) {
  // The chevron in the button's label is aria-hidden in T3's PopoverHome, so
  // the accessible name is just "Manage" — matching on the decoration never
  // resolves.
  const manage = page.getByRole("button", { name: "Manage", exact: true });
  // Some successful inline flows deliberately leave the popover open. Treat
  // this helper as "ensure Manage is open", not as a blind toggle that can
  // close an already-open popover.
  if (!(await manage.isVisible())) await clickUnique(page, "AgentStack");
  await manage.waitFor({ state: "visible" });
  assert.equal(await manage.count(), 1, 'expected one "Manage" button');
  await manage.click();
  await page.getByRole("dialog", { name: "Manage AgentStack" }).waitFor({ state: "visible" });
}

async function setMode(modeFile, page, mode) {
  await writeFile(modeFile, `${mode}\n`, "utf8");
  await page.reload();
  await page.getByRole("button", { name: "AgentStack", exact: true }).waitFor({ state: "visible" });
}

function fixtureScript() {
  return `#!/usr/bin/env node
const fs = require("node:fs");
const args = process.argv.slice(2);
const mode = fs.readFileSync(process.env.AGENTSTACK_E2E_MODE_FILE, "utf8").trim();
const has = (...parts) => parts.every((part) => args.includes(part));
const out = (value) => process.stdout.write((typeof value === "string" ? value : JSON.stringify(value)) + "\\n");
const digest = "sha256:" + "a".repeat(64);
const fullFeatures = [
  "init-plan", "apply-setup", "restore-last", "trust-preview", "trust-consent",
  "sessions-v1", "profiles-v1", "profiles-edit-v1", "profiles-edit-batch-v1",
  "toolset-rename-v1", "toolset-delete-v1", "toolset-create-v2", "library-remove-v1",
  "manifest-remove-v1", "doctor-advisories-v1", "doctor-probe-v1", "doctor-mode-v1",
  "doctor-cli-coverage-v1", "gitignore-opt-out-v1", "set-mode-v1", "status-honesty-v1",
  "diff-v1", "diff-ownership-v1", "activity-skill-load-v1", "workflow-observe-v1",
  "workflow-serial-roles-v1", "trust-server-blockers-v1", "trust-review-card-v1",
  "trust-card-diff-v1"
];
const limitedFeatures = fullFeatures.filter((feature) =>
  !["doctor-probe-v1", "workflow-serial-roles-v1", "diff-v1", "set-mode-v1"].includes(feature)
);

if (args.includes("--version")) {
  out("agentstack 0.17.0 (sandbox: no)");
} else if (has("delete-profile", "--preview") && args.includes("only")) {
  process.stderr.write("error: won't delete 'only' — it is the only toolset here\\n");
  process.exitCode = 1;
} else if (has("doctor", "--probe", "--json")) {
  out({ errors: 0, warnings: 0, state: "ready", sections: [], probe: {
    ran: false, skipped_reason: "drifted", servers: []
  }, schema_version: 1, features: fullFeatures });
} else if (has("doctor", "--json")) {
  const needsSetup = mode === "needs_setup";
  const drifted = mode === "drift";
  out({
    errors: 0,
    warnings: drifted ? 1 : 0,
    advisories: 0,
    state: needsSetup ? "needs_setup" : drifted ? "needs_attention" : "ready",
    readiness: needsSetup ? "needs_setup" : drifted ? "drifted" : "ready",
    trust: drifted ? "drifted" : "trusted",
    mode: needsSetup ? null : "static",
    activation: needsSetup ? null : "locked",
    gitignore: needsSetup ? null : true,
    clis: needsSetup ? null : { detected: 3, bridge_capable: 2, bridge_incapable: ["legacy-cli"] },
    sections: drifted ? [{ title: "Drift", lines: [{
      level: "warn", msg: "Claude Code edited on disk since last apply"
    }] }] : [],
    schema_version: 1,
    features: mode === "no_contracts" ? limitedFeatures : fullFeatures
  });
} else if (has("init", "--plan")) {
  out({
    path: "/fixture/project",
    manifest_path: "/fixture/project/.agentstack/agentstack.toml",
    already_initialized: false,
    detected: [{ id: "claude-code", display: "Claude Code", bin_on_path: true,
      configs: ["/fixture/home/.claude.json"] }],
    servers: [{ name: "github", kind: "http", target: "https://api.example/mcp" }],
    settings_from: ["claude-code"],
    conflicts: [{ name: "github", other_definitions: 1 }],
    secrets: [{ reference: "GITHUB_TOKEN", origin: "claude-code:github" }],
    secrets_destination: args[args.indexOf("--secrets") + 1] || "env",
    destinations: [{ id: "claude-code", display: "Claude Code", scope: "project",
      path: "/fixture/project/.mcp.json", writes: ["MCP servers"] }],
    plan_digest: digest,
    schema_version: 1,
    features: fullFeatures
  });
} else if (has("trust", "--preview")) {
  out({
    path: "/fixture/project", state: "drifted", re_trust: true,
    servers: [{ name: "docs", kind: "http", target: "https://docs.example/mcp",
      runs: [], contacts: ["docs.example"], may_read: [], pin: null, prior_pin: null,
      recognized_other_projects: 0, diff: null }],
    server_blockers: [], secrets: ["GITHUB_TOKEN"],
    counts: { skills: 1, workflows: 1, extensions: 0, instructions: 1, hooks: 0, settings: 0 },
    skills: ["review"], workflows: [{ name: "serial-build", roles: ["builder"] }],
    extensions: [], instructions: ["project-guidance"], hooks: [], settings: [],
    policy_requested: ["docs: network allow docs.example"], machine_policy_ceiling: "/fixture/policy.toml",
    surface_digest: digest, schema_version: 1, features: fullFeatures
  });
} else if (has("diff", "--json")) {
  const scope = args[args.indexOf("--scope") + 1] || "project";
  out({ scope, drifted: 1, targets: [{ id: "claude-code", display: "Claude Code",
    path: scope === "project" ? "/fixture/project/.mcp.json" : "/fixture/home/.claude.json",
    changed: true, hand_edited: true, existed_before: true, kept: [],
    diff: "--- disk\\n+++ manifest\\n@@ -1 +1 @@\\n-old\\n+new\\n" }], warnings: [],
    schema_version: 1, features: fullFeatures });
} else if (has("workflow", "report")) {
  out({ run: args[args.indexOf("report") + 1], workflow: "serial-build", outcome: "completed",
    exhausted: false, duration_ms: 1250, max_agents: 4, max_wall_seconds: 600,
    steps: [{ step: 1, role: "builder", label: "verify:tests", state: "completed",
      outcome: "ok", tool_calls: 2, duration_ms: 1250, taint: [] }] });
} else if (has("workflow", "runs", "--json")) {
  out({ runs: [{ run: "w-abc123", workflow: "serial-build", outcome: "completed",
    exhausted: false, resumable: false, started_unix: 2000000000, duration_ms: 1250, steps: 1 }] });
} else if (has("workflow", "list", "--json")) {
  out({ workflows: [{ name: "serial-build", declared: true, trusted: true,
    lock_status: "matches", roles: ["builder"], serial_roles: ["builder"],
    max_agents: 4, max_wall_seconds: 600 }], schema_version: 1,
    features: mode === "no_contracts" ? ["workflow-observe-v1"] : fullFeatures });
} else if (has("report", "calls", "--json")) {
  out({ events: mode === "rich" ? [
    { ts: 2000000000, server: "github", tool: "search", outcome: "ok", ms: 12,
      args_digest: "abcdef1234567890", kind: "call" },
    { ts: 2000000001, server: "guard", tool: "shell", outcome: "denied", ms: 0,
      detail: "blocked by network policy", kind: "call" },
    { ts: 2000000002, name: "review", reason: "inspect the patch", kind: "skill_load" }
  ] : [] });
} else if (has("use", "--list", "--json")) {
  const profiles = mode === "rich" || mode === "drift" ? [
    { name: "backend", skills: ["review"], servers: ["docs"], pinned: true, active: true, blockers: [] },
    { name: "frontend", skills: [], servers: ["github"], pinned: true, active: false, blockers: [] }
  ] : [{ name: "only", skills: [], servers: [], pinned: true, active: true, blockers: [] }];
  out({ path: "/fixture/project", trust: "trusted", profiles, session: null,
    schema_version: 1, features: fullFeatures });
} else if (args.includes("library-index")) {
  out({ skills: mode === "rich" ? [{ name: "review", description: "Review code safely",
    origin: "library", in_manifest: true }] : [],
    servers: mode === "rich" ? [
      { name: "docs", provenance: null, origin: "manifest", in_manifest: true },
      { name: "github", provenance: "consolidated:github", origin: "library", in_manifest: true }
    ] : [], profiles: mode === "rich" ? ["backend", "frontend"] : ["only"],
    schema_version: 1, features: fullFeatures });
} else if (has("restore", "--json")) {
  out({ entries: mode === "rich" ? [{ id: "b".repeat(64), short_id: "bbbbbbbb",
    time_unix: 2000000000, scope: "project", operation: "apply", summary: "1 file · claude-code",
    undone: false, touches_project: true }] : [], adapter_backups: [],
    schema_version: 1, features: fullFeatures });
} else if (args.includes("--preview")) {
  const action = args.find((arg) => ["create-profile", "edit-profile", "rename-profile",
    "set-mode", "set-gitignore", "add-skill-to-profile", "add-server-to-profile",
    "remove-from-library", "remove-capability"].includes(arg)) || "edit-profile";
  const targetMode = action === "set-mode" ? args[args.indexOf("set-mode") + 1] : undefined;
  out({ action, profile: "web", consent_digest: digest, note: "fixture preview",
    ...(targetMode ? { mode: targetMode, current_mode: "static", changed: true,
      removes: [{ label: "Claude Code config", path: "/fixture/project/.mcp.json" }],
      locks: targetMode === "clean-at-rest", machine_scope: targetMode === "zero-files",
      bridge: targetMode === "zero-files" ? { registers: true, detected: 3, capable: 2,
        incapable: ["legacy-cli"] } : null, undo: "agentstack restore --last --write" } : {}),
    schema_version: 1, features: fullFeatures });
} else {
  out("fixture write completed");
}
`;
}

async function main() {
  await requirePath(path.join(T3CODE_REPO, "package.json"), "T3 Code repository not found");
  await requirePath(
    path.join(T3CODE_REPO, "node_modules", ".bin", "vp"),
    "T3 Code dependencies are not installed",
  );
  await requirePath(AGENTSTACK_BIN, "AgentStack release binary not found; run cargo build --release");

  let playwright;
  try {
    playwright = await import("playwright");
  } catch {
    throw new Error(
      "Playwright is not installed. Run `npm i playwright@1.54.0 --no-save --no-package-lock` and `npx playwright@1.54.0 install chromium`.",
    );
  }

  console.log("t3-integration-smoke: real AgentStack bridge");
  await run(
    path.join(T3CODE_REPO, "node_modules", ".bin", "vp"),
    ["test", "run", "src/agentstack/AgentstackCli.e2e.test.ts"],
    {
      cwd: path.join(T3CODE_REPO, "apps", "server"),
      env: { ...process.env, T3CODE_AGENTSTACK_BIN: AGENTSTACK_BIN },
      echo: true,
    },
  );

  const scratch = await mkdtemp(path.join(os.tmpdir(), "agentstack-t3-smoke-"));
  const modeFile = path.join(scratch, "mode");
  const fixture = path.join(scratch, "agentstack-fixture");
  await writeFile(modeFile, "needs_setup\n", "utf8");
  await writeFile(fixture, fixtureScript(), "utf8");
  await chmod(fixture, 0o755);

  const t3 = spawn(
    process.execPath,
    [
      "scripts/dev-runner.ts",
      "--home-dir",
      path.join(scratch, "t3-home"),
      "--auto-bootstrap-project-from-cwd",
      "dev",
    ],
    {
      cwd: T3CODE_REPO,
      env: {
        ...process.env,
        HOME: path.join(scratch, "home"),
        AGENTSTACK_HOME: path.join(scratch, "agentstack-home"),
        AGENTSTACK_E2E_MODE_FILE: modeFile,
        T3CODE_AGENTSTACK_BIN: fixture,
      },
      stdio: ["ignore", "pipe", "pipe"],
      detached: true,
    },
  );

  let browser;
  try {
    const pairingUrl = await waitForPairing(t3);
    browser = await playwright.chromium.launch();
    const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
    const consoleErrors = [];
    page.on("console", (message) => {
      if (message.type() === "error") consoleErrors.push(message.text());
    });
    await page.goto(pairingUrl);
    await page.getByRole("button", { name: "AgentStack", exact: true }).waitFor({
      state: "visible",
    });

    console.log("t3-integration-smoke: F1 setup posture");
    await clickUnique(page, "AgentStack");
    const needsYou = page.getByText("Needs you", { exact: true });
    await needsYou.waitFor({ state: "visible" });
    assert.equal(await needsYou.count(), 1);

    console.log("t3-integration-smoke: F1a setup plan and bound apply");
    await clickUnique(page, "Review setup");
    const setupDialog = page.getByRole("dialog", { name: "Set up this project" });
    await setupDialog.waitFor({ state: "visible" });
    await setupDialog.getByText("GITHUB_TOKEN", { exact: true }).waitFor({ state: "visible" });
    assert.equal(await setupDialog.getByText("Claude Code", { exact: true }).count(), 1);
    assert.equal(await setupDialog.getByText("Defined more than once", { exact: true }).count(), 1);
    await setupDialog.getByRole("button", { name: /^Don't store yet/ }).click();
    const updatingPlan = setupDialog.getByText("Updating the plan…", { exact: true });
    await updatingPlan.waitFor({ state: "visible" });
    await updatingPlan.waitFor({ state: "hidden" });
    await setupDialog.getByText(/Values you'll still provide/).waitFor({ state: "visible" });
    await setupDialog.getByRole("button", { name: "Set up this project", exact: true }).click();
    await setupDialog.waitFor({ state: "hidden" });

    console.log("t3-integration-smoke: F2 CLI refusal projection");
    await setMode(modeFile, page, "ready");
    await openManage(page);
    await clickUnique(page, "Toolsets");
    await clickUnique(page, "Delete");
    await page.getByText("Nothing was changed.", { exact: true }).waitFor({ state: "visible" });
    assert.equal(
      await page.getByText("won't delete 'only' — it is the only toolset here", {
        exact: true,
      }).count(),
      1,
    );
    assert.equal(await page.getByText(/update agentstack/i).count(), 0);

    console.log("t3-integration-smoke: F3 startup probe");
    await setMode(modeFile, page, "ready");
    await openManage(page);
    await clickUnique(page, "Test server startup");
    assert.equal(await page.getByText(/starts every stdio server/).count(), 1);
    assert.equal(await page.getByText(/Nothing is written/).count(), 1);
    await clickUnique(page, "Start them");
    await page
      .getByText(
        "Nothing was started — the manifest or lockfile changed since this project was trusted. Review this project again.",
        { exact: true },
      )
      .waitFor({ state: "visible" });
    assert.equal(await page.getByRole("button", { name: "Test server startup" }).count(), 0);
    await clickUnique(page, "Review this project");
    const trustDialog = page.getByRole("dialog", { name: "Review this project" });
    await trustDialog.waitFor({ state: "visible" });
    await trustDialog.getByText("docs", { exact: true }).waitFor({ state: "visible" });
    assert.equal(await trustDialog.getByText("GITHUB_TOKEN", { exact: true }).count(), 1);
    assert.equal(await trustDialog.getByText("review", { exact: true }).count(), 1);

    console.log("t3-integration-smoke: F4 serial workflow projection");
    await setMode(modeFile, page, "rich");
    await openManage(page);
    await clickUnique(page, "Activity");
    assert.equal(
      await page.getByText(
        "builder runs one child at a time — that harness takes no per-child MCP config, so the ≤4 ceiling doesn't apply to it.",
        { exact: true },
      ).count(),
      1,
    );

    console.log("t3-integration-smoke: F5 calls, skill loads, and workflow history");
    assert.equal(await page.getByText("github__search", { exact: true }).count(), 1);
    assert.equal(await page.getByText("guard__shell", { exact: true }).count(), 1);
    assert.equal(await page.getByText("blocked by network policy", { exact: true }).count(), 1);
    assert.equal(await page.getByText("review", { exact: true }).count(), 1);
    assert.equal(await page.getByText("skill loaded", { exact: true }).count(), 1);
    const recordedRun = page.getByRole("button", { name: /serial-build.*w-abc123/ });
    assert.equal(await recordedRun.count(), 1);
    await recordedRun.click();
    const runDialog = page.getByRole("dialog", { name: "serial-build" });
    await runDialog.waitFor({ state: "visible" });
    await runDialog.getByText("verify:tests", { exact: true }).waitFor({ state: "visible" });

    console.log("t3-integration-smoke: F6 delivery-mode preview and apply");
    await setMode(modeFile, page, "rich");
    await clickUnique(page, "AgentStack");
    const onDisk = page.getByRole("button", { name: "on disk", exact: true });
    await onDisk.waitFor({ state: "visible" });
    await onDisk.click();
    assert.equal(await page.getByText("HOW CAPABILITIES REACH YOUR CLIS", { exact: true }).count(), 1);
    assert.equal(await page.getByRole("button", { name: /Served live/ }).count(), 1);
    assert.equal(await page.getByRole("button", { name: /Only while you work/ }).count(), 1);
    await page.getByRole("button", { name: /Only while you work/ }).click();
    await page.getByText("pin agentstack.lock — sessions activate from it", { exact: true }).waitFor({
      state: "visible",
    });
    await clickUnique(page, "Switch to only while you work");
    await page.getByText("HOW CAPABILITIES REACH YOUR CLIS", { exact: true }).waitFor({
      state: "hidden",
    });

    console.log("t3-integration-smoke: F7 toolset creation and library consent");
    await setMode(modeFile, page, "rich");
    await openManage(page);
    await clickUnique(page, "Toolsets");
    await page.getByText("backend", { exact: true }).waitFor({ state: "visible" });
    assert.equal(await page.getByText("frontend", { exact: true }).count(), 1);
    await page.getByText("Review code safely", { exact: true }).waitFor({ state: "visible" });
    await clickUnique(page, "+ New");
    await page.getByPlaceholder("Name it, e.g. web").fill("web");
    await page.getByRole("switch", { name: "Include review in this toolset" }).click();
    await clickUnique(page, "Create");
    await page.getByText('New toolset "web" with 1 skill', { exact: true }).waitFor({
      state: "visible",
    });
    await clickUnique(page, "Confirm");
    await page.getByText('Toolset "web" created', { exact: true }).waitFor({ state: "visible" });
    assert.equal(await page.getByText("Written and locked", { exact: true }).count(), 1);

    console.log("t3-integration-smoke: F8 exact-id undo");
    await setMode(modeFile, page, "rich");
    await openManage(page);
    await clickUnique(page, "Undo a change…");
    await page.getByText("Recorded changes", { exact: true }).waitFor({ state: "visible" });
    await clickUnique(page, "Revert");
    await clickUnique(page, "Revert this change");
    await page.getByText("Undone", { exact: true }).waitFor({ state: "visible" });

    console.log("t3-integration-smoke: F9 project and machine drift decisions");
    await setMode(modeFile, page, "drift");
    await openManage(page);
    await page.getByRole("dialog", { name: "Manage AgentStack" }).getByRole("button", {
      name: "Review",
      exact: true,
    }).first().click();
    const driftDialog = page.getByRole("dialog", { name: "Review drift" });
    await driftDialog.waitFor({ state: "visible" });
    const keepProjectEdits = driftDialog.getByRole("button", {
      name: "Keep edits — This project",
    });
    const rerenderGlobal = driftDialog.getByRole("button", {
      name: "Re-render — Machine-wide configs",
    });
    await keepProjectEdits.waitFor({ state: "visible" });
    await rerenderGlobal.waitFor({ state: "visible" });
    await keepProjectEdits.click();
    await driftDialog.getByText("Done", { exact: true }).waitFor({ state: "visible" });

    console.log("t3-integration-smoke: feature contracts fail closed");
    await setMode(modeFile, page, "no_contracts");
    await openManage(page);
    assert.equal(await page.getByRole("button", { name: "Test server startup" }).count(), 0);
    await clickUnique(page, "Activity");
    assert.equal(await page.getByText(/runs one child at a time/).count(), 0);

    assert.deepEqual(consoleErrors, [], `browser console errors:\n${consoleErrors.join("\n")}`);
    console.log(
      "t3-integration-smoke: OK (4 real CLI E2E journeys + 9 browser journeys + fail-closed gates)",
    );
  } finally {
    if (browser) await browser.close();
    await stopExactChild(t3);
    await rm(scratch, { recursive: true, force: true });
  }
}

main().catch((error) => {
  console.error("t3-integration-smoke: FAILED");
  console.error(error);
  process.exitCode = 1;
});
