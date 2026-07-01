//! Project-review finding: `doctor --ci` is documented as a trust gate, but it
//! used to downgrade *every* manifest-validation issue to a warning and only
//! exit nonzero on `report.errors > 0` — so a structural error (e.g. a profile
//! referencing an undefined server) passed `--ci`. These tests pin the fix:
//! structural issues count as errors and fail `--ci`, while a clean manifest
//! passes. Non-CI `doctor` never fails on validation regardless.

use std::fs;
use std::sync::Mutex;

use agentstack::cli::DoctorArgs;
use agentstack::commands::doctor;

// doctor mutates the process-global HOME; serialize these tests.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn setup(home: &std::path::Path) {
    fs::create_dir_all(home).unwrap();
    std::env::set_var("HOME", home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));
}

fn ci_args() -> DoctorArgs {
    DoctorArgs {
        ci: true,
        live: false,
        fix: false,
    }
}

/// A profile whose `servers` list names a server that is never defined — this
/// is an `UnknownServerRef`, a structural (is_error) validation issue.
fn write_manifest_with_structural_error(proj: &std::path::Path) {
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [profiles.work]\nservers = [\"ghost\"]\n",
    )
    .unwrap();
}

fn write_clean_manifest(proj: &std::path::Path) {
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n",
    )
    .unwrap();
}

#[test]
fn ci_fails_on_structural_validation_error() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    setup(&tmp.path().join("home"));

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    write_manifest_with_structural_error(&proj);

    let err = doctor::run(&ci_args(), Some(&proj)).unwrap_err();
    assert!(
        err.to_string().contains("error"),
        "doctor --ci must fail the trust gate on a structural validation error, got: {err}"
    );
}

#[test]
fn ci_passes_on_clean_manifest() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    setup(&tmp.path().join("home"));

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    write_clean_manifest(&proj);

    doctor::run(&ci_args(), Some(&proj)).expect("doctor --ci must pass on a clean manifest");
}

/// Without `--ci`, a structural validation error is reported but never fails
/// the command — the gate is opt-in.
#[test]
fn non_ci_never_fails_on_validation_error() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    setup(&tmp.path().join("home"));

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    write_manifest_with_structural_error(&proj);

    let dargs = DoctorArgs {
        ci: false,
        live: false,
        fix: false,
    };
    doctor::run(&dargs, Some(&proj)).expect("non-CI doctor must not fail on a validation error");
}
