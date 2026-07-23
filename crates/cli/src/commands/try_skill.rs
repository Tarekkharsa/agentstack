//! `agentstack try <source>` — ephemeral, manifest-free skill use:
//!
//! ```text
//! agentstack try anthropics/skills --skill pdf | claude
//! ```
//!
//! Stages the source (never the persistent store), discovers, scan-gates,
//! materializes ONE skill into `~/.agentstack/try/<id>/skill`, and emits a
//! wrapper prompt on **stdout** for piping into any agent CLI. Everything a
//! human reads goes to **stderr** — stdout is the pipe.
//!
//! Trust posture: nothing here touches the manifest, lock, library, or any
//! rendered target. The gates that DO apply: the scan (High findings block
//! unless `--allow-flagged`), clone containment, and a symlink refusal —
//! the ephemeral copy must not dereference a hostile link into the prompt's
//! support dir. The human explicitly piping content into their own agent is
//! the consent; the stderr provenance line makes what loaded visible.
//! This follows the familiar `skills use … | <agent>` interaction pattern.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use owo_colors::OwoColorize;

use crate::cli::TryArgs;

pub fn run(args: &TryArgs) -> Result<()> {
    let prepared = prepare(args)?;
    for w in &prepared.scan_warnings {
        eprintln!("{} {w}", "⚠".yellow());
    }
    eprintln!("{} {}", "✓".green(), prepared.provenance);
    eprintln!(
        "{} support files: {} — remove when done (doctor lists leftovers)",
        "·".dimmed(),
        prepared.support_dir.display()
    );
    print!("{}", prepared.prompt);
    Ok(())
}

#[derive(Debug)]
pub(crate) struct Prepared {
    pub prompt: String,
    pub support_dir: PathBuf,
    pub provenance: String,
    pub scan_warnings: Vec<String>,
}

pub(crate) fn prepare(args: &TryArgs) -> Result<Prepared> {
    use crate::provider::source::{parse_source, SkillSource};
    let parsed = parse_source(&args.source)?;
    let mut requested = args.skill.clone();
    if let Some(alias) = &parsed.skill_alias {
        if requested.is_empty() {
            requested.push(alias.clone());
        } else if !requested.contains(alias) {
            bail!(
                "skill given twice and they disagree: @{} vs --skill {}",
                crate::text::sanitize_line(alias),
                requested.join(", ")
            );
        }
    }

    // `_stage` must outlive the copy below (RAII removal on drop).
    let (_stage, src_dir, name, provenance) = match parsed.source {
        SkillSource::Local { path } => {
            if args.rev.is_some() || args.subpath.is_some() {
                bail!("--rev/--subpath apply to git sources");
            }
            let abs = if path.is_absolute() {
                path
            } else {
                std::env::current_dir()?.join(path)
            };
            anyhow::ensure!(abs.is_dir(), "no such directory: {}", abs.display());
            let root_name = abs.file_name().map(|n| n.to_string_lossy().into_owned());
            let (dir, name) = pick_one(&abs, root_name.as_deref(), &requested)?;
            let provenance = format!(
                "loaded '{}' from {} (scanned)",
                crate::text::sanitize_line(&name),
                abs.display()
            );
            (None, dir, name, provenance)
        }
        SkillSource::Git { url, ref_, subpath } => {
            let rev = super::add::merge_source_opt("rev", args.rev.as_ref(), ref_)?;
            let subpath = super::add::merge_source_opt("subpath", args.subpath.as_ref(), subpath)?;
            if let Some(s) = &subpath {
                crate::provider::source::validate_subpath(s)?;
            }
            let stage = crate::store::Stage::create()?;
            let staging = stage.store();
            let (clone_root, head) = crate::store::checkout(&staging, &url, rev.as_deref())?;
            let disc_root = crate::store::contained_content_dir(&clone_root, subpath.as_deref())?;
            anyhow::ensure!(
                disc_root.is_dir(),
                "subpath does not exist in {}",
                crate::text::sanitize_line(&url)
            );
            let repo_name = crate::provider::source::repo_name(&url);
            let (dir, name) = pick_one(&disc_root, repo_name.as_deref(), &requested)?;
            let provenance = format!(
                "loaded '{}' from {} @ {} (scanned)",
                crate::text::sanitize_line(&name),
                crate::text::sanitize_line(&url),
                &head[..head.len().min(12)]
            );
            (Some(stage), dir, name, provenance)
        }
    };

    let mut scan_warnings = Vec::new();
    crate::scan::gate(&name, &src_dir, args.allow_flagged, &mut scan_warnings)?;
    refuse_symlinks(&src_dir)?;

    // Persistent (outlives this process — the consuming agent reads it), on
    // the agentstack home like staging; doctor's stale sweep names leftovers.
    let dest_root = crate::util::paths::agentstack_home()
        .join("try")
        .join(crate::runs::gen_id());
    let dest = dest_root.join("skill");
    std::fs::create_dir_all(&dest_root)
        .with_context(|| format!("creating {}", dest_root.display()))?;
    crate::util::restrict(&dest_root, true);
    crate::util::fsx::copy_dir_all(&src_dir, &dest)?;

    let body = std::fs::read_to_string(src_dir.join("SKILL.md"))
        .with_context(|| format!("reading {}", src_dir.join("SKILL.md").display()))?;
    // The body is delivered verbatim — same exemption as `agentstack_load`:
    // the skill's content IS the product, and the scan gate above is its
    // hostile-bytes jurisdiction.
    let prompt = format!(
        "You are being given a skill to apply to the user's next request.\n\n\
         <SKILL.md>\n{body}\n</SKILL.md>\n\n\
         Supporting files for this skill are at:\n{}\n\n\
         When SKILL.md references relative paths, read them from that directory.\n",
        dest.display()
    );
    Ok(Prepared {
        prompt,
        support_dir: dest,
        provenance,
        scan_warnings,
    })
}

/// `try` runs exactly one skill: an explicit `--skill`, or the source's
/// single conventional hit.
fn pick_one(
    root: &Path,
    root_name: Option<&str>,
    requested: &[String],
) -> Result<(PathBuf, String)> {
    let discovered = crate::provider::discover::discover_skills(root, root_name)?;
    anyhow::ensure!(!discovered.is_empty(), "no SKILL.md found in this source");
    let names = || {
        discovered
            .iter()
            .map(|s| crate::text::sanitize_line(&s.name))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let picked = match requested {
        [one] => discovered
            .iter()
            .find(|s| s.name == *one)
            .with_context(|| format!("no skill named '{}' — found: {}", one, names()))?,
        [] if discovered.len() == 1 && !discovered[0].via_fallback => &discovered[0],
        [] => bail!(
            "`try` runs exactly one skill — pass --skill <name>: {}",
            names()
        ),
        _ => bail!("`try` runs exactly one skill — pass a single --skill"),
    };
    let dir = if picked.rel_path.is_empty() {
        root.to_path_buf()
    } else {
        root.join(&picked.rel_path)
    };
    Ok((dir, picked.name.clone()))
}

/// The ephemeral copy is read by an agent outside every later gate — a
/// symlink in the skill body could route that read anywhere, so refuse the
/// whole skill rather than silently dereferencing or dropping links.
fn refuse_symlinks(dir: &Path) -> Result<()> {
    for entry in std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .flatten()
    {
        let ft = entry.file_type()?;
        if ft.is_symlink() {
            bail!(
                "skill contains a symlink ({}) — not supported for ephemeral use",
                crate::text::sanitize_line(&entry.file_name().to_string_lossy())
            );
        }
        if ft.is_dir() && entry.file_name() != ".git" {
            refuse_symlinks(&entry.path())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepares_prompt_from_local_dir_and_refuses_symlinks() {
        use assert_fs::prelude::*;
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", home.path());

        let work = assert_fs::TempDir::new().unwrap();
        let skill = work.child("greeter");
        skill.create_dir_all().unwrap();
        skill
            .child("SKILL.md")
            .write_str("---\ndescription: greet\n---\nSay hello warmly.\n")
            .unwrap();

        let args = crate::cli::TryArgs {
            source: skill.path().display().to_string(),
            skill: vec![],
            rev: None,
            subpath: None,
            allow_flagged: false,
        };
        let p = prepare(&args).unwrap();
        assert!(p.prompt.contains("Say hello warmly."));
        assert!(p.prompt.contains("<SKILL.md>"));
        assert!(p.support_dir.join("SKILL.md").exists());
        assert!(p.provenance.contains("greeter"));

        // A symlink inside the body refuses the whole skill.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc/hosts", skill.path().join("leak")).unwrap();
            let err = prepare(&args).unwrap_err();
            assert!(err.to_string().contains("symlink"), "{err:#}");
        }
        std::env::remove_var("AGENTSTACK_HOME");
    }
}
