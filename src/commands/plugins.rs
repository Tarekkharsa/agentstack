//! `agentstack plugins` — manage AgentStack plugin recipes and generated
//! repo-local native plugin marketplaces.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

use anyhow::{Context, Result};
use indexmap::IndexMap;
use owo_colors::OwoColorize;
use serde_json::{json, Value};

use crate::cli::{
    PluginsAdoptArgs, PluginsArgs, PluginsCommand, PluginsCreateArgs, PluginsNativeArgs,
    PluginsStatusArgs, PluginsSyncArgs,
};
use crate::manifest::{validate_with_targets, Hook, PluginRecipe, Server, ServerType, Skill};
use crate::plugin_recipes::{self, SyncOptions};
use crate::util::diff;

pub fn run(args: &PluginsArgs, manifest_dir: Option<&Path>) -> Result<()> {
    match &args.command {
        PluginsCommand::List => list(manifest_dir),
        PluginsCommand::Status(a) => status(a, manifest_dir),
        PluginsCommand::Create(a) => create(a, manifest_dir),
        PluginsCommand::Adopt(a) => adopt(a, manifest_dir),
        PluginsCommand::Sync(a) => sync(a, manifest_dir),
        PluginsCommand::Install(a) => native_install(a, manifest_dir),
        PluginsCommand::Remove(a) => native_remove(a, manifest_dir),
    }
}

fn list(manifest_dir: Option<&Path>) -> Result<()> {
    let ctx = super::load(manifest_dir)?;
    let statuses = plugin_recipes::statuses(&ctx.loaded.manifest, &ctx.registry, &ctx.dir);
    if statuses.is_empty() {
        println!("Manifest defines no plugin recipes.");
        return Ok(());
    }
    println!("AgentStack plugin recipes:\n");
    for recipe in statuses {
        let targets = if recipe.targets.is_empty() {
            "no supported targets".to_string()
        } else {
            recipe.targets.join(", ")
        };
        let state = if recipe.conflict.is_some() {
            "conflict".red().to_string()
        } else if recipe.generated {
            "generated".green().to_string()
        } else {
            "pending".yellow().to_string()
        };
        println!(
            "{} {} {} ({targets})",
            recipe.name.bold(),
            recipe.version.dimmed(),
            state
        );
        println!("  {}", recipe.description);
        println!(
            "  servers: {} · skills: {} · hooks: {}",
            count(recipe.servers.len()),
            count(recipe.skills.len()),
            count(recipe.hooks.len())
        );
        if !recipe.required_secrets.is_empty() {
            println!("  secrets: {}", recipe.required_secrets.join(", "));
        }
        if let Some(conflict) = recipe.conflict {
            println!("  {} {conflict}", "✗".red());
        }
    }
    Ok(())
}

fn status(args: &PluginsStatusArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let ctx = super::load(manifest_dir)?;
    let mut statuses = plugin_recipes::statuses(&ctx.loaded.manifest, &ctx.registry, &ctx.dir);
    if let Some(name) = &args.name {
        statuses.retain(|r| &r.name == name);
        if statuses.is_empty() {
            anyhow::bail!("no plugin recipe named '{name}'");
        }
    }
    if statuses.is_empty() {
        println!("Manifest defines no plugin recipes.");
        return Ok(());
    }

    println!("AgentStack plugin recipe status:\n");
    for recipe in statuses {
        println!(
            "{} {} {}",
            recipe.name.bold(),
            format!("({})", recipe.display).dimmed(),
            recipe.version.dimmed()
        );
        println!("  package: {}", recipe.package_path.display());
        println!("  state: {}", recipe_state_label(&recipe));
        if let Some(conflict) = &recipe.conflict {
            println!("  conflict: {conflict}");
        }
        if !recipe.missing_skills.is_empty() {
            println!("  missing skills: {}", recipe.missing_skills.join(", "));
        }
        if !recipe.required_secrets.is_empty() {
            println!("  secrets: {}", recipe.required_secrets.join(", "));
        }
        for target in &recipe.targets {
            let marketplace = recipe.marketplaces.iter().find(|m| &m.target == target);
            let install = recipe.installs.iter().find(|i| &i.target == target);
            let guidance = recipe.guidance.iter().find(|g| &g.target == target);
            println!("  {target}:");
            if let Some(m) = marketplace {
                println!(
                    "    marketplace: {} ({})",
                    marketplace_label(m.present, m.stale),
                    m.path.display()
                );
                println!(
                    "    native marketplace: {}",
                    if m.native_visible {
                        match &m.native_source {
                            Some(source) if !source.is_empty() => {
                                format!("visible as agentstack ({source})")
                            }
                            _ => "visible as agentstack".to_string(),
                        }
                    } else {
                        "not visible in native discovery".to_string()
                    }
                );
            }
            if let Some(i) = install {
                println!("    native install: {}", install_label(i));
            }
            if let Some(g) = guidance {
                println!("    next: {}", g.next_action);
            }
        }
        println!();
    }
    Ok(())
}

fn create(args: &PluginsCreateArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let ctx = super::load(manifest_dir)?;
    if ctx.loaded.manifest.plugins.contains_key(&args.name) {
        anyhow::bail!("plugin recipe '{}' already exists", args.name);
    }
    let recipe = PluginRecipe {
        version: args.version.clone(),
        description: args.description.clone(),
        display: args.display.clone(),
        category: args.category.clone(),
        targets: if args.targets.is_empty() {
            vec!["*".into()]
        } else {
            args.targets.clone()
        },
        default_enabled: args.default_enabled.then_some(true),
        servers: args.servers.clone(),
        skills: args.skills.clone(),
        hooks: args.hooks.clone(),
        homepage: args.homepage.clone(),
        repository: args.repository.clone(),
        license: args.license.clone(),
        author: args.author.clone(),
    };
    let original = fs::read_to_string(&ctx.loaded.manifest_path)
        .with_context(|| format!("reading {}", ctx.loaded.manifest_path.display()))?;
    let new_text = insert_plugin_recipe(&original, &args.name, &recipe)?;
    validate_manifest_text(&new_text, &ctx)?;
    print_manifest_change(
        "create plugin recipe",
        &args.name,
        &ctx.loaded.manifest_path,
        &original,
        &new_text,
    );
    if args.write {
        crate::util::atomic::write(&ctx.loaded.manifest_path, &new_text)
            .with_context(|| format!("writing {}", ctx.loaded.manifest_path.display()))?;
        println!("{} created plugin recipe '{}'.", "✓".green(), args.name);
    } else {
        println!(
            "\nDry run. Re-run with {} to update the manifest.",
            "--write".bold()
        );
    }
    Ok(())
}

fn adopt(args: &PluginsAdoptArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let ctx = super::load(manifest_dir)?;
    let (native_plugins, _) = crate::plugins::all_plugins();
    let matches: Vec<_> = native_plugins
        .into_iter()
        .filter(|p| p.name == args.name)
        .filter(|p| args.harness.as_ref().map_or(true, |h| &p.harness == h))
        .filter(|p| {
            args.marketplace
                .as_ref()
                .map_or(true, |m| &p.marketplace == m)
        })
        .collect();
    if matches.is_empty() {
        anyhow::bail!("no installed native plugin named '{}'", args.name);
    }
    if matches.len() > 1 {
        let choices = matches
            .iter()
            .map(|p| format!("{}@{} ({})", p.name, p.marketplace, p.harness))
            .collect::<Vec<_>>()
            .join(", ");
        anyhow::bail!(
            "multiple installed plugins match; pass --harness or --marketplace: {choices}"
        );
    }
    let native = &matches[0];
    let Some(source) = &native.source else {
        anyhow::bail!(
            "{}@{} ({}) has no local package path to inspect",
            native.name,
            native.marketplace,
            native.harness
        );
    };
    let source = PathBuf::from(source);
    if !source.is_dir() {
        anyhow::bail!("native plugin path does not exist: {}", source.display());
    }
    let recipe_name = args.as_name.clone().unwrap_or_else(|| native.name.clone());
    if ctx.loaded.manifest.plugins.contains_key(&recipe_name) {
        anyhow::bail!("plugin recipe '{}' already exists", recipe_name);
    }

    let adopted = inspect_native_plugin(&recipe_name, native, &source, &ctx.loaded.manifest)?;
    let original = fs::read_to_string(&ctx.loaded.manifest_path)
        .with_context(|| format!("reading {}", ctx.loaded.manifest_path.display()))?;
    let mut new_text = original.clone();
    for (name, server) in &adopted.servers {
        new_text = crate::commands::add::build_manifest_with(
            &new_text,
            "servers",
            name,
            &serde_json::to_value(server)?,
            None,
        )?;
    }
    for (name, skill) in &adopted.skills {
        new_text = crate::commands::add::build_manifest_with(
            &new_text,
            "skills",
            name,
            &serde_json::to_value(skill)?,
            None,
        )?;
    }
    for (name, hook) in &adopted.hooks {
        new_text = crate::commands::add::build_manifest_with(
            &new_text,
            "hooks",
            name,
            &serde_json::to_value(hook)?,
            None,
        )?;
    }
    new_text = insert_plugin_recipe(&new_text, &recipe_name, &adopted.recipe)?;
    validate_manifest_text(&new_text, &ctx)?;

    print_manifest_change(
        "adopt native plugin",
        &format!(
            "{}@{} ({})",
            native.name, native.marketplace, native.harness
        ),
        &ctx.loaded.manifest_path,
        &original,
        &new_text,
    );
    println!(
        "  lifted {} server(s), {} skill(s), {} hook(s) from {}",
        adopted.servers.len(),
        adopted.skills.len(),
        adopted.hooks.len(),
        source.display()
    );
    if args.write {
        crate::util::atomic::write(&ctx.loaded.manifest_path, &new_text)
            .with_context(|| format!("writing {}", ctx.loaded.manifest_path.display()))?;
        println!("{} adopted plugin recipe '{}'.", "✓".green(), recipe_name);
    } else {
        println!(
            "\nDry run. Re-run with {} to update the manifest.",
            "--write".bold()
        );
    }
    Ok(())
}

fn sync(args: &PluginsSyncArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let ctx = super::load(manifest_dir)?;
    let supported = plugin_recipes::supported_targets();
    for target in &args.targets {
        if !supported.contains(target) {
            anyhow::bail!("target '{target}' does not support managed plugin recipes in v1");
        }
    }
    let valid_targets: Vec<&str> = ctx.registry.ids().collect();
    let issues = validate_with_targets(&ctx.loaded.manifest, valid_targets);
    let mut has_errors = false;
    for issue in issues {
        if issue.kind.is_error() {
            has_errors = true;
            println!("{} {}", "✗".red(), issue.message);
        } else {
            println!("{} {}", "⚠".yellow(), issue.message);
        }
    }
    if has_errors {
        anyhow::bail!("manifest has validation errors — not syncing plugin recipes");
    }

    let report = plugin_recipes::sync(
        &ctx.loaded.manifest,
        &ctx.registry,
        &ctx.dir,
        &SyncOptions {
            targets: args.targets.clone(),
            write: args.write,
        },
    )?;

    if report.recipes.is_empty() {
        println!("Manifest defines no plugin recipes.");
        return Ok(());
    }

    for recipe in &report.recipes {
        let targets = if recipe.targets.is_empty() {
            "no supported targets".to_string()
        } else {
            recipe.targets.join(", ")
        };
        println!(
            "{} {} → {}",
            if recipe.conflict.is_some() {
                "✗".red().to_string()
            } else if recipe.generated {
                "✓".green().to_string()
            } else {
                "·".dimmed().to_string()
            },
            recipe.name.bold(),
            targets
        );
        if let Some(conflict) = &recipe.conflict {
            println!("  {conflict}");
        }
    }

    for missing in &report.missing_skills {
        println!("{} {missing}", "✗".red());
    }
    for conflict in &report.conflicts {
        println!("{} {conflict}", "✗".red());
    }

    println!();
    if args.write {
        println!(
            "{} wrote {} generated path(s).",
            "✓".green(),
            report.changed.len()
        );
        print_sync_guidance();
    } else {
        println!(
            "{} path(s) would change. Re-run with {} to write.",
            report.changed.len(),
            "--write".bold()
        );
    }
    plugin_recipes::ensure_no_sync_errors(&report)?;
    Ok(())
}

fn native_install(args: &PluginsNativeArgs, manifest_dir: Option<&Path>) -> Result<()> {
    native_plugin_action(
        &args.name,
        &args.targets,
        NativeAction::Install,
        args.write,
        manifest_dir,
    )
}

fn native_remove(args: &PluginsNativeArgs, manifest_dir: Option<&Path>) -> Result<()> {
    native_plugin_action(
        &args.name,
        &args.targets,
        NativeAction::Remove,
        args.write,
        manifest_dir,
    )
}

pub fn install_recipe_native(
    manifest_dir: Option<&Path>,
    name: &str,
    targets: &[String],
    write: bool,
) -> Result<()> {
    native_plugin_action(name, targets, NativeAction::Install, write, manifest_dir)
}

pub fn remove_recipe_native(
    manifest_dir: Option<&Path>,
    name: &str,
    targets: &[String],
    write: bool,
) -> Result<()> {
    native_plugin_action(name, targets, NativeAction::Remove, write, manifest_dir)
}

fn native_plugin_action(
    name: &str,
    requested_targets: &[String],
    action: NativeAction,
    write: bool,
    manifest_dir: Option<&Path>,
) -> Result<()> {
    let ctx = super::load(manifest_dir)?;
    let mut statuses = plugin_recipes::statuses(&ctx.loaded.manifest, &ctx.registry, &ctx.dir);
    let Some(recipe) = statuses.iter_mut().find(|r| r.name == name) else {
        anyhow::bail!("no plugin recipe named '{name}'");
    };
    let targets = selected_native_targets(recipe, requested_targets)?;
    let plans = native_action_plans(recipe, &targets, action);
    println!(
        "AgentStack native plugin {} plan for {}:\n",
        action.verb(),
        name.bold()
    );
    let mut executable = 0usize;
    for plan in &plans {
        println!("{}:", plan.target);
        for note in &plan.notes {
            println!("  {note}");
        }
        for command in &plan.commands {
            executable += 1;
            println!("  $ {}", shell_display(command));
        }
        if plan.notes.is_empty() && plan.commands.is_empty() {
            println!("  no action needed");
        }
    }
    if !write {
        if executable == 0 {
            println!("\nNo native commands would run.");
        } else {
            println!(
                "\nDry run. Re-run with {} to execute these native commands.",
                "--write".bold()
            );
        }
        return Ok(());
    }
    if executable == 0 {
        println!("\nNo native commands to run.");
        return Ok(());
    }
    println!();
    for plan in plans {
        for command in plan.commands {
            run_native_command(&command)?;
        }
    }
    Ok(())
}

fn selected_native_targets(
    recipe: &plugin_recipes::RecipeStatus,
    requested: &[String],
) -> Result<Vec<String>> {
    let supported = plugin_recipes::supported_targets();
    let selected = if requested.is_empty() {
        recipe.targets.clone()
    } else {
        requested.to_vec()
    };
    for target in &selected {
        if !supported.contains(target) {
            anyhow::bail!("target '{target}' does not support managed plugin recipes in v1");
        }
        if !recipe.targets.contains(target) {
            anyhow::bail!("recipe '{}' does not target '{target}'", recipe.name);
        }
    }
    Ok(selected)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeAction {
    Install,
    Remove,
}

impl NativeAction {
    fn verb(self) -> &'static str {
        match self {
            NativeAction::Install => "install",
            NativeAction::Remove => "remove",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NativeCommandPlan {
    target: String,
    notes: Vec<String>,
    commands: Vec<Vec<String>>,
}

fn native_action_plans(
    recipe: &plugin_recipes::RecipeStatus,
    targets: &[String],
    action: NativeAction,
) -> Vec<NativeCommandPlan> {
    targets
        .iter()
        .map(|target| native_action_plan(recipe, target, action))
        .collect()
}

fn native_action_plan(
    recipe: &plugin_recipes::RecipeStatus,
    target: &str,
    action: NativeAction,
) -> NativeCommandPlan {
    match action {
        NativeAction::Install => native_install_plan(recipe, target),
        NativeAction::Remove => native_remove_plan(recipe, target),
    }
}

fn native_install_plan(recipe: &plugin_recipes::RecipeStatus, target: &str) -> NativeCommandPlan {
    let marketplace = recipe.marketplaces.iter().find(|m| m.target == target);
    let install = recipe.installs.iter().find(|i| i.target == target);
    let mut notes = Vec::new();
    let mut commands = Vec::new();
    if let Some(conflict) = &recipe.conflict {
        notes.push(format!(
            "blocked: resolve package conflict ({conflict}), then run agentstack plugins sync --write"
        ));
        return NativeCommandPlan {
            target: target.into(),
            notes,
            commands,
        };
    }
    if !recipe.missing_skills.is_empty() {
        notes.push(format!(
            "blocked: install missing skill source(s) with agentstack install: {}",
            recipe.missing_skills.join(", ")
        ));
        return NativeCommandPlan {
            target: target.into(),
            notes,
            commands,
        };
    }
    if !recipe.generated || recipe.stale {
        notes.push("blocked: run agentstack plugins sync --write before native install".into());
        return NativeCommandPlan {
            target: target.into(),
            notes,
            commands,
        };
    }
    if marketplace.map(|m| !m.present || m.stale).unwrap_or(true) {
        notes
            .push("blocked: run agentstack plugins sync --write to refresh the marketplace".into());
        return NativeCommandPlan {
            target: target.into(),
            notes,
            commands,
        };
    }
    if install.map(|i| i.installed).unwrap_or(false) {
        notes.push("already installed from the AgentStack marketplace".into());
        return NativeCommandPlan {
            target: target.into(),
            notes,
            commands,
        };
    }
    if !marketplace.map(|m| m.native_visible).unwrap_or(false) {
        if let Some(cmd) = native_marketplace_command(target, &recipe.package_path) {
            commands.push(cmd);
        } else {
            notes.push("blocked: this target has no automated marketplace add command".into());
            return NativeCommandPlan {
                target: target.into(),
                notes,
                commands,
            };
        }
    }
    if let Some(cmd) = native_install_command(target, &recipe.name) {
        commands.push(cmd);
    } else {
        notes.push("blocked: this target has no automated install command".into());
    }
    NativeCommandPlan {
        target: target.into(),
        notes,
        commands,
    }
}

fn native_remove_plan(recipe: &plugin_recipes::RecipeStatus, target: &str) -> NativeCommandPlan {
    let install = recipe.installs.iter().find(|i| i.target == target);
    let mut notes = Vec::new();
    let mut commands = Vec::new();
    if !install.map(|i| i.installed).unwrap_or(false) {
        notes.push("not installed from the AgentStack marketplace".into());
        return NativeCommandPlan {
            target: target.into(),
            notes,
            commands,
        };
    }
    if let Some(cmd) = native_remove_command(target, &recipe.name) {
        commands.push(cmd);
        notes.push(
            "marketplace source is left configured; remove it in the native CLI if desired".into(),
        );
    } else {
        notes.push("blocked: this target has no automated remove command".into());
    }
    NativeCommandPlan {
        target: target.into(),
        notes,
        commands,
    }
}

fn native_marketplace_command(target: &str, package_path: &Path) -> Option<Vec<String>> {
    let repo_dir = package_path.parent()?.parent()?.parent()?;
    match target {
        "codex" => Some(vec![
            "codex".into(),
            "plugin".into(),
            "marketplace".into(),
            "add".into(),
            repo_dir.display().to_string(),
            "--json".into(),
        ]),
        "claude-code" => Some(vec![
            "claude".into(),
            "plugin".into(),
            "marketplace".into(),
            "add".into(),
            "--scope".into(),
            "local".into(),
            repo_dir.display().to_string(),
        ]),
        _ => None,
    }
}

fn native_install_command(target: &str, name: &str) -> Option<Vec<String>> {
    match target {
        "codex" => Some(vec![
            "codex".into(),
            "plugin".into(),
            "add".into(),
            format!("{name}@agentstack"),
            "--json".into(),
        ]),
        "claude-code" => Some(vec![
            "claude".into(),
            "plugin".into(),
            "install".into(),
            format!("{name}@agentstack"),
            "--scope".into(),
            "local".into(),
        ]),
        _ => None,
    }
}

fn native_remove_command(target: &str, name: &str) -> Option<Vec<String>> {
    match target {
        "codex" => Some(vec![
            "codex".into(),
            "plugin".into(),
            "remove".into(),
            format!("{name}@agentstack"),
            "--json".into(),
        ]),
        "claude-code" => Some(vec![
            "claude".into(),
            "plugin".into(),
            "uninstall".into(),
            format!("{name}@agentstack"),
            "--scope".into(),
            "local".into(),
        ]),
        _ => None,
    }
}

fn run_native_command(command: &[String]) -> Result<()> {
    if command.is_empty() {
        return Ok(());
    }
    println!("$ {}", shell_display(command));
    let status = ProcessCommand::new(&command[0])
        .args(&command[1..])
        .status()
        .with_context(|| format!("running {}", shell_display(command)))?;
    if !status.success() {
        anyhow::bail!(
            "native command failed with status {status}: {}",
            shell_display(command)
        );
    }
    Ok(())
}

fn shell_display(command: &[String]) -> String {
    command
        .iter()
        .map(|part| {
            if part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_./:@=".contains(c))
            {
                part.clone()
            } else {
                format!("'{}'", part.replace('\'', "'\\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn recipe_state_label(recipe: &plugin_recipes::RecipeStatus) -> String {
    if recipe.conflict.is_some() {
        "conflict".red().to_string()
    } else if !recipe.missing_skills.is_empty() {
        "missing skill".yellow().to_string()
    } else if !recipe.generated {
        "not generated".yellow().to_string()
    } else if recipe.stale {
        "stale".yellow().to_string()
    } else {
        "generated".green().to_string()
    }
}

fn marketplace_label(present: bool, stale: bool) -> String {
    match (present, stale) {
        (true, false) => "present".green().to_string(),
        (true, true) => "stale".yellow().to_string(),
        (false, _) => "missing".yellow().to_string(),
    }
}

fn install_label(status: &plugin_recipes::TargetInstallStatus) -> String {
    if !status.installed {
        return "not installed".yellow().to_string();
    }
    match status.enabled {
        Some(true) => status
            .status
            .clone()
            .unwrap_or_else(|| "installed, enabled".to_string())
            .green()
            .to_string(),
        Some(false) => status
            .status
            .clone()
            .unwrap_or_else(|| "installed, disabled".to_string())
            .yellow()
            .to_string(),
        None => status
            .status
            .clone()
            .unwrap_or_else(|| "installed, enabled unknown".to_string())
            .yellow()
            .to_string(),
    }
}

fn count(n: usize) -> String {
    match n {
        0 => "none".into(),
        1 => "1".into(),
        _ => n.to_string(),
    }
}

fn insert_plugin_recipe(original: &str, name: &str, recipe: &PluginRecipe) -> Result<String> {
    crate::commands::add::build_manifest_with(
        original,
        "plugins",
        name,
        &serde_json::to_value(recipe)?,
        None,
    )
}

fn validate_manifest_text(text: &str, ctx: &crate::commands::Context) -> Result<()> {
    let manifest: crate::manifest::Manifest =
        toml::from_str(text).context("parsing updated manifest")?;
    let valid_targets: Vec<&str> = ctx.registry.ids().collect();
    let issues = validate_with_targets(&manifest, valid_targets);
    if let Some(issue) = issues.into_iter().find(|i| i.kind.is_error()) {
        anyhow::bail!(issue.message);
    }
    Ok(())
}

fn print_manifest_change(action: &str, name: &str, path: &Path, before: &str, after: &str) {
    println!("{} {action} '{}' in {}", "+".green(), name, path.display());
    print!(
        "{}",
        diff::render(before, after)
            .lines()
            .map(|l| format!("  {l}\n"))
            .collect::<String>()
    );
}

fn print_sync_guidance() {
    println!();
    println!("Next:");
    println!("  Codex: restart/open Codex, run /plugins, choose the repo marketplace, then install the recipe.");
    println!("  Claude Code: open /plugin and add/install from this repository marketplace if it is not prompted automatically.");
    println!("  AgentStack generated packages only; native harnesses still ask for install/trust consent.");
}

struct AdoptedPlugin {
    recipe: PluginRecipe,
    servers: IndexMap<String, Server>,
    skills: IndexMap<String, Skill>,
    hooks: IndexMap<String, Hook>,
}

fn inspect_native_plugin(
    recipe_name: &str,
    native: &crate::plugins::Plugin,
    source: &Path,
    manifest: &crate::manifest::Manifest,
) -> Result<AdoptedPlugin> {
    let meta = read_native_plugin_meta(source)?;
    let mut servers = IndexMap::new();
    for (name, server) in read_native_mcp(source)? {
        let key = unique_name(
            manifest.servers.keys(),
            servers.keys(),
            &format!("{recipe_name}-{name}"),
        );
        servers.insert(key, server);
    }

    let mut skills = IndexMap::new();
    let skills_dir = source.join("skills");
    if let Ok(entries) = fs::read_dir(&skills_dir) {
        for entry in entries.flatten() {
            if entry.path().join("SKILL.md").is_file() {
                let native_name = entry.file_name().to_string_lossy().to_string();
                let key = unique_name(
                    manifest.skills.keys(),
                    skills.keys(),
                    &format!("{recipe_name}-{native_name}"),
                );
                skills.insert(
                    key,
                    Skill {
                        path: Some(entry.path().display().to_string()),
                        git: None,
                        rev: None,
                    },
                );
            }
        }
    }

    let mut hooks = IndexMap::new();
    for (name, hook) in read_native_hooks(source, recipe_name)? {
        let key = unique_name(manifest.hooks.keys(), hooks.keys(), &name);
        hooks.insert(key, hook);
    }

    let recipe = PluginRecipe {
        version: meta
            .get("version")
            .and_then(Value::as_str)
            .or(native.version.as_deref())
            .unwrap_or("0.1.0")
            .to_string(),
        description: meta
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("Adopted native plugin")
            .to_string(),
        display: meta
            .get("interface")
            .and_then(|i| i.get("displayName"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| meta.get("name").and_then(Value::as_str).map(str::to_string)),
        category: meta
            .get("interface")
            .and_then(|i| i.get("category"))
            .and_then(Value::as_str)
            .map(str::to_string),
        targets: vec![native.harness.clone()],
        default_enabled: None,
        servers: servers.keys().cloned().collect(),
        skills: skills.keys().cloned().collect(),
        hooks: hooks.keys().cloned().collect(),
        homepage: meta
            .get("homepage")
            .and_then(Value::as_str)
            .map(str::to_string),
        repository: meta
            .get("repository")
            .and_then(Value::as_str)
            .map(str::to_string),
        license: meta
            .get("license")
            .and_then(Value::as_str)
            .map(str::to_string),
        author: meta
            .get("author")
            .and_then(|a| a.get("name").or(Some(a)))
            .and_then(Value::as_str)
            .map(str::to_string),
    };

    Ok(AdoptedPlugin {
        recipe,
        servers,
        skills,
        hooks,
    })
}

fn read_native_plugin_meta(source: &Path) -> Result<Value> {
    for rel in [".codex-plugin/plugin.json", ".claude-plugin/plugin.json"] {
        let path = source.join(rel);
        if path.is_file() {
            let text =
                fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
            return serde_json::from_str(&text)
                .with_context(|| format!("parsing {}", path.display()));
        }
    }
    Ok(json!({}))
}

fn read_native_mcp(source: &Path) -> Result<IndexMap<String, Server>> {
    let path = source.join(".mcp.json");
    if !path.is_file() {
        return Ok(IndexMap::new());
    }
    let text = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let root: Value =
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    let entries = root
        .get("mcpServers")
        .and_then(Value::as_object)
        .or_else(|| root.as_object())
        .cloned()
        .unwrap_or_default();
    let mut out = IndexMap::new();
    for (name, value) in entries {
        if let Some(server) = server_from_native_value(&value) {
            out.insert(name, server);
        }
    }
    Ok(out)
}

fn server_from_native_value(value: &Value) -> Option<Server> {
    let obj = value.as_object()?;
    let url = obj.get("url").and_then(Value::as_str).map(str::to_string);
    let command = obj
        .get("command")
        .and_then(Value::as_str)
        .map(str::to_string);
    let server_type = if url.is_some() {
        ServerType::Http
    } else if command.is_some() {
        ServerType::Stdio
    } else {
        return None;
    };
    let args = obj
        .get("args")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let headers = obj
        .get("headers")
        .or_else(|| obj.get("http_headers"))
        .and_then(Value::as_object)
        .map(string_map)
        .unwrap_or_default();
    let env = obj
        .get("env")
        .and_then(Value::as_object)
        .map(string_map)
        .unwrap_or_default();
    Some(Server {
        server_type,
        url,
        command,
        args,
        headers,
        env,
    })
}

fn string_map(map: &serde_json::Map<String, Value>) -> IndexMap<String, String> {
    map.iter()
        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
        .collect()
}

fn read_native_hooks(source: &Path, recipe_name: &str) -> Result<IndexMap<String, Hook>> {
    let mut path = source.join("hooks/hooks.json");
    if !path.is_file() {
        path = source.join("hooks.json");
    }
    if !path.is_file() {
        return Ok(IndexMap::new());
    }
    let text = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let root: Value =
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    let events = root
        .get("hooks")
        .and_then(Value::as_object)
        .or_else(|| root.as_object())
        .cloned()
        .unwrap_or_default();
    let mut out = IndexMap::new();
    for (event, handlers) in events {
        let Some(handlers) = handlers.as_array() else {
            continue;
        };
        for (idx, entry) in handlers.iter().enumerate() {
            let matcher = entry
                .get("matcher")
                .and_then(Value::as_str)
                .map(str::to_string);
            let Some(hook_arr) = entry.get("hooks").and_then(Value::as_array) else {
                continue;
            };
            for (handler_idx, handler) in hook_arr.iter().enumerate() {
                let Some(command) = handler.get("command").and_then(Value::as_str) else {
                    continue;
                };
                let args = handler
                    .get("args")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                let timeout = handler.get("timeout").and_then(Value::as_u64);
                out.insert(
                    format!(
                        "{}-{}-{}",
                        recipe_name,
                        sanitize_id(&event),
                        idx + handler_idx + 1
                    ),
                    Hook {
                        event: event.clone(),
                        matcher: matcher.clone(),
                        command: command.to_string(),
                        args,
                        timeout,
                        targets: vec!["*".into()],
                    },
                );
            }
        }
    }
    Ok(out)
}

fn unique_name<'a>(
    existing: impl Iterator<Item = &'a String>,
    pending: impl Iterator<Item = &'a String>,
    desired: &str,
) -> String {
    let taken: std::collections::BTreeSet<String> =
        existing.chain(pending).map(|s| s.to_string()).collect();
    let base = sanitize_id(desired);
    if !taken.contains(&base) {
        return base;
    }
    for i in 2.. {
        let candidate = format!("{base}-{i}");
        if !taken.contains(&candidate) {
            return candidate;
        }
    }
    unreachable!()
}

fn sanitize_id(input: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for c in input.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "plugin".into()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::prelude::*;

    fn recipe_status_for_plan() -> plugin_recipes::RecipeStatus {
        plugin_recipes::RecipeStatus {
            name: "play".into(),
            display: "Play".into(),
            version: "0.1.0".into(),
            description: "Play plugin".into(),
            category: None,
            targets: vec!["codex".into(), "claude-code".into()],
            servers: vec![],
            skills: vec![],
            hooks: vec![],
            package_path: PathBuf::from("/repo/plugins/agentstack/play"),
            generated: true,
            stale: false,
            conflict: None,
            missing_skills: vec![],
            marketplaces: vec![
                plugin_recipes::TargetMarketplaceStatus {
                    target: "codex".into(),
                    path: PathBuf::from("/repo/.agents/plugins/marketplace.json"),
                    present: true,
                    stale: false,
                    native_visible: false,
                    native_source: None,
                },
                plugin_recipes::TargetMarketplaceStatus {
                    target: "claude-code".into(),
                    path: PathBuf::from("/repo/.claude-plugin/marketplace.json"),
                    present: true,
                    stale: false,
                    native_visible: true,
                    native_source: Some("/repo".into()),
                },
            ],
            installs: vec![
                plugin_recipes::TargetInstallStatus {
                    target: "codex".into(),
                    installed: false,
                    enabled: None,
                    status: None,
                },
                plugin_recipes::TargetInstallStatus {
                    target: "claude-code".into(),
                    installed: true,
                    enabled: None,
                    status: Some("installed".into()),
                },
            ],
            guidance: vec![],
            required_secrets: vec![],
        }
    }

    #[test]
    fn native_install_plan_adds_marketplace_before_plugin() {
        let recipe = recipe_status_for_plan();
        let plan = native_action_plan(&recipe, "codex", NativeAction::Install);
        assert_eq!(plan.notes, Vec::<String>::new());
        assert_eq!(
            plan.commands,
            vec![
                vec![
                    "codex".to_string(),
                    "plugin".into(),
                    "marketplace".into(),
                    "add".into(),
                    "/repo".into(),
                    "--json".into(),
                ],
                vec![
                    "codex".to_string(),
                    "plugin".into(),
                    "add".into(),
                    "play@agentstack".into(),
                    "--json".into(),
                ],
            ]
        );
    }

    #[test]
    fn native_install_plan_noops_when_already_installed() {
        let recipe = recipe_status_for_plan();
        let plan = native_action_plan(&recipe, "claude-code", NativeAction::Install);
        assert!(plan.commands.is_empty());
        assert_eq!(
            plan.notes,
            vec!["already installed from the AgentStack marketplace"]
        );
    }

    #[test]
    fn native_remove_plan_uninstalls_installed_plugin() {
        let recipe = recipe_status_for_plan();
        let plan = native_action_plan(&recipe, "claude-code", NativeAction::Remove);
        assert_eq!(
            plan.commands,
            vec![vec![
                "claude".to_string(),
                "plugin".into(),
                "uninstall".into(),
                "play@agentstack".into(),
                "--scope".into(),
                "local".into(),
            ]]
        );
    }

    #[test]
    fn inspects_native_plugin_package_into_recipe_parts() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let plugin = tmp.child("native");
        plugin
            .child(".codex-plugin/plugin.json")
            .write_str(
                r#"{
          "name":"play",
          "version":"1.2.3",
          "description":"Play plugin",
          "skills":"./skills/",
          "mcpServers":"./.mcp.json",
          "interface":{"displayName":"Play","category":"Developer Tools"}
        }"#,
            )
            .unwrap();
        plugin
            .child(".mcp.json")
            .write_str(
                r#"{
          "mcpServers": {
            "play": {"command":"npx","args":["play"],"env":{"PLAY_TOKEN":"${PLAY_TOKEN}"}}
          }
        }"#,
            )
            .unwrap();
        plugin
            .child("skills/run/SKILL.md")
            .write_str("# Run\n")
            .unwrap();
        plugin
            .child("hooks/hooks.json")
            .write_str(
                r#"{
          "hooks": {"Stop": [{"hooks": [{"type":"command","command":"echo done"}]}]}
        }"#,
            )
            .unwrap();
        let native = crate::plugins::Plugin {
            harness: "codex".into(),
            name: "play".into(),
            marketplace: "local".into(),
            scope: "available".into(),
            projects: vec![],
            version: Some("1.2.3".into()),
            enabled: Some(true),
            status: "installed".into(),
            source: Some(plugin.path().display().to_string()),
        };
        let manifest: crate::manifest::Manifest = toml::from_str("version = 1\n").unwrap();
        let adopted = inspect_native_plugin("play", &native, plugin.path(), &manifest).unwrap();
        assert_eq!(adopted.recipe.version, "1.2.3");
        assert_eq!(adopted.recipe.targets, vec!["codex"]);
        assert_eq!(adopted.servers.len(), 1);
        assert_eq!(adopted.skills.len(), 1);
        assert_eq!(adopted.hooks.len(), 1);
        assert!(adopted.recipe.servers.contains(&"play-play".to_string()));
    }
}
