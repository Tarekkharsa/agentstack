//! Path helpers: `~` expansion and well-known agentstack locations.

use std::path::PathBuf;

/// Expand a config path: the per-OS placeholders `{config}` (e.g.
/// `~/Library/Application Support` on macOS, `~/.config` on Linux, `%APPDATA%`
/// on Windows) and `{data}`, plus a leading `~`. Other paths are unchanged.
pub fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("{config}/") {
        if let Some(base) = dirs::config_dir() {
            return base.join(rest);
        }
    }
    if let Some(rest) = path.strip_prefix("{data}/") {
        if let Some(base) = dirs::data_dir() {
            return base.join(rest);
        }
    }
    if path == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
    }
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(path)
}

/// `~/.agentstack` — where state, backups, and user adapters live. Honors the
/// `AGENTSTACK_HOME` env var (handy for tests, CI, and relocating state).
pub fn agentstack_home() -> PathBuf {
    if let Some(dir) = std::env::var_os("AGENTSTACK_HOME") {
        return PathBuf::from(dir);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".agentstack")
}

/// `~/.agentstack/adapters` — user-supplied descriptor overrides/additions.
pub fn user_adapters_dir() -> PathBuf {
    agentstack_home().join("adapters")
}

/// `~/.agentstack/backups` — pre-write copies of configs we modify.
pub fn backups_dir() -> PathBuf {
    agentstack_home().join("backups")
}

/// `~/.agentstack/lib` — the central capability library. Projects reference its
/// contents by name instead of copying capability files (see
/// `docs/reference.md#the-central-library`). Library commands populate
/// `lib/skills/` and `library.toml`.
pub fn lib_home() -> PathBuf {
    agentstack_home().join("lib")
}

/// `~/.agentstack/lib/skills` — skill bodies in the central library, referenced
/// by name from project manifests.
pub fn lib_skills_home() -> PathBuf {
    lib_home().join("skills")
}

/// `~/.agentstack/lib/servers` — reusable MCP server definitions in the central
/// library (Phase 1b), referenced by name from project manifests. Each
/// `<name>.toml` holds a server definition with `${REF}` secrets only.
pub fn lib_servers_home() -> PathBuf {
    lib_home().join("servers")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_tilde() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(
            expand_tilde("~/.codex/config.toml"),
            home.join(".codex/config.toml")
        );
        assert_eq!(expand_tilde("/abs/path"), PathBuf::from("/abs/path"));
    }

    #[test]
    fn lib_paths_hang_off_home_and_honor_override() {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var("AGENTSTACK_HOME", "/tmp/as-home");
        assert_eq!(lib_home(), PathBuf::from("/tmp/as-home/lib"));
        assert_eq!(lib_skills_home(), PathBuf::from("/tmp/as-home/lib/skills"));
        assert_eq!(
            lib_servers_home(),
            PathBuf::from("/tmp/as-home/lib/servers")
        );
        // lib/ lives under the same home as the other managed stores.
        assert_eq!(lib_home(), agentstack_home().join("lib"));
        std::env::remove_var("AGENTSTACK_HOME");
    }
}
