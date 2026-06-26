# Dashboard Control Center Plan

Date: 2026-06-26

## Goal

Turn the AgentStack dashboard from a broad inspection UI into a safe control center where a developer can understand, fix, sync, install, and apply the whole stack without dropping to the terminal for normal workflows.

The dashboard should answer three questions immediately:

1. What is wrong or unfinished?
2. What will AgentStack change if I click the button?
3. What is the next safe action?

## Current State

The dashboard already has useful sections:

- Overview
- Discover
- Servers
- Skills
- Settings
- Hooks
- Plugins
- Instructions
- Secrets
- Health

It can already mutate important state:

- initialize a manifest
- add servers, skills, hooks, plugin recipes
- toggle servers/skills globally per target
- save/import settings
- set secrets
- install skills
- sync plugin recipes
- install/remove native plugins
- apply rendered configs

This is a strong foundation, but the experience still feels like an admin panel built around internal primitives. It should become an operator console built around guided fixes.

## Key Findings

### 1. Health And Preview Contradict Each Other

Observed live:

- Health reported: `4 target(s) drifted (global) - Apply to reconcile`
- Preview global reported: `No changes - everything is already in sync.`

Likely cause:

- Health drift checks all known adapters from the registry.
- Preview only checks resolved manifest targets.

Impact:

- This damages trust. Users cannot tell whether applying is needed.

Fix:

- Use one shared drift calculation for Health and Preview.
- Health should say whether drift exists for:
  - selected/default manifest targets
  - all known installed targets, if different
- The visible action should open the matching preview scope and target set.

### 2. The Overview Is Not Yet A Command Center

The overview shows useful counts and generic actions, but it does not prioritize what to do next.

Missing:

- unresolved secrets action
- drift fix action
- plugin sync/install action
- missing skill install action
- stale generated artifacts action
- target detection/config issue action

Fix:

- Add a "Next actions" command center on Overview.
- Each action card should include:
  - severity
  - concise issue
  - exact impacted object count
  - primary button
  - secondary link to the relevant section

### 3. Plugins Page Is Powerful But Too Dense

Managed recipes currently show too many concepts in one row:

- recipe metadata
- generated package state
- marketplace entry state
- native marketplace visibility
- native install state
- capability counts
- raw command guidance

Fix:

- Turn each managed recipe into a stepper:
  1. Recipe valid
  2. Package generated
  3. Marketplace written
  4. Native marketplace visible
  5. Native plugin installed
  6. Enabled/ready
- Each step should show:
  - done / pending / blocked / warning
  - one next action if needed
  - target-specific state for Codex and Claude Code

### 4. Mutation Safety Is Inconsistent

`Apply` has a diff preview. Other write actions often mutate directly:

- toggles
- profile activation
- settings save
- plugin sync
- native plugin install/remove
- skill consolidation/adoption

Fix:

- Introduce a generic dashboard "operation preview" model.
- Before any risky mutation, show:
  - files that may change
  - native commands that may run
  - whether secrets are written
  - whether backups are created
  - whether the action is reversible
- V1 can use a simpler modal confirmation for non-file mutations, but it should be structured, not `window.confirm`.

### 5. Settings Page Is Too Dense

The settings page is powerful, but it is long and hard to scan.

Fix:

- Add search/filter across setting label, key, and help text.
- Collapse groups by default after the first group.
- Show managed count per adapter.
- Show dirty state before Save.
- Add preset buttons:
  - conservative
  - strict permissions
  - fast coding
  - privacy-focused
- Keep unmanaged keys visible and explicitly preserved.

### 6. Server And Skill Matrices Need Clearer Scope Control

Cells currently show `global`, `project`, or `both`, but clicking toggles only global scope.

Fix:

- Add a scope selector above the matrix: Global / Project.
- Matrix clicks affect selected scope.
- Cell display should separate:
  - active in global
  - active in project
  - inherited/effective state
- Add hover/tooltips or compact legends.

## Product Principles

1. **Actionable over informational**
   Every warning should have a direct next action.

2. **Preview before trust-sensitive writes**
   Native commands and config writes should be visible before execution.

3. **Same logic everywhere**
   CLI, Health, Preview, and Dashboard action cards must share status calculations.

4. **Native systems remain native**
   AgentStack can prepare and hand off, but UI must clearly distinguish:
   - AgentStack manifest state
   - generated repo artifacts
   - native harness state

5. **No secret leakage**
   Dashboard may show secret names and resolved/missing status, never values except during explicit set input.

## Implementation Plan

### Phase 1: Fix Health And Preview Consistency

Create a shared dashboard drift/status helper used by both:

- `snapshot::health_checks`
- `snapshot::diffs`

Implementation details:

- Add a function such as:
  - `dashboard::snapshot::render_drift_targets(ctx, manifest, scope, target_filter)`
- It should return structured drift objects:
  - target id
  - display
  - config path
  - selected by manifest
  - installed/config present
  - changed
  - diff
  - reason skipped, if no plan
- Health should count drift for the same selected targets shown in Preview.
- If non-selected installed adapters are drifted, show a separate warning:
  - "N installed non-default target(s) have renderable drift; add them to [targets].default or preview all."

Acceptance:

- Health and Preview never contradict for selected/default targets.
- The Health drift row can open a matching preview.
- Tests cover default-target drift count.

### Phase 2: Add Dashboard Next Actions Model

Add structured `nextActions` to `/api/state`.

Suggested shape:

```json
{
  "id": "missing-secret:KIBANA_TOKEN",
  "level": "error",
  "title": "KIBANA_TOKEN is missing",
  "detail": "1 server references this secret.",
  "section": "secrets",
  "primary": {
    "label": "Set secret",
    "action": "section",
    "section": "secrets"
  }
}
```

Actions to generate:

- missing secrets
- drifted selected targets
- missing skill sources
- stale/missing plugin packages
- plugin marketplace not visible
- plugin not installed
- installed adapters with config parse errors

Acceptance:

- Overview renders a "Next actions" card above generic actions.
- Empty state says the stack is ready.
- Each action links to a section or runs a safe preview.

### Phase 3: Build Command Center Overview

Replace the generic Overview "Actions" block with:

- Next actions
- Stack summary
- Recent/available operations

Suggested layout:

- Top row: health summary cards
- Main left: prioritized next actions
- Main right: active targets and generated state
- Lower: profiles and usage

Actions should be ordered:

1. errors
2. warnings
3. setup/incomplete items
4. optional improvements

Acceptance:

- A new user can see the first thing to fix without opening Health.
- Buttons route to Secrets, Preview, Plugins, Skills, or Settings.

### Phase 4: Plugin Recipe Stepper

Replace the managed recipe row with a richer but still compact card.

Per recipe card:

- title, id, version
- target chips
- capability summary
- package path collapsed or copyable
- per-target stepper

Target stepper states:

- `generated`
- `marketplace entry`
- `native marketplace`
- `native install`
- `ready`

Primary action rules:

- conflict -> show conflict, no install action
- missing skills -> install skills
- not generated/stale -> sync recipes
- marketplace hidden -> add/install native plugin
- not installed -> install native plugin
- installed disabled/unknown -> open native UI guidance

Acceptance:

- The user can tell exactly why a recipe is not ready.
- Install/Remove buttons remain available but are contextual.
- Raw command guidance moves into a details disclosure or preview modal.

### Phase 5: Structured Operation Preview Modal

Create a reusable modal helper for actions.

V1 operations:

- Apply config
- Plugin sync
- Native plugin install/remove
- Skill install
- Profile activate

Modal content:

- operation title
- what will happen
- file changes or native commands
- affected targets
- risk level
- confirmation button

For file changes:

- reuse `/api/diff`

For native commands:

- expose a dry-run plan endpoint or reuse command planner logic through dashboard server.

Acceptance:

- No native plugin install/remove uses browser `confirm`.
- The user sees native commands before execution.
- Apply still shows real diffs.

### Phase 6: Settings UX Pass

Add:

- search input
- group collapse/expand
- managed count per adapter
- dirty indicator
- save disabled until dirty
- preset buttons

Acceptance:

- User can find "sandbox", "model", "permission", etc.
- It is clear which settings AgentStack will manage.
- Save has a preview of manifest changes or at least a structured summary.

### Phase 7: Scope Control For Servers And Skills

Add global/project segmented control.

Implementation:

- Add local UI state `MATRIX_SCOPE = "global" | "project"`.
- Use it in `/api/toggle` and `/api/toggle_skill`.
- Update table text/tooltips.

Acceptance:

- Users can manage project-scoped and global-scoped capabilities from UI.
- Cell state clearly shows global/project/both.

## Suggested First Implementation Slice

Implement these first:

1. Phase 1: Health/Preview consistency.
2. Phase 2: `nextActions` model.
3. Phase 3: Command Center Overview.
4. Phase 4, small version: plugin cards with target stepper.

Defer:

- Settings presets
- full operation preview backend
- matrix scope selector

This slice directly fixes trust and makes the dashboard feel like a control center.

## Test Plan

Unit tests:

- Health drift uses same target set as Preview.
- `nextActions` includes missing secret.
- `nextActions` includes selected-target drift.
- `nextActions` includes stale/missing plugin recipe status.
- Plugin step status maps generated/marketplace/install state correctly.

Integration/manual checks:

- `cargo fmt`
- `cargo test`
- `cargo clippy --all-targets -- -D warnings`
- Launch dashboard:
  - Overview shows Next actions.
  - Health drift and Preview agree.
  - Plugins page shows stepper cards.
  - Existing actions still work.

Browser checks:

- Desktop viewport: no overlapping rows/chips/buttons.
- Mobile/narrow viewport: sidebar wraps; cards remain readable.
- Plugins page with many installed plugins remains scannable.

## Non-Goals For This Slice

- Full redesign with a frontend framework.
- Cloud sync/accounts.
- Remote team collaboration.
- Editing every possible manifest field.
- Replacing native harness marketplace UIs.

## Success Criteria

The dashboard should feel like this:

1. Open dashboard.
2. See the top three problems.
3. Click one action.
4. Preview exactly what changes.
5. Apply safely.
6. Status updates immediately.

When this is true, AgentStack becomes substantially more useful for real developers because the UI no longer only visualizes complexity; it actively reduces it.
