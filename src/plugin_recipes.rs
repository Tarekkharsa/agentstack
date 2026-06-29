//! AgentStack-managed plugin recipes.
//!
//! Recipes are authored once in `agentstack.toml` and rendered into repo-local
//! native plugin packages/marketplaces for Claude Code and Codex.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde_json::{json, Map, Value};

use crate::adapter::Registry;
use crate::manifest::{Hook, Manifest, PluginRecipe, Server};
use crate::secret::{refs_in, MapResolver};
use crate::store::{local_source_dir, Store};

const SUPPORTED_TARGETS: &[&str] = &["codex", "claude-code"];
const PACKAGE_ROOT: &str = "plugins/agentstack";
const MARKER: &str = ".agentstack-managed.json";

#[derive(Debug, Clone)]
pub struct SyncOptions {
    pub targets: Vec<String>,
    pub write: bool,
}

#[derive(Debug, Clone)]
pub struct SyncReport {
    pub recipes: Vec<RecipeStatus>,
    pub changed: Vec<PathBuf>,
    pub conflicts: Vec<String>,
    pub missing_skills: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct RecipeStatus {
    pub name: String,
    pub display: String,
    pub version: String,
    pub description: String,
    pub category: Option<String>,
    pub targets: Vec<String>,
    pub servers: Vec<String>,
    pub skills: Vec<String>,
    pub hooks: Vec<String>,
    pub package_path: PathBuf,
    pub generated: bool,
    pub stale: bool,
    pub conflict: Option<String>,
    pub missing_skills: Vec<String>,
    pub marketplaces: Vec<TargetMarketplaceStatus>,
    pub installs: Vec<TargetInstallStatus>,
    pub guidance: Vec<TargetGuidance>,
    pub required_secrets: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct TargetMarketplaceStatus {
    pub target: String,
    pub path: PathBuf,
    pub present: bool,
    pub stale: bool,
    pub native_visible: bool,
    pub native_source: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TargetInstallStatus {
    pub target: String,
    pub installed: bool,
    pub enabled: Option<bool>,
    pub status: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TargetGuidance {
    pub target: String,
    pub next_action: String,
}

impl SyncReport {
    pub fn has_errors(&self) -> bool {
        !self.conflicts.is_empty() || !self.missing_skills.is_empty()
    }
}

pub fn statuses(manifest: &Manifest, registry: &Registry, dir: &Path) -> Vec<RecipeStatus> {
    let targets = default_targets(registry);
    let (native_plugins, native_marketplaces) = crate::plugins::all_plugins();
    manifest
        .plugins
        .iter()
        // Pack ledgers are install records, not publishable plugins.
        .filter(|(_, recipe)| !is_pack_ledger(recipe))
        .map(|(name, recipe)| {
            let recipe_targets = effective_targets(recipe, &targets);
            recipe_status(
                name,
                recipe,
                manifest,
                dir,
                &recipe_targets,
                &native_plugins,
                &native_marketplaces,
            )
        })
        .collect()
}

pub fn sync(
    manifest: &Manifest,
    registry: &Registry,
    dir: &Path,
    opts: &SyncOptions,
) -> Result<SyncReport> {
    let selected_targets = if opts.targets.is_empty() {
        default_targets(registry)
    } else {
        opts.targets
            .iter()
            .filter(|t| SUPPORTED_TARGETS.contains(&t.as_str()))
            .cloned()
            .collect()
    };

    let mut report = SyncReport {
        recipes: Vec::new(),
        changed: Vec::new(),
        conflicts: Vec::new(),
        missing_skills: Vec::new(),
    };
    let mut generated = Vec::new();

    for (name, recipe) in &manifest.plugins {
        // Pack ledgers are install records, not publishable plugins — never
        // render them as native plugin packages/marketplaces.
        if is_pack_ledger(recipe) {
            continue;
        }
        let targets = effective_targets(recipe, &selected_targets);
        let native_plugins = Vec::new();
        let native_marketplaces = Vec::new();
        let mut status = recipe_status(
            name,
            recipe,
            manifest,
            dir,
            &targets,
            &native_plugins,
            &native_marketplaces,
        );
        if targets.is_empty() {
            report.recipes.push(status);
            continue;
        }

        let package = package_dir(dir, name);
        if package.exists() && !is_managed_dir(&package) {
            let msg = format!(
                "{} exists without {} — not overwriting",
                package.display(),
                MARKER
            );
            status.conflict = Some(msg.clone());
            report.conflicts.push(msg);
            report.recipes.push(status);
            continue;
        }

        let missing = missing_recipe_skills(recipe, manifest, dir);
        if !missing.is_empty() {
            for skill in &missing {
                report
                    .missing_skills
                    .push(format!("{name}: skill '{skill}' is not installed/resolved"));
            }
            status.conflict = Some("one or more skills are not installed".into());
            report.recipes.push(status);
            continue;
        }

        let rendered = render_package(name, recipe, manifest, dir, &targets)?;
        if opts.write {
            if package.exists() {
                fs::remove_dir_all(&package)
                    .with_context(|| format!("removing {}", package.display()))?;
            }
            write_rendered_dir(&package, &rendered.files)?;
        }
        report.changed.push(package.clone());
        status.generated = true;
        report.recipes.push(status);
        generated.push(GeneratedRecipe {
            name: name.clone(),
            recipe: recipe.clone(),
            targets,
        });
    }

    for removed in removed_managed_packages(manifest, dir)? {
        if opts.write {
            fs::remove_dir_all(&removed)
                .with_context(|| format!("removing {}", removed.display()))?;
        }
        report.changed.push(removed);
    }

    for target in &selected_targets {
        let relevant: Vec<&GeneratedRecipe> = generated
            .iter()
            .filter(|g| g.targets.iter().any(|t| t == target))
            .collect();
        let path = marketplace_path(dir, target);
        let proposed = match target.as_str() {
            "codex" => merge_codex_marketplace(&path, &relevant)?,
            "claude-code" => merge_claude_marketplace(&path, &relevant)?,
            _ => continue,
        };
        if marketplace_changed(&path, &proposed)? {
            if opts.write {
                write_json_file(&path, &proposed)?;
            }
            report.changed.push(path);
        }
    }

    Ok(report)
}

/// Whether a recipe is a pack install ledger (written by `agentstack add
/// <pack>`) rather than a publishable plugin. Pack ledgers are invisible to
/// `plugins sync` and `doctor`'s plugin reporting.
fn is_pack_ledger(recipe: &PluginRecipe) -> bool {
    recipe.kind.as_deref() == Some("pack")
}

fn default_targets(registry: &Registry) -> Vec<String> {
    SUPPORTED_TARGETS
        .iter()
        .filter(|id| registry.get(id).is_some())
        .map(|id| (*id).to_string())
        .collect()
}

fn effective_targets(recipe: &PluginRecipe, selected: &[String]) -> Vec<String> {
    selected
        .iter()
        .filter(|target| {
            recipe
                .targets
                .iter()
                .any(|t| t == "*" || t == target.as_str())
        })
        .cloned()
        .collect()
}

fn recipe_status(
    name: &str,
    recipe: &PluginRecipe,
    manifest: &Manifest,
    dir: &Path,
    targets: &[String],
    native_plugins: &[crate::plugins::Plugin],
    native_marketplaces: &[crate::plugins::Marketplace],
) -> RecipeStatus {
    let package_path = package_dir(dir, name);
    let conflict = package_path
        .exists()
        .then(|| (!is_managed_dir(&package_path)).then(|| "unmanaged package dir exists".into()))
        .flatten();
    let missing_skills = missing_recipe_skills(recipe, manifest, dir);
    let generated = is_managed_dir(&package_path);
    let stale = conflict.is_none()
        && generated
        && package_is_stale(name, recipe, manifest, dir, targets).unwrap_or(true);
    let marketplaces =
        marketplace_statuses(name, recipe, dir, targets, native_marketplaces).unwrap_or_default();
    let installs = install_statuses(name, targets, native_plugins);
    let guidance = target_guidance(RecipeGuidanceContext {
        name,
        dir,
        targets,
        generated,
        stale,
        conflict: conflict.as_deref(),
        missing_skills: &missing_skills,
        marketplaces: &marketplaces,
        installs: &installs,
    });
    RecipeStatus {
        name: name.to_string(),
        display: recipe.display.clone().unwrap_or_else(|| name.to_string()),
        version: recipe.version.clone(),
        description: recipe.description.clone(),
        category: recipe.category.clone(),
        targets: targets.to_vec(),
        servers: recipe.servers.clone(),
        skills: recipe.skills.clone(),
        hooks: recipe.hooks.clone(),
        generated,
        stale,
        conflict,
        missing_skills,
        marketplaces,
        installs,
        guidance,
        required_secrets: recipe_secrets(recipe, manifest),
        package_path,
    }
}

fn recipe_secrets(recipe: &PluginRecipe, manifest: &Manifest) -> Vec<String> {
    let mut refs = Vec::new();
    for name in &recipe.servers {
        if let Some(server) = manifest.servers.get(name) {
            collect_server_refs(server, &mut refs);
        }
    }
    for name in &recipe.hooks {
        if let Some(hook) = manifest.hooks.get(name) {
            collect_hook_refs(hook, &mut refs);
        }
    }
    refs.sort();
    refs.dedup();
    refs
}

fn collect_server_refs(server: &Server, refs: &mut Vec<String>) {
    let mut push = |s: &str| {
        for r in refs_in(s) {
            if !refs.contains(&r) {
                refs.push(r);
            }
        }
    };
    if let Some(url) = &server.url {
        push(url);
    }
    if let Some(cmd) = &server.command {
        push(cmd);
    }
    for arg in &server.args {
        push(arg);
    }
    for value in server.headers.values() {
        push(value);
    }
    for value in server.env.values() {
        push(value);
    }
}

fn collect_hook_refs(hook: &Hook, refs: &mut Vec<String>) {
    for r in refs_in(&hook.command) {
        if !refs.contains(&r) {
            refs.push(r);
        }
    }
    for arg in &hook.args {
        for r in refs_in(arg) {
            if !refs.contains(&r) {
                refs.push(r);
            }
        }
    }
}

fn missing_recipe_skills(recipe: &PluginRecipe, manifest: &Manifest, dir: &Path) -> Vec<String> {
    let store = Store::default_store();
    recipe
        .skills
        .iter()
        .filter_map(|name| {
            let skill = manifest.skills.get(name)?;
            local_source_dir(&store, skill, dir)
                .is_none()
                .then(|| name.clone())
        })
        .collect()
}

fn package_is_stale(
    name: &str,
    recipe: &PluginRecipe,
    manifest: &Manifest,
    dir: &Path,
    targets: &[String],
) -> Result<bool> {
    let package = package_dir(dir, name);
    let rendered = render_package(name, recipe, manifest, dir, targets)?;
    for (rel, expected) in rendered.files {
        let path = package.join(rel);
        match fs::read(&path) {
            Ok(actual) if actual == expected => {}
            _ => return Ok(true),
        }
    }
    Ok(false)
}

fn marketplace_statuses(
    name: &str,
    recipe: &PluginRecipe,
    dir: &Path,
    targets: &[String],
    native_marketplaces: &[crate::plugins::Marketplace],
) -> Result<Vec<TargetMarketplaceStatus>> {
    let generated = GeneratedRecipe {
        name: name.to_string(),
        recipe: recipe.clone(),
        targets: targets.to_vec(),
    };
    let mut out = Vec::new();
    for target in targets {
        let path = marketplace_path(dir, target);
        let expected_entry = match target.as_str() {
            "codex" => codex_marketplace_entry(&generated),
            "claude-code" => claude_marketplace_entry(&generated),
            _ => continue,
        };
        let actual_entry = marketplace_recipe_entry(&path, name);
        let present = actual_entry.is_some();
        let native = native_marketplaces
            .iter()
            .find(|m| m.harness == *target && m.name == "agentstack");
        out.push(TargetMarketplaceStatus {
            target: target.clone(),
            path: path.clone(),
            present,
            stale: actual_entry
                .as_ref()
                .map(|actual| actual != &expected_entry)
                .unwrap_or(false),
            native_visible: native.is_some(),
            native_source: native.map(|m| m.source.clone()),
        });
    }
    Ok(out)
}

fn marketplace_recipe_entry(path: &Path, name: &str) -> Option<Value> {
    let root = read_json_object(path)?;
    root.get("plugins")
        .and_then(Value::as_array)
        .and_then(|plugins| {
            plugins
                .iter()
                .find(|p| {
                    p.get("name").and_then(Value::as_str) == Some(name)
                        && is_agentstack_marketplace_entry(p)
                })
                .cloned()
        })
}

fn install_statuses(
    name: &str,
    targets: &[String],
    native_plugins: &[crate::plugins::Plugin],
) -> Vec<TargetInstallStatus> {
    targets
        .iter()
        .map(|target| {
            let plugin = native_plugins
                .iter()
                .find(|p| p.harness == *target && p.name == name && p.marketplace == "agentstack");
            TargetInstallStatus {
                target: target.clone(),
                installed: plugin.is_some(),
                enabled: plugin.and_then(|p| p.enabled),
                status: plugin.map(|p| p.status.clone()),
            }
        })
        .collect()
}

struct RecipeGuidanceContext<'a> {
    name: &'a str,
    dir: &'a Path,
    targets: &'a [String],
    generated: bool,
    stale: bool,
    conflict: Option<&'a str>,
    missing_skills: &'a [String],
    marketplaces: &'a [TargetMarketplaceStatus],
    installs: &'a [TargetInstallStatus],
}

fn target_guidance(ctx: RecipeGuidanceContext<'_>) -> Vec<TargetGuidance> {
    ctx.targets
        .iter()
        .map(|target| {
            let marketplace = ctx.marketplaces.iter().find(|m| m.target == *target);
            let install = ctx.installs.iter().find(|i| i.target == *target);
            TargetGuidance {
                target: target.clone(),
                next_action: next_action(NextActionContext {
                    name: ctx.name,
                    target,
                    repo_dir: ctx.dir,
                    conflict: ctx.conflict,
                    missing_skills: ctx.missing_skills,
                    generated: ctx.generated,
                    stale: ctx.stale,
                    marketplace_present: marketplace.map(|m| m.present).unwrap_or(false),
                    marketplace_stale: marketplace.map(|m| m.stale).unwrap_or(false),
                    native_marketplace_visible: marketplace
                        .map(|m| m.native_visible)
                        .unwrap_or(false),
                    installed: install.map(|i| i.installed).unwrap_or(false),
                    enabled: install.and_then(|i| i.enabled),
                }),
            }
        })
        .collect()
}

struct NextActionContext<'a> {
    name: &'a str,
    target: &'a str,
    repo_dir: &'a Path,
    conflict: Option<&'a str>,
    missing_skills: &'a [String],
    generated: bool,
    stale: bool,
    marketplace_present: bool,
    marketplace_stale: bool,
    native_marketplace_visible: bool,
    installed: bool,
    enabled: Option<bool>,
}

fn next_action(ctx: NextActionContext<'_>) -> String {
    if let Some(conflict) = ctx.conflict {
        return format!(
            "Resolve package conflict ({conflict}), then run agentstack plugins sync --write."
        );
    }
    if !ctx.missing_skills.is_empty() {
        return format!(
            "Install missing skill source(s) with agentstack install, then run agentstack plugins sync --write: {}.",
            ctx.missing_skills.join(", ")
        );
    }
    if !ctx.generated {
        return "Run agentstack plugins sync --write to generate the package and marketplaces."
            .to_string();
    }
    if ctx.stale || !ctx.marketplace_present || ctx.marketplace_stale {
        return "Run agentstack plugins sync --write to refresh the generated package and marketplace entry."
            .to_string();
    }
    if !ctx.native_marketplace_visible {
        return native_marketplace_action(ctx.target, ctx.name, ctx.repo_dir);
    }
    if !ctx.installed {
        return native_install_action(ctx.target, ctx.name);
    }
    match ctx.enabled {
        Some(true) => "Installed and enabled; no action needed.".to_string(),
        Some(false) => native_enable_action(ctx.target),
        None => native_check_action(ctx.target),
    }
}

fn native_marketplace_action(target: &str, name: &str, repo_dir: &Path) -> String {
    match target {
        "codex" => format!(
            "Make the repo marketplace visible to Codex: codex plugin marketplace add {} --json; then install with codex plugin add {name}@agentstack --json or browse /plugins.",
            repo_dir.display()
        ),
        "claude-code" => format!(
            "Make the repo marketplace visible to Claude Code: claude plugin marketplace add --scope local {}; then install with claude plugin install {name}@agentstack --scope local or browse /plugin.",
            repo_dir.display()
        ),
        _ => "Open the native plugin marketplace UI/CLI for this target and add this repository marketplace."
            .to_string(),
    }
}

fn native_install_action(target: &str, name: &str) -> String {
    match target {
        "codex" => format!(
            "Install from the native Codex marketplace: codex plugin add {name}@agentstack --json or browse /plugins."
        ),
        "claude-code" => format!(
            "Install from the native Claude Code marketplace: claude plugin install {name}@agentstack --scope local or browse /plugin."
        ),
        _ => "Install from this target's native plugin marketplace.".to_string(),
    }
}

fn native_enable_action(target: &str) -> String {
    match target {
        "codex" => "Plugin is installed but disabled; open Codex /plugins and enable or inspect it."
            .to_string(),
        "claude-code" => {
            "Plugin is installed but reported disabled; open Claude Code /plugin and enable or inspect it."
                .to_string()
        }
        _ => "Plugin is installed but disabled; open the native plugin UI/CLI and enable or inspect it."
            .to_string(),
    }
}

fn native_check_action(target: &str) -> String {
    match target {
        "codex" => "Plugin is installed; Codex discovery did not report enabled state, so check /plugins."
            .to_string(),
        "claude-code" => {
            "Plugin is installed; Claude Code discovery does not expose enabled state, so check /plugin."
                .to_string()
        }
        _ => "Plugin is installed; this target did not expose enabled state, so check the native UI/CLI."
            .to_string(),
    }
}

struct RenderedPackage {
    files: Vec<(PathBuf, Vec<u8>)>,
}

#[derive(Debug, Clone)]
struct GeneratedRecipe {
    name: String,
    recipe: PluginRecipe,
    targets: Vec<String>,
}

fn render_package(
    name: &str,
    recipe: &PluginRecipe,
    manifest: &Manifest,
    dir: &Path,
    targets: &[String],
) -> Result<RenderedPackage> {
    let mut files = Vec::new();
    files.push((
        PathBuf::from(MARKER),
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "generatedBy": "agentstack",
            "recipe": name
        }))?,
    ));
    files.push((
        PathBuf::from("README.md"),
        readme(name, recipe, manifest).into_bytes(),
    ));
    files.push((PathBuf::from(".mcp.json"), mcp_json(recipe, manifest)?));
    files.push((
        PathBuf::from("hooks/hooks.json"),
        hooks_json(recipe, manifest)?,
    ));

    if targets.iter().any(|t| t == "codex") {
        files.push((
            PathBuf::from(".codex-plugin/plugin.json"),
            serde_json::to_vec_pretty(&codex_manifest(name, recipe))?,
        ));
    }
    if targets.iter().any(|t| t == "claude-code") {
        files.push((
            PathBuf::from(".claude-plugin/plugin.json"),
            serde_json::to_vec_pretty(&claude_manifest(name, recipe))?,
        ));
    }

    for skill_name in &recipe.skills {
        let Some(skill) = manifest.skills.get(skill_name) else {
            continue;
        };
        let Some(source) = local_source_dir(&Store::default_store(), skill, dir) else {
            continue;
        };
        collect_copy_files(
            &source,
            &source,
            &PathBuf::from("skills").join(skill_name),
            &mut files,
        )?;
    }

    Ok(RenderedPackage { files })
}

fn codex_manifest(name: &str, recipe: &PluginRecipe) -> Value {
    let mut obj = Map::new();
    obj.insert("name".into(), json!(name));
    obj.insert("version".into(), json!(recipe.version));
    obj.insert("description".into(), json!(recipe.description));
    if let Some(default_enabled) = recipe.default_enabled {
        obj.insert("defaultEnabled".into(), json!(default_enabled));
    }
    if let Some(author) = &recipe.author {
        obj.insert("author".into(), json!({ "name": author }));
    }
    if let Some(homepage) = &recipe.homepage {
        obj.insert("homepage".into(), json!(homepage));
    }
    if let Some(repository) = &recipe.repository {
        obj.insert("repository".into(), json!(repository));
    }
    if let Some(license) = &recipe.license {
        obj.insert("license".into(), json!(license));
    }
    obj.insert("skills".into(), json!("./skills/"));
    obj.insert("mcpServers".into(), json!("./.mcp.json"));
    obj.insert("hooks".into(), json!("./hooks/hooks.json"));
    obj.insert(
        "interface".into(),
        json!({
            "displayName": recipe.display.clone().unwrap_or_else(|| name.to_string()),
            "shortDescription": recipe.description,
            "longDescription": recipe.description,
            "developerName": recipe.author.clone().unwrap_or_else(|| "AgentStack".to_string()),
            "category": recipe.category.clone().unwrap_or_else(|| "Developer Tools".to_string()),
            "capabilities": ["Interactive"]
        }),
    );
    Value::Object(obj)
}

fn claude_manifest(name: &str, recipe: &PluginRecipe) -> Value {
    let mut obj = Map::new();
    obj.insert("name".into(), json!(name));
    obj.insert("version".into(), json!(recipe.version));
    obj.insert("description".into(), json!(recipe.description));
    if let Some(default_enabled) = recipe.default_enabled {
        obj.insert("defaultEnabled".into(), json!(default_enabled));
    }
    if let Some(author) = &recipe.author {
        obj.insert("author".into(), json!({ "name": author }));
    }
    obj.insert("skills".into(), json!("./skills/"));
    obj.insert("mcpServers".into(), json!("./.mcp.json"));
    obj.insert("hooks".into(), json!("./hooks/hooks.json"));
    Value::Object(obj)
}

fn mcp_json(recipe: &PluginRecipe, manifest: &Manifest) -> Result<Vec<u8>> {
    let resolver = MapResolver::default();
    let reg = Registry::load()?;
    let desc = reg
        .get("claude-code")
        .or_else(|| reg.get("codex"))
        .context("no plugin-compatible MCP renderer adapter found")?;
    let mut servers = Map::new();
    for name in &recipe.servers {
        if let Some(server) = manifest.servers.get(name) {
            let rendered = crate::adapter::render_server(desc, server, &resolver).value;
            servers.insert(name.clone(), rendered);
        }
    }
    serde_json::to_vec_pretty(&json!({ "mcpServers": servers })).map_err(Into::into)
}

fn hooks_json(recipe: &PluginRecipe, manifest: &Manifest) -> Result<Vec<u8>> {
    let selected: Vec<(&String, &Hook)> = recipe
        .hooks
        .iter()
        .filter_map(|name| manifest.hooks.get_key_value(name))
        .collect();
    let mut unresolved = Vec::new();
    let resolver = MapResolver::default();
    let hooks = crate::render::hooks::build_claude_hooks(&selected, &resolver, &mut unresolved);
    serde_json::to_vec_pretty(&json!({ "hooks": hooks })).map_err(Into::into)
}

fn readme(name: &str, recipe: &PluginRecipe, manifest: &Manifest) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# {}\n\n",
        recipe.display.as_deref().unwrap_or(name)
    ));
    out.push_str(&recipe.description);
    out.push_str("\n\nGenerated by AgentStack from `agentstack.toml`.\n\n");
    out.push_str("## Capabilities\n\n");
    out.push_str(&format!("- Servers: {}\n", list_or_none(&recipe.servers)));
    out.push_str(&format!("- Skills: {}\n", list_or_none(&recipe.skills)));
    out.push_str(&format!("- Hooks: {}\n", list_or_none(&recipe.hooks)));
    let secrets = recipe_secrets(recipe, manifest);
    if !secrets.is_empty() {
        out.push_str("\n## Required Secret References\n\n");
        for s in secrets {
            out.push_str(&format!("- `${{{s}}}`\n"));
        }
    }
    out
}

fn list_or_none(items: &[String]) -> String {
    if items.is_empty() {
        "none".into()
    } else {
        items.join(", ")
    }
}

fn collect_copy_files(
    root: &Path,
    dir: &Path,
    dest_root: &Path,
    files: &mut Vec<(PathBuf, Vec<u8>)>,
) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        if entry.file_name() == ".git" {
            continue;
        }
        let path = entry.path();
        let rel = path.strip_prefix(root)?;
        let dest = dest_root.join(rel);
        if entry.file_type()?.is_dir() {
            collect_copy_files(root, &path, dest_root, files)?;
        } else {
            files.push((
                dest,
                fs::read(&path).with_context(|| format!("reading {}", path.display()))?,
            ));
        }
    }
    Ok(())
}

fn write_rendered_dir(package: &Path, files: &[(PathBuf, Vec<u8>)]) -> Result<()> {
    for (rel, bytes) in files {
        let path = package.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(())
}

fn removed_managed_packages(manifest: &Manifest, dir: &Path) -> Result<Vec<PathBuf>> {
    let root = dir.join(PACKAGE_ROOT);
    let mut removed = Vec::new();
    let Ok(entries) = fs::read_dir(&root) else {
        return Ok(removed);
    };
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let path = entry.path();
        if !manifest.plugins.contains_key(&name) && is_managed_dir(&path) {
            removed.push(path);
        }
    }
    Ok(removed)
}

fn merge_codex_marketplace(path: &Path, recipes: &[&GeneratedRecipe]) -> Result<Value> {
    let mut root = read_json_object(path).unwrap_or_else(|| {
        json!({
            "name": "agentstack",
            "interface": { "displayName": "AgentStack" },
            "plugins": []
        })
    });
    ensure_object_key(&mut root, "name", json!("agentstack"));
    ensure_object_key(
        &mut root,
        "interface",
        json!({ "displayName": "AgentStack" }),
    );
    replace_agentstack_plugins(&mut root, recipes, codex_marketplace_entry);
    Ok(root)
}

fn merge_claude_marketplace(path: &Path, recipes: &[&GeneratedRecipe]) -> Result<Value> {
    let mut root = read_json_object(path).unwrap_or_else(|| {
        json!({
            "$schema": "https://anthropic.com/claude-code/marketplace.schema.json",
            "name": "agentstack",
            "description": "AgentStack-managed plugins",
            "owner": { "name": "AgentStack" },
            "plugins": []
        })
    });
    ensure_object_key(
        &mut root,
        "$schema",
        json!("https://anthropic.com/claude-code/marketplace.schema.json"),
    );
    ensure_object_key(&mut root, "name", json!("agentstack"));
    ensure_object_key(
        &mut root,
        "description",
        json!("AgentStack-managed plugins"),
    );
    ensure_object_key(&mut root, "owner", json!({ "name": "AgentStack" }));
    replace_agentstack_plugins(&mut root, recipes, claude_marketplace_entry);
    Ok(root)
}

fn codex_marketplace_entry(g: &GeneratedRecipe) -> Value {
    json!({
        "name": g.name,
        "source": { "source": "local", "path": format!("./{PACKAGE_ROOT}/{}", g.name) },
        "policy": {
            "installation": "AVAILABLE",
            "authentication": "ON_INSTALL"
        },
        "category": g.recipe.category.clone().unwrap_or_else(|| "Developer Tools".to_string())
    })
}

fn claude_marketplace_entry(g: &GeneratedRecipe) -> Value {
    let mut obj = Map::new();
    obj.insert("name".into(), json!(g.name));
    obj.insert("description".into(), json!(g.recipe.description));
    obj.insert(
        "source".into(),
        json!(format!("./{PACKAGE_ROOT}/{}", g.name)),
    );
    if let Some(author) = &g.recipe.author {
        obj.insert("author".into(), json!({ "name": author }));
    }
    if let Some(category) = &g.recipe.category {
        obj.insert("category".into(), json!(category));
    }
    if let Some(homepage) = &g.recipe.homepage {
        obj.insert("homepage".into(), json!(homepage));
    }
    Value::Object(obj)
}

fn replace_agentstack_plugins<F>(root: &mut Value, recipes: &[&GeneratedRecipe], build: F)
where
    F: Fn(&GeneratedRecipe) -> Value,
{
    let Some(obj) = root.as_object_mut() else {
        return;
    };
    let mut plugins = obj
        .remove("plugins")
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    plugins.retain(|p| !is_agentstack_marketplace_entry(p));
    for recipe in recipes {
        plugins.push(build(recipe));
    }
    obj.insert("plugins".into(), Value::Array(plugins));
}

fn is_agentstack_marketplace_entry(value: &Value) -> bool {
    let Some(obj) = value.as_object() else {
        return false;
    };
    if let Some(source) = obj.get("source") {
        if let Some(s) = source.as_str() {
            return s.starts_with("./plugins/agentstack/");
        }
        if let Some(path) = source.get("path").and_then(Value::as_str) {
            return path.starts_with("./plugins/agentstack/");
        }
    }
    false
}

fn read_json_object(path: &Path) -> Option<Value> {
    let text = fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    value.as_object()?;
    Some(value)
}

fn ensure_object_key(root: &mut Value, key: &str, value: Value) {
    if let Some(obj) = root.as_object_mut() {
        obj.entry(key.to_string()).or_insert(value);
    }
}

fn marketplace_changed(path: &Path, proposed: &Value) -> Result<bool> {
    let existing = fs::read_to_string(path).unwrap_or_default();
    let mut proposed_text = serde_json::to_string_pretty(proposed)?;
    proposed_text.push('\n');
    Ok(existing != proposed_text)
}

fn write_json_file(path: &Path, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    fs::write(path, text).with_context(|| format!("writing {}", path.display()))
}

fn package_dir(dir: &Path, name: &str) -> PathBuf {
    dir.join(PACKAGE_ROOT).join(name)
}

fn marketplace_path(dir: &Path, target: &str) -> PathBuf {
    match target {
        "codex" => dir.join(".agents/plugins/marketplace.json"),
        "claude-code" => dir.join(".claude-plugin/marketplace.json"),
        _ => dir.join(format!(".agentstack/unsupported-{target}.json")),
    }
}

fn is_managed_dir(path: &Path) -> bool {
    path.join(MARKER).is_file()
}

pub fn supported_targets() -> BTreeSet<String> {
    SUPPORTED_TARGETS.iter().map(|s| (*s).to_string()).collect()
}

pub fn ensure_no_sync_errors(report: &SyncReport) -> Result<()> {
    if report.has_errors() {
        bail!("plugin recipe sync has conflicts or missing skills");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::prelude::*;

    fn manifest() -> Manifest {
        toml::from_str(
            r#"
            version = 1

            [servers.play]
            type = "stdio"
            command = "play-${PLAY_TOKEN}"
            args = ["serve"]

            [skills.play]
            path = "./skills/play"

            [hooks.done]
            event = "Stop"
            command = "echo ${PLAY_TOKEN}"

            [plugins.play]
            version = "1.0.0"
            description = "Play plugin"
            display = "Play"
            category = "Developer Tools"
            targets = ["codex", "claude-code"]
            servers = ["play"]
            skills = ["play"]
            hooks = ["done"]
            "#,
        )
        .unwrap()
    }

    #[test]
    fn sync_dry_run_writes_nothing_then_write_creates_package_and_marketplaces() {
        let tmp = assert_fs::TempDir::new().unwrap();
        tmp.child("skills/play/SKILL.md")
            .write_str("# Play\n")
            .unwrap();
        let m = manifest();
        let reg = Registry::load().unwrap();

        let dry = sync(
            &m,
            &reg,
            tmp.path(),
            &SyncOptions {
                targets: vec![],
                write: false,
            },
        )
        .unwrap();
        assert!(!dry.changed.is_empty());
        assert!(!tmp.path().join("plugins/agentstack/play").exists());

        let written = sync(
            &m,
            &reg,
            tmp.path(),
            &SyncOptions {
                targets: vec![],
                write: true,
            },
        )
        .unwrap();
        ensure_no_sync_errors(&written).unwrap();
        let package = tmp.path().join("plugins/agentstack/play");
        assert!(package.join(".agentstack-managed.json").exists());
        assert!(package.join(".codex-plugin/plugin.json").exists());
        assert!(package.join(".claude-plugin/plugin.json").exists());
        assert!(package.join("skills/play/SKILL.md").exists());
        let mcp = fs::read_to_string(package.join(".mcp.json")).unwrap();
        assert!(mcp.contains("${PLAY_TOKEN}"));
        assert!(tmp.path().join(".agents/plugins/marketplace.json").exists());
        assert!(tmp.path().join(".claude-plugin/marketplace.json").exists());
    }

    #[test]
    fn statuses_report_generated_and_stale_package() {
        let tmp = assert_fs::TempDir::new().unwrap();
        tmp.child("skills/play/SKILL.md")
            .write_str("# Play\n")
            .unwrap();
        let m = manifest();
        let reg = Registry::load().unwrap();
        let initial = statuses(&m, &reg, tmp.path());
        assert!(!initial[0].generated);

        sync(
            &m,
            &reg,
            tmp.path(),
            &SyncOptions {
                targets: vec![],
                write: true,
            },
        )
        .unwrap();
        let fresh = statuses(&m, &reg, tmp.path());
        assert!(fresh[0].generated);
        assert!(!fresh[0].stale);

        fs::write(
            tmp.path().join("plugins/agentstack/play/README.md"),
            "changed",
        )
        .unwrap();
        let stale = statuses(&m, &reg, tmp.path());
        assert!(stale[0].stale);
    }

    #[test]
    fn marketplace_merge_preserves_unrelated_entries_and_prunes_agentstack_entries() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let path = tmp.path().join(".agents/plugins/marketplace.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{
              "name": "custom",
              "plugins": [
                {"name":"manual","source":{"source":"local","path":"./plugins/manual"}},
                {"name":"old","source":{"source":"local","path":"./plugins/agentstack/old"}}
              ]
            }"#,
        )
        .unwrap();
        let m = manifest();
        let recipe = GeneratedRecipe {
            name: "play".into(),
            recipe: m.plugins["play"].clone(),
            targets: vec!["codex".into()],
        };
        let merged = merge_codex_marketplace(&path, &[&recipe]).unwrap();
        let names: Vec<_> = merged["plugins"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["manual", "play"]);
    }

    #[test]
    fn unmanaged_package_dir_is_a_conflict() {
        let tmp = assert_fs::TempDir::new().unwrap();
        tmp.child("skills/play/SKILL.md")
            .write_str("# Play\n")
            .unwrap();
        tmp.child("plugins/agentstack/play/README.md")
            .write_str("manual")
            .unwrap();
        let report = sync(
            &manifest(),
            &Registry::load().unwrap(),
            tmp.path(),
            &SyncOptions {
                targets: vec![],
                write: true,
            },
        )
        .unwrap();
        assert!(!report.conflicts.is_empty());
        assert_eq!(
            fs::read_to_string(tmp.path().join("plugins/agentstack/play/README.md")).unwrap(),
            "manual"
        );
    }

    #[test]
    fn pack_ledger_is_invisible_to_sync_and_statuses() {
        let m: Manifest = toml::from_str(
            r#"
            version = 1

            [servers.linear-pack]
            type = "http"
            url = "https://mcp.linear.app/mcp"

            [servers.linear-pack.headers]
            Authorization = "Bearer ${LINEAR_PACK_TOKEN}"

            [plugins.linear-pack]
            kind = "pack"
            version = "0.1.0"
            description = "Linear pack"
            source = "catalog:linear-pack"
            servers = ["linear-pack"]
            "#,
        )
        .unwrap();
        let tmp = assert_fs::TempDir::new().unwrap();
        let reg = Registry::load().unwrap();
        // No recipe statuses are reported for a pack ledger.
        assert!(statuses(&m, &reg, tmp.path()).is_empty());
        // Sync renders nothing and writes no package.
        let report = sync(
            &m,
            &reg,
            tmp.path(),
            &SyncOptions {
                targets: vec![],
                write: true,
            },
        )
        .unwrap();
        assert!(report.recipes.is_empty());
        assert!(!tmp.path().join("plugins/agentstack/linear-pack").exists());
    }

    #[test]
    fn next_action_prioritizes_generation_before_native_install() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let action = next_action(NextActionContext {
            name: "play",
            target: "codex",
            repo_dir: tmp.path(),
            conflict: None,
            missing_skills: &[],
            generated: false,
            stale: false,
            marketplace_present: false,
            marketplace_stale: false,
            native_marketplace_visible: false,
            installed: false,
            enabled: None,
        });
        assert_eq!(
            action,
            "Run agentstack plugins sync --write to generate the package and marketplaces."
        );
    }

    #[test]
    fn next_action_guides_native_marketplace_visibility() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let action = next_action(NextActionContext {
            name: "play",
            target: "claude-code",
            repo_dir: tmp.path(),
            conflict: None,
            missing_skills: &[],
            generated: true,
            stale: false,
            marketplace_present: true,
            marketplace_stale: false,
            native_marketplace_visible: false,
            installed: false,
            enabled: None,
        });
        assert!(action.contains("claude plugin marketplace add --scope local"));
        assert!(action.contains("claude plugin install play@agentstack --scope local"));
    }

    #[test]
    fn next_action_reports_uncertain_enabled_state() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let action = next_action(NextActionContext {
            name: "play",
            target: "claude-code",
            repo_dir: tmp.path(),
            conflict: None,
            missing_skills: &[],
            generated: true,
            stale: false,
            marketplace_present: true,
            marketplace_stale: false,
            native_marketplace_visible: true,
            installed: true,
            enabled: None,
        });
        assert_eq!(
            action,
            "Plugin is installed; Claude Code discovery does not expose enabled state, so check /plugin."
        );
    }
}
