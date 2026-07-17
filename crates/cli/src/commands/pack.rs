//! `agentstack lib pack-init` — scaffold a publishable pack: a git repo with a
//! `pack.toml` describing its members (server + skills + instructions).
//! Publishing is just `git push` + a version tag; installing is
//! `agentstack add from git:<host>/<repo>@<tag>`.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use owo_colors::OwoColorize;

pub fn init(name: Option<&str>) -> Result<()> {
    let cwd = std::env::current_dir().context("reading current dir")?;
    let name = match name {
        Some(n) => n.to_string(),
        None => cwd
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "my-pack".into()),
    };
    let pack_toml = cwd.join("pack.toml");
    if pack_toml.exists() {
        anyhow::bail!("pack.toml already exists here");
    }

    let toml_body = format!(
        r#"# agentstack pack manifest — install with:
#   agentstack add from git:<host>/<repo>@<tag>
name = "{name}"
description = "What this pack sets an agent up to do."

# Optional MCP server. Secrets are declared by NAME only — agentstack lifts
# them to ${{REF}}s in the installer's manifest; never put values here.
# [server]
# type = "http"
# url = "https://mcp.example.com/mcp"
# secret_headers = ["Authorization"]

[[skill]]
name = "{name}-starter"
path = "skills/starter"

# Optional house rules (installed only with --with-instructions):
# [[instruction]]
# name = "{name}-rules"
# path = "rules.md"
"#
    );
    let skill_dir = cwd.join("skills/starter");
    fs::create_dir_all(&skill_dir).with_context(|| format!("creating {}", skill_dir.display()))?;
    fs::write(&pack_toml, toml_body).context("writing pack.toml")?;
    write_if_absent(
        &skill_dir.join("SKILL.md"),
        &format!(
            "---\nname: {name}-starter\ndescription: Replace with what this skill teaches the agent.\n---\n\n# {name} starter skill\n\nWrite the skill body here.\n"
        ),
    )?;
    write_if_absent(
        &cwd.join("README.md"),
        &format!(
            "# {name}\n\nAn [agentstack](https://github.com/Tarekkharsa/agentstack) pack.\n\n\
             ## Install\n\n```sh\nagentstack add from git:<host>/<repo>@v0.1.0 --write\n```\n\n\
             ## Publish\n\n```sh\ngit init && git add . && git commit -m \"pack: {name}\"\n\
             git tag v0.1.0\ngit push origin main --tags\n```\n"
        ),
    )?;

    println!("{} scaffolded pack '{}':", "✓".green(), name.bold());
    println!("  pack.toml");
    println!("  skills/starter/SKILL.md");
    println!("  README.md");
    println!(
        "\n{} edit pack.toml, then publish: git tag v0.1.0 && git push --tags",
        "↳".cyan()
    );
    Ok(())
}

fn write_if_absent(path: &Path, body: &str) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    fs::write(path, body).with_context(|| format!("writing {}", path.display()))
}
