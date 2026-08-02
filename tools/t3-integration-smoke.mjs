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
  await clickUnique(page, "AgentStack");
  // The chevron in the button's label is aria-hidden in T3's PopoverHome, so
  // the accessible name is just "Manage" — matching on the decoration never
  // resolves.
  const manage = page.getByRole("button", { name: "Manage", exact: true });
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
  return `#!/bin/sh
mode=$(cat "$AGENTSTACK_E2E_MODE_FILE")

case " $* " in
  *" --version "*)
    printf '%s\\n' 'agentstack 0.17.0 (sandbox: no)'
    ;;
  *" delete-profile "*" --preview "*)
    printf '%s\\n' "error: won't delete 'only' — it is the only toolset here" >&2
    exit 1
    ;;
  *" doctor --probe --json "*)
    printf '%s\\n' '{"errors":0,"warnings":0,"state":"ready","sections":[],"probe":{"ran":false,"skipped_reason":"drifted","servers":[]},"schema_version":1,"features":["doctor-advisories-v1","doctor-probe-v1"]}'
    ;;
  *" doctor --json "*)
    if [ "$mode" = "needs_setup" ]; then
      state=needs_setup
    else
      state=ready
    fi
    if [ "$mode" = "no_contracts" ]; then
      features='["apply-setup","restore-last","trust-consent","sessions-v1","profiles-v1","profiles-edit-v1","profiles-edit-batch-v1","toolset-rename-v1","toolset-delete-v1","toolset-create-v2","library-remove-v1","doctor-advisories-v1"]'
    else
      features='["apply-setup","restore-last","trust-consent","sessions-v1","profiles-v1","profiles-edit-v1","profiles-edit-batch-v1","toolset-rename-v1","toolset-delete-v1","toolset-create-v2","library-remove-v1","doctor-advisories-v1","doctor-probe-v1"]'
    fi
    printf '{"errors":0,"warnings":0,"state":"%s","sections":[],"advisories":0,"schema_version":1,"features":%s}\\n' "$state" "$features"
    ;;
  *" workflow list --json "*)
    if [ "$mode" = "no_contracts" ]; then
      features='["workflow-observe-v1"]'
    else
      features='["workflow-observe-v1","workflow-serial-roles-v1"]'
    fi
    printf '{"workflows":[{"name":"serial-build","declared":true,"trusted":true,"lock_status":"matches","roles":["builder"],"serial_roles":["builder"],"max_agents":4,"max_wall_seconds":600}],"schema_version":1,"features":%s}\\n' "$features"
    ;;
  *" workflow runs --json "*)
    printf '%s\\n' '{"runs":[]}'
    ;;
  *" report calls --json "*)
    printf '%s\\n' '{"events":[]}'
    ;;
  *" use --list --json "*)
    printf '%s\\n' '{"path":".","trust":"trusted","profiles":[{"name":"only","skills":[],"servers":[],"pinned":true,"active":true,"blockers":[]}],"schema_version":1,"features":["profiles-v1","sessions-v1","profiles-edit-v1","profiles-edit-batch-v1","toolset-rename-v1","toolset-delete-v1","toolset-create-v2"]}'
    ;;
  *" library-index "*)
    printf '%s\\n' '{"skills":[],"servers":[],"profiles":["only"],"schema_version":1,"features":["profiles-v1","profiles-edit-v1"]}'
    ;;
  *" restore --list --json "*)
    printf '%s\\n' '{"entries":[],"adapter_backups":[],"schema_version":1,"features":["restore-last"]}'
    ;;
  *)
    printf '%s\\n' '{}'
    ;;
esac
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
    await page.getByRole("dialog", { name: "Review this project" }).waitFor({ state: "visible" });

    console.log("t3-integration-smoke: F4 serial workflow projection");
    await setMode(modeFile, page, "ready");
    await openManage(page);
    await clickUnique(page, "Activity");
    assert.equal(
      await page.getByText(
        "builder runs one child at a time — that harness takes no per-child MCP config, so the ≤4 ceiling doesn't apply to it.",
        { exact: true },
      ).count(),
      1,
    );

    console.log("t3-integration-smoke: feature contracts fail closed");
    await setMode(modeFile, page, "no_contracts");
    await openManage(page);
    assert.equal(await page.getByRole("button", { name: "Test server startup" }).count(), 0);
    await clickUnique(page, "Activity");
    assert.equal(await page.getByText(/runs one child at a time/).count(), 0);

    assert.deepEqual(consoleErrors, [], `browser console errors:\n${consoleErrors.join("\n")}`);
    console.log("t3-integration-smoke: OK (real bridge + 4 browser regressions + fail-closed gates)");
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
