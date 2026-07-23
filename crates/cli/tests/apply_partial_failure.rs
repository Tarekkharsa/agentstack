// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Stage 1.2 witness: a hard write failure on one target must not hide the
//! targets that succeeded or leave ownership ambiguous. The pass continues,
//! the successful writes land in the undo ledger and ownership state, the
//! summary names the failed target, and the command still exits nonzero.

use std::fs;
use std::path::Path;
use std::sync::Mutex;

use agentstack::cli::ApplyArgs;
use agentstack::commands::apply;
use agentstack::history;

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Restore directory permissions on drop so the TempDir can clean up even if
/// an assertion panics first.
struct RestorePerms<'a>(&'a Path);
impl Drop for RestorePerms<'_> {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(self.0, fs::Permissions::from_mode(0o755));
    }
}

#[test]
fn one_failed_target_reports_continues_and_keeps_successes_undoable() {
    use std::os::unix::fs::PermissionsExt;

    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    // Claude's global config is writable; Codex's lives in a directory made
    // read-only, so its atomic write (temp file + rename in that directory)
    // fails with a real I/O error while its config stays readable.
    fs::write(home.join(".claude.json"), "{}").unwrap();
    let codex_dir = home.join(".codex");
    fs::create_dir_all(&codex_dir).unwrap();
    fs::write(codex_dir.join("config.toml"), "").unwrap();
    fs::set_permissions(&codex_dir, fs::Permissions::from_mode(0o555)).unwrap();
    let _restore = RestorePerms(&codex_dir);

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        r#"version = 1

[servers.srv]
type = "stdio"
command = "npx"
args = ["srv-mcp"]

[targets]
default = ["claude-code", "codex"]
"#,
    )
    .unwrap();

    let err = apply::run(
        &ApplyArgs {
            targets: Vec::new(),
            profile: None,
            dry_run: false,
            write: true,
            scope: Some(agentstack::scope::Scope::Global),
            allow_unresolved: false,
            prune_foreign: false,
            no_gitignore: false,
        },
        Some(&proj),
    )
    .expect_err("a failed target write must exit nonzero");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("write failed on 1 target(s)") && msg.contains("Codex"),
        "the error names the failed target: {msg}"
    );

    // The successful target's write landed…
    let claude = fs::read_to_string(home.join(".claude.json")).unwrap();
    assert!(claude.contains("srv"), "Claude write landed: {claude}");
    // …the failed target's config is untouched…
    assert_eq!(
        fs::read_to_string(codex_dir.join("config.toml")).unwrap(),
        ""
    );

    // …and ownership is unambiguous: the undo ledger records exactly the
    // write that happened (Claude), never the one that failed (Codex).
    let entries = history::list();
    let newest = entries.first().expect("the partial apply was recorded");
    assert!(!newest.undone);
    assert!(
        newest
            .files
            .iter()
            .any(|f| f.path.ends_with(".claude.json")),
        "ledger holds the successful write: {:?}",
        newest.files
    );
    assert!(
        !newest.files.iter().any(|f| f.path.contains(".codex")),
        "a failed write must not enter the undo ledger: {:?}",
        newest.files
    );

    std::env::remove_var("HOME");
    std::env::remove_var("AGENTSTACK_HOME");
}
