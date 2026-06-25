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
}
