//! Adapter registry: the descriptors embedded in the binary, plus any
//! user-supplied overrides/additions dropped in `~/.agentstack/adapters/`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use anyhow::{Context, Result};
use include_dir::{include_dir, Dir};

use super::descriptor::{AdapterDescriptor, AdapterSource};
use crate::util::paths;

/// Descriptors shipped inside the binary.
static EMBEDDED: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/adapters");

/// All known adapters, keyed by id. User descriptors override embedded ones
/// with the same id.
pub struct Registry {
    adapters: BTreeMap<String, AdapterDescriptor>,
    /// Ids shipped in the binary — so a user descriptor sharing one can be
    /// flagged as an override.
    builtin_ids: BTreeSet<String>,
}

impl Registry {
    /// Load embedded descriptors then layer user descriptors on top.
    pub fn load() -> Result<Self> {
        let mut adapters = BTreeMap::new();
        let mut builtin_ids = BTreeSet::new();

        for file in EMBEDDED.files() {
            if !is_yaml(file.path().to_string_lossy().as_ref()) {
                continue;
            }
            let text = file
                .contents_utf8()
                .context("embedded adapter is not valid UTF-8")?;
            let desc: AdapterDescriptor = serde_yaml::from_str(text)
                .with_context(|| format!("parsing embedded adapter {}", file.path().display()))?;
            builtin_ids.insert(desc.id.clone());
            adapters.insert(desc.id.clone(), desc);
        }

        let user_dir = paths::user_adapters_dir();
        if user_dir.is_dir() {
            for entry in fs::read_dir(&user_dir)
                .with_context(|| format!("reading {}", user_dir.display()))?
            {
                let path = entry?.path();
                if !path.is_file() || !is_yaml(path.to_string_lossy().as_ref()) {
                    continue;
                }
                // A broken user descriptor must never brick the whole registry
                // (and with it every command) — skip it with a warning. Use
                // `agentstack adapters validate <file>` to diagnose.
                let text = match fs::read_to_string(&path) {
                    Ok(t) => t,
                    Err(e) => {
                        eprintln!(
                            "warning: skipping unreadable adapter {}: {e}",
                            path.display()
                        );
                        continue;
                    }
                };
                let mut desc: AdapterDescriptor = match serde_yaml::from_str(&text) {
                    Ok(d) => d,
                    Err(e) => {
                        eprintln!("warning: skipping invalid adapter {}: {e}", path.display());
                        continue;
                    }
                };
                desc.source = AdapterSource::User(path.clone());
                adapters.insert(desc.id.clone(), desc);
            }
        }

        Ok(Registry {
            adapters,
            builtin_ids,
        })
    }

    pub fn get(&self, id: &str) -> Option<&AdapterDescriptor> {
        self.adapters.get(id)
    }

    /// Whether `id` is shipped in the binary (regardless of a user override).
    pub fn is_builtin(&self, id: &str) -> bool {
        self.builtin_ids.contains(id)
    }

    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.adapters.keys().map(String::as_str)
    }

    pub fn iter(&self) -> impl Iterator<Item = &AdapterDescriptor> {
        self.adapters.values()
    }
}

fn is_yaml(name: &str) -> bool {
    name.ends_with(".yaml") || name.ends_with(".yml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_adapters_load() {
        let reg = Registry::load().unwrap();
        assert!(reg.get("claude-code").is_some());
        assert!(reg.get("codex").is_some());
    }

    /// Every shipped harness adapter must parse and embed. Guards against a new
    /// descriptor regressing the schema (parse errors surface in `load`).
    #[test]
    fn all_shipped_adapters_present() {
        let reg = Registry::load().unwrap();
        for id in [
            "claude-code",
            "claude-desktop",
            "codex",
            "copilot-cli",
            "cursor",
            "gemini",
            "antigravity",
            "junie",
            "kiro",
            "opencode",
            "pi",
            "vscode",
            "windsurf",
        ] {
            assert!(reg.get(id).is_some(), "adapter {id} failed to load");
        }
    }

    /// A user descriptor loads with a `User` source, an override wins by id and
    /// is still flagged as a built-in id, and a broken drop-in is skipped
    /// (never bricks the registry).
    #[test]
    fn user_descriptors_load_override_and_survive_a_broken_file() {
        let _g = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", home.path());
        let adir = home.path().join("adapters");
        fs::create_dir_all(&adir).unwrap();
        fs::write(
            adir.join("my-agent.yaml"),
            "id: my-agent\ndisplay: My Agent\n",
        )
        .unwrap();
        fs::write(
            adir.join("cursor.yaml"),
            "id: cursor\ndisplay: Cursor Custom\n",
        )
        .unwrap();
        fs::write(adir.join("broken.yaml"), "id: broken:::\n").unwrap();

        let reg = Registry::load().unwrap();

        // Brand-new adapter: User source, not a built-in id.
        let mine = reg.get("my-agent").expect("custom adapter loaded");
        assert!(matches!(mine.source, AdapterSource::User(_)));
        assert!(!reg.is_builtin("my-agent"));

        // Override: the user file wins, but the id is still a built-in.
        let cur = reg.get("cursor").unwrap();
        assert!(
            matches!(cur.source, AdapterSource::User(_)),
            "override wins"
        );
        assert_eq!(cur.display, "Cursor Custom");
        assert!(reg.is_builtin("cursor"));

        // The broken file was skipped, not fatal — built-ins still load.
        let cc = reg.get("claude-code").expect("built-in survived");
        assert!(matches!(cc.source, AdapterSource::BuiltIn));

        std::env::remove_var("AGENTSTACK_HOME");
    }

    /// Project-scope paths must anchor at the PROJECT ROOT even when the caller
    /// holds the `.agentstack/` manifest dir — `.mcp.json` and `.claude/skills`
    /// nested inside `.agentstack/` are invisible to the CLIs.
    #[test]
    fn project_paths_anchor_at_root_for_agentstack_layout() {
        use crate::scope::Scope;
        use std::path::Path;
        let reg = Registry::load().unwrap();
        let desc = reg.get("claude-code").unwrap();

        let manifest_dir = Path::new("/repo/.agentstack");
        let (cfg, _) = desc.config_for(Scope::Project, manifest_dir).unwrap();
        assert_eq!(cfg, Path::new("/repo/.mcp.json"));
        let skills = desc.skills_dir_for(Scope::Project, manifest_dir).unwrap();
        assert_eq!(skills, Path::new("/repo/.claude/skills"));

        // Legacy layout: manifest at the root — paths unchanged.
        let root = Path::new("/repo");
        let (cfg, _) = desc.config_for(Scope::Project, root).unwrap();
        assert_eq!(cfg, Path::new("/repo/.mcp.json"));
    }
}
