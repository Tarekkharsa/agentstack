//! Layered manifest loading.
//!
//! The shared, committed `agentstack.toml` is deep-merged with an optional,
//! gitignored `agentstack.local.toml` overlay (per-machine servers, path
//! differences, target subsets). The merge happens at the [`toml::Value`] level
//! so the overlay can touch any field without the model knowing about it.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::model::Manifest;

pub const MANIFEST_FILE: &str = "agentstack.toml";
/// Newest manifest schema version this build reads and writes. Anything above
/// it was authored by a future agentstack whose semantics this build cannot
/// know, so [`load_from_dir`] refuses it instead of misinterpreting silently.
pub const SUPPORTED_MANIFEST_VERSION: u32 = 1;
pub const LOCAL_FILE: &str = "agentstack.local.toml";
/// Preferred subdirectory holding the manifest and all agentstack-managed files
/// (lock, `skills/`, `instructions/`, `.env`). A repo opts in by placing its
/// manifest here; the legacy root layout is still discovered for back-compat.
pub const MANIFEST_SUBDIR: &str = ".agentstack";

/// Resolve the directory that holds an *existing* manifest, given a project/base
/// dir. Prefers `<base>/.agentstack/` (the new layout) when it actually contains
/// a manifest, otherwise falls back to the legacy root `<base>/`. When neither
/// has a manifest, returns the legacy root so callers' "no manifest" errors point
/// at the conventional path.
pub fn resolve_manifest_dir(base: &Path) -> PathBuf {
    let nested = base.join(MANIFEST_SUBDIR);
    if nested.join(MANIFEST_FILE).exists() {
        nested
    } else {
        base.to_path_buf()
    }
}

/// Resolve the directory where a *new* manifest should be created. Keeps using a
/// legacy root manifest if one already exists there; otherwise prefers the new
/// `<base>/.agentstack/` layout.
pub fn new_manifest_dir(base: &Path) -> PathBuf {
    if base.join(MANIFEST_FILE).exists() {
        base.to_path_buf()
    } else {
        base.join(MANIFEST_SUBDIR)
    }
}

/// Walk upward from `start` to the filesystem root looking for a project that
/// carries a manifest (`.agentstack/agentstack.toml` preferred, legacy root
/// `agentstack.toml` accepted). Returns the project BASE dir — the dir you'd
/// hand to [`resolve_manifest_dir`] / `commands::load` — not the manifest dir.
/// This is how the zero-files bridge follows the agent into a repo when it was
/// launched from a subdirectory (or a GUI harness's own cwd).
///
/// The walk stops AT the `$HOME` layer without matching it, and likewise stops
/// empty-handed at the machine home itself (`~/.agentstack`, or a relocated
/// `AGENTSTACK_HOME`): the home manifest (`~/.agentstack/agentstack.toml`,
/// seeded by `init --global`) is the personal machine-level layer, not a
/// project — it must never be discovered (and so never offered for `trust`,
/// never activated) by the zero-files bridge, even for sessions rooted inside
/// it.
pub fn discover_project_base(start: &Path) -> Option<PathBuf> {
    discover_project_base_below(start, dirs::home_dir().as_deref())
}

fn discover_project_base_below(start: &Path, home: Option<&Path>) -> Option<PathBuf> {
    // The machine home (~/.agentstack, or wherever AGENTSTACK_HOME relocated
    // it) carries the machine manifest at its ROOT, so it matches the
    // legacy-root check below before the walk ever reaches $HOME. Compare
    // canonicalized so symlinked spellings still match.
    let machine_home = crate::util::paths::agentstack_home();
    let machine_home_canon = machine_home.canonicalize().ok();
    let mut cur = Some(start);
    while let Some(dir) = cur {
        if home == Some(dir) {
            return None;
        }
        // Checked BEFORE the manifest-existence test: a session rooted at or
        // below the machine home (e.g. editing personal fragments) must never
        // discover the machine manifest as a project — it would become
        // trustable/spawnable via the zero-files bridge.
        if dir == machine_home
            || (machine_home_canon.is_some() && dir.canonicalize().ok() == machine_home_canon)
        {
            return None;
        }
        if dir.join(MANIFEST_SUBDIR).join(MANIFEST_FILE).exists()
            || dir.join(MANIFEST_FILE).exists()
        {
            return Some(dir.to_path_buf());
        }
        cur = dir.parent();
    }
    None
}

/// The project root a manifest dir belongs to: the parent for the
/// `.agentstack/` layout, the dir itself for a legacy root manifest. This is
/// the anchor for everything project-scoped (`.mcp.json`, `.claude/skills/`,
/// `.gitignore`).
pub fn project_root_of(manifest_dir: &Path) -> PathBuf {
    if manifest_dir.file_name().and_then(|n| n.to_str()) == Some(MANIFEST_SUBDIR) {
        match manifest_dir.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
            _ => manifest_dir.to_path_buf(),
        }
    } else {
        manifest_dir.to_path_buf()
    }
}

/// Result of a layered load, keeping the resolved manifest plus provenance.
#[derive(Debug)]
pub struct LoadedManifest {
    pub manifest: Manifest,
    pub manifest_path: PathBuf,
    pub local_path: Option<PathBuf>,
    /// The machine-level manifest whose `[instructions]` merged in beneath
    /// this one via [`merge_user_layer`]; `None` when that layer is absent,
    /// wasn't merged, or IS this manifest.
    pub user_path: Option<PathBuf>,
}

/// Load `agentstack.toml` from `dir`, deep-merging `agentstack.local.toml` over
/// it when present.
pub fn load_from_dir(dir: &Path) -> Result<LoadedManifest> {
    let manifest_path = dir.join(MANIFEST_FILE);
    let base_text = fs::read_to_string(&manifest_path).with_context(|| {
        format!(
            "no manifest here (looked for {}) — run `agentstack init` to create \
             .agentstack/agentstack.toml, or point at one with --manifest-dir",
            manifest_path.display()
        )
    })?;
    let mut base: toml::Value = toml::from_str(&base_text)
        .with_context(|| format!("parsing {}", manifest_path.display()))?;

    let local_path = dir.join(LOCAL_FILE);
    let local_path = if local_path.exists() {
        let local_text = fs::read_to_string(&local_path)
            .with_context(|| format!("reading {}", local_path.display()))?;
        let overlay: toml::Value = toml::from_str(&local_text)
            .with_context(|| format!("parsing {}", local_path.display()))?;
        merge_value(&mut base, overlay);
        Some(local_path)
    } else {
        None
    };

    let manifest: Manifest = base
        .try_into()
        .context("manifest does not match the expected schema")?;
    crate::util::check_schema_version(
        manifest.version,
        SUPPORTED_MANIFEST_VERSION,
        "manifest",
        &manifest_path,
    )?;

    Ok(LoadedManifest {
        manifest,
        manifest_path,
        local_path,
        user_path: None,
    })
}

/// Merge the machine-level manifest's `[instructions]` — and ONLY those —
/// beneath an already-loaded project manifest. Layer order is user → project
/// → project-local (the project side of that chain is already collapsed in
/// `loaded`), so a project fragment of the same name wins outright: a project
/// that redefines a fragment fully owns it, which is more predictable than a
/// field-by-field splice of personal and team content. Inherited fragments
/// are flagged `from_user_layer` (compiled at global scope only, see
/// `render::instructions`), listed FIRST (machine-wide rules before project
/// rules), and their relative paths are re-anchored at the machine layer.
///
/// Servers, skills, settings, and hooks are deliberately NOT inherited:
/// personal capabilities must never auto-inject into a team project, and the
/// trust digest doesn't cover this layer — it must never widen the runtime
/// surface. Called by `commands::load` (every command's context), not by
/// [`load_from_dir`], so primitive loads (trust review, the machine layer
/// itself) stay single-layer.
///
/// A missing or unparseable machine layer is a silent no-op — a broken
/// personal file must not take every project down — as is loading the machine
/// manifest itself.
pub fn merge_user_layer(loaded: &mut LoadedManifest) {
    let home = crate::util::paths::agentstack_home();
    let user_manifest = home.join(MANIFEST_FILE);
    if !user_manifest.exists() {
        return;
    }
    // Never merge the layer beneath itself (canonicalize survives symlinked
    // temp dirs and `~` spellings).
    let project_dir = loaded
        .manifest_path
        .parent()
        .unwrap_or_else(|| Path::new("."));
    let same = match (home.canonicalize(), project_dir.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => home == project_dir,
    };
    if same {
        return;
    }
    let Ok(text) = fs::read_to_string(&user_manifest) else {
        return;
    };
    let Ok(user) = toml::from_str::<Manifest>(&text) else {
        return;
    };
    // Same no-op policy for an unsupported schema version: a machine layer
    // authored by a future agentstack must neither be misread nor take every
    // project down.
    if crate::util::check_schema_version(
        user.version,
        SUPPORTED_MANIFEST_VERSION,
        "manifest",
        &user_manifest,
    )
    .is_err()
    {
        return;
    }
    if user.instructions.is_empty() {
        return;
    }

    let mut merged = indexmap::IndexMap::new();
    for (name, mut instr) in user.instructions {
        if loaded.manifest.instructions.contains_key(&name) {
            continue; // the project's definition wins
        }
        let p = Path::new(&instr.path);
        if !p.is_absolute() {
            let rel = p.strip_prefix("./").unwrap_or(p);
            instr.path = home.join(rel).display().to_string();
        }
        instr.from_user_layer = true;
        merged.insert(name, instr);
    }
    if merged.is_empty() {
        return;
    }
    merged.extend(std::mem::take(&mut loaded.manifest.instructions));
    loaded.manifest.instructions = merged;
    loaded.user_path = Some(user_manifest);
}

/// Deep-merge `overlay` into `base`. Tables merge key-by-key (recursively);
/// every other value (including arrays) is replaced wholesale by the overlay.
fn merge_value(base: &mut toml::Value, overlay: toml::Value) {
    match (base, overlay) {
        (toml::Value::Table(base_tbl), toml::Value::Table(overlay_tbl)) => {
            for (k, v) in overlay_tbl {
                match base_tbl.get_mut(&k) {
                    Some(existing) => merge_value(existing, v),
                    None => {
                        base_tbl.insert(k, v);
                    }
                }
            }
        }
        (base_slot, overlay_val) => {
            *base_slot = overlay_val;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_prefers_nested_then_falls_back_to_root() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let base = tmp.path();

        // Nothing yet → legacy root (so "no manifest" errors point at the root).
        assert_eq!(resolve_manifest_dir(base), base.to_path_buf());
        // New manifests are created under `.agentstack/`.
        assert_eq!(new_manifest_dir(base), base.join(MANIFEST_SUBDIR));

        // Legacy root manifest present → both resolve to root.
        fs::write(base.join(MANIFEST_FILE), "version = 1\n").unwrap();
        assert_eq!(resolve_manifest_dir(base), base.to_path_buf());
        assert_eq!(new_manifest_dir(base), base.to_path_buf());

        // `.agentstack/` manifest present → preferred over a missing root one.
        let tmp2 = assert_fs::TempDir::new().unwrap();
        let base2 = tmp2.path();
        let nested = base2.join(MANIFEST_SUBDIR);
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join(MANIFEST_FILE), "version = 1\n").unwrap();
        assert_eq!(resolve_manifest_dir(base2), nested);
        assert_eq!(new_manifest_dir(base2), nested);
    }

    #[test]
    fn discover_walks_up_to_the_project_base() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let base = tmp.path();
        let nested = base.join(MANIFEST_SUBDIR);
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join(MANIFEST_FILE), "version = 1\n").unwrap();
        let deep = base.join("src/render/deeper");
        fs::create_dir_all(&deep).unwrap();

        // From the base itself and from a deep subdir → the same base.
        assert_eq!(discover_project_base(base), Some(base.to_path_buf()));
        assert_eq!(discover_project_base(&deep), Some(base.to_path_buf()));

        // A tree with no manifest anywhere above → None. (TempDirs live under
        // the system temp root, which carries no manifest.)
        let bare = assert_fs::TempDir::new().unwrap();
        assert_eq!(discover_project_base(bare.path()), None);
    }

    #[test]
    fn discover_never_surfaces_the_home_layer() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let home = tmp.path().join("home");
        // The machine-level manifest lives at ~/.agentstack/agentstack.toml…
        fs::create_dir_all(home.join(MANIFEST_SUBDIR)).unwrap();
        fs::write(
            home.join(MANIFEST_SUBDIR).join(MANIFEST_FILE),
            "version = 1\n",
        )
        .unwrap();
        let deep = home.join("code/somewhere/deep");
        fs::create_dir_all(&deep).unwrap();

        // …but it is not a project: the walk-up stops at $HOME empty-handed,
        // from a subdirectory and from $HOME itself.
        assert_eq!(discover_project_base_below(&deep, Some(&home)), None);
        assert_eq!(discover_project_base_below(&home, Some(&home)), None);

        // A real project below $HOME is still discovered normally.
        let proj = home.join("code/proj");
        fs::create_dir_all(proj.join(MANIFEST_SUBDIR)).unwrap();
        fs::write(
            proj.join(MANIFEST_SUBDIR).join(MANIFEST_FILE),
            "version = 1\n",
        )
        .unwrap();
        let inner = proj.join("src");
        fs::create_dir_all(&inner).unwrap();
        assert_eq!(discover_project_base_below(&inner, Some(&home)), Some(proj));
    }

    #[test]
    fn discovery_inside_the_default_machine_home_never_matches_it() {
        // ~/.agentstack carries the machine manifest at its ROOT, so it matches
        // the legacy-root check before the walk ever reaches $HOME. A session
        // rooted at or below it (e.g. editing personal fragments) must not
        // discover the machine manifest as a project — that would make the
        // personal layer trustable/spawnable via the zero-files bridge.
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let prev_home = std::env::var_os("HOME");
        let prev_as_home = std::env::var_os("AGENTSTACK_HOME");

        let tmp = assert_fs::TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let machine = home.join(".agentstack");
        fs::create_dir_all(machine.join("instructions")).unwrap();
        fs::write(machine.join(MANIFEST_FILE), "version = 1\n").unwrap();
        // Default layout: agentstack_home() derives from $HOME.
        std::env::set_var("HOME", &home);
        std::env::remove_var("AGENTSTACK_HOME");

        assert_eq!(
            discover_project_base_below(&machine.join("instructions"), Some(&home)),
            None,
            "a walk started below ~/.agentstack must not surface the machine manifest"
        );
        assert_eq!(
            discover_project_base_below(&machine, Some(&home)),
            None,
            "a walk started at ~/.agentstack itself must not surface it either"
        );

        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match prev_as_home {
            Some(v) => std::env::set_var("AGENTSTACK_HOME", v),
            None => std::env::remove_var("AGENTSTACK_HOME"),
        }
    }

    #[test]
    fn discovery_inside_a_relocated_machine_home_never_matches_it() {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let prev_as_home = std::env::var_os("AGENTSTACK_HOME");

        let tmp = assert_fs::TempDir::new().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();
        // AGENTSTACK_HOME relocated outside $HOME entirely.
        let machine = tmp.path().join("relocated/as-home");
        fs::create_dir_all(machine.join("instructions")).unwrap();
        fs::write(machine.join(MANIFEST_FILE), "version = 1\n").unwrap();
        std::env::set_var("AGENTSTACK_HOME", &machine);

        assert_eq!(
            discover_project_base_below(&machine.join("instructions"), Some(&home)),
            None
        );
        assert_eq!(discover_project_base_below(&machine, Some(&home)), None);

        // A real project elsewhere is still discovered normally.
        let proj = tmp.path().join("relocated/proj");
        fs::create_dir_all(proj.join(MANIFEST_SUBDIR)).unwrap();
        fs::write(
            proj.join(MANIFEST_SUBDIR).join(MANIFEST_FILE),
            "version = 1\n",
        )
        .unwrap();
        let inner = proj.join("src");
        fs::create_dir_all(&inner).unwrap();
        assert_eq!(discover_project_base_below(&inner, Some(&home)), Some(proj));

        match prev_as_home {
            Some(v) => std::env::set_var("AGENTSTACK_HOME", v),
            None => std::env::remove_var("AGENTSTACK_HOME"),
        }
    }

    #[test]
    fn load_checks_the_manifest_schema_version() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let path = tmp.path().join(MANIFEST_FILE);

        // The current version loads.
        fs::write(&path, "version = 1\n").unwrap();
        assert!(load_from_dir(tmp.path()).is_ok());

        // A future version is refused with an actionable message, not
        // silently misread.
        fs::write(&path, "version = 99\n").unwrap();
        let err = load_from_dir(tmp.path()).unwrap_err().to_string();
        assert!(err.contains("version 99"), "{err}");
        assert!(err.contains("newer than this agentstack build"), "{err}");
        assert!(err.contains("upgrade agentstack"), "{err}");
        assert!(err.contains(MANIFEST_FILE), "{err}");

        // Version 0 never named a real schema.
        fs::write(&path, "version = 0\n").unwrap();
        let err = load_from_dir(tmp.path()).unwrap_err().to_string();
        assert!(err.contains("version 0"), "{err}");

        // A manifest with no version at all fails deserialization (the field
        // is required), so it can never sneak past the check.
        fs::write(&path, "[servers]\n").unwrap();
        let err = format!("{:#}", load_from_dir(tmp.path()).unwrap_err());
        assert!(err.contains("version"), "{err}");
    }

    #[test]
    fn overlay_cannot_bypass_the_version_check() {
        // The overlay merges before deserialization, so a local file bumping
        // the version is caught by the same guard.
        let tmp = assert_fs::TempDir::new().unwrap();
        fs::write(tmp.path().join(MANIFEST_FILE), "version = 1\n").unwrap();
        fs::write(tmp.path().join(LOCAL_FILE), "version = 99\n").unwrap();
        let err = load_from_dir(tmp.path()).unwrap_err().to_string();
        assert!(err.contains("version 99"), "{err}");
    }

    #[test]
    fn overlay_adds_and_overrides() {
        let mut base: toml::Value = toml::from_str(
            r#"
            version = 1
            [servers.kibana]
            type = "http"
            url = "https://old"
            "#,
        )
        .unwrap();
        let overlay: toml::Value = toml::from_str(
            r#"
            [servers.kibana]
            url = "https://new"
            [servers.local-only]
            type = "stdio"
            command = "node"
            "#,
        )
        .unwrap();
        merge_value(&mut base, overlay);
        let m: Manifest = base.try_into().unwrap();
        assert_eq!(m.servers["kibana"].url.as_deref(), Some("https://new"));
        assert!(m.servers.contains_key("local-only"));
    }
}
