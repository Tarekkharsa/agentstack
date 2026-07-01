//! `agentstack audit` — supply-chain content scan across every skill source
//! materialized locally plus the manifest's instruction files. High findings
//! (hidden Unicode) exit nonzero; Warn findings (injection heuristics) only
//! advise. `doctor --ci` runs the same scan as an extra check.

use std::path::{Path, PathBuf};

use anyhow::Result;
use owo_colors::OwoColorize;
use serde::Serialize;

use crate::cli::AuditArgs;
use crate::manifest::Manifest;
use crate::scan::{self, Finding, Severity};
use crate::store::Store;

/// One scanned capability (skill or instruction fragment) and its findings.
#[derive(Serialize)]
pub struct Unit {
    /// `skill` or `instruction`.
    pub kind: &'static str,
    pub name: String,
    /// Set when the source couldn't be scanned (not materialized, read error).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
    pub findings: Vec<Finding>,
}

/// Scan every manifest skill materialized locally (path sources and store
/// clones the lock references) plus every instruction file. Offline: a git
/// skill not yet in the store is reported as skipped, never fetched.
pub fn collect(manifest: &Manifest, dir: &Path, store: &Store) -> Vec<Unit> {
    let mut units = Vec::new();
    for (name, skill) in &manifest.skills {
        let unit = match crate::store::local_source_dir(store, skill, dir) {
            None => skipped_unit(
                "skill",
                name,
                "not materialized ↳ agentstack install".into(),
            ),
            Some(src) => match scan::scan_tree(&src) {
                Ok(findings) => Unit {
                    kind: "skill",
                    name: name.clone(),
                    skipped: None,
                    findings,
                },
                Err(e) => skipped_unit("skill", name, format!("scan failed: {e}")),
            },
        };
        units.push(unit);
    }
    for (name, instr) in &manifest.instructions {
        let path = resolve(dir, &instr.path);
        let unit = if !path.exists() {
            skipped_unit("instruction", name, format!("missing file {}", instr.path))
        } else {
            match scan::scan_file(&path, &instr.path) {
                Ok(findings) => Unit {
                    kind: "instruction",
                    name: name.clone(),
                    skipped: None,
                    findings,
                },
                Err(e) => skipped_unit("instruction", name, format!("scan failed: {e}")),
            }
        };
        units.push(unit);
    }
    units
}

fn skipped_unit(kind: &'static str, name: &str, reason: String) -> Unit {
    Unit {
        kind,
        name: name.to_string(),
        skipped: Some(reason),
        findings: Vec::new(),
    }
}

fn resolve(dir: &Path, p: &str) -> PathBuf {
    let pb = PathBuf::from(p);
    if pb.is_absolute() {
        pb
    } else {
        dir.join(pb)
    }
}

pub fn run(args: &AuditArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let ctx = super::load(manifest_dir)?;
    let store = Store::default_store();
    let units = collect(&ctx.loaded.manifest, &ctx.dir, &store);

    let count = |sev: Severity| {
        units
            .iter()
            .flat_map(|u| &u.findings)
            .filter(|f| f.severity == sev)
            .count()
    };
    let high = count(Severity::High);
    let warn = count(Severity::Warn);

    if args.json {
        let out = serde_json::json!({
            "high": high,
            "warn": warn,
            "capabilities": units,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        if units.is_empty() {
            println!("Nothing to audit — the manifest defines no skills or instructions.");
        }
        for unit in &units {
            println!("{} {}", unit.kind, unit.name.bold());
            if let Some(reason) = &unit.skipped {
                println!("  {} skipped — {reason}", "⚠".yellow());
                continue;
            }
            if unit.findings.is_empty() {
                println!("  {} clean", "✓".green());
            }
            for f in &unit.findings {
                let mark = match f.severity {
                    Severity::High => "✗".red().to_string(),
                    Severity::Warn => "⚠".yellow().to_string(),
                };
                println!("  {mark} {:<4} {}", f.severity.label(), f.describe());
            }
        }
        println!("\n{high} high, {warn} warn finding(s).");
    }

    // High findings fail the audit (warns never do). Return an error rather
    // than exiting inline so `main` owns the single exit point.
    if high > 0 {
        anyhow::bail!("audit found {high} high-severity finding(s) — see report above");
    }
    Ok(())
}
