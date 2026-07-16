// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Regression for issue #2: `setup`/`bootstrap` preflight must resolve
//! plugin-declared skills from the central library exactly the way `doctor` and
//! `apply` do. The bug was that the shared preflight validated with an
//! inline-only view (`validate_with_targets`), so a `[plugins.X]` recipe whose
//! `skills` are satisfied only by `~/.agentstack/lib` was hard-flagged as
//! "unknown skill" — blocking setup — while `doctor`/`apply` rendered fine.
//!
//! This exercises the same composition the fixed preflight uses: a real loaded
//! `Context`, its `library_ctx()`, and `validate_with_context`.

use std::fs;
use std::sync::Mutex;

use agentstack::manifest::{validate_with_context, validate_with_targets, IssueKind};

// `library_ctx()` reads the process-global AGENTSTACK_HOME; serialize the tests
// in this binary so they don't race on it.
static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn plugin_skill_from_central_library_does_not_block_setup() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();

    // A central library home holding one path-source skill, referenced by name.
    let ashome = tmp.path().join(".agentstack");
    let lib_home = ashome.join("lib");
    fs::create_dir_all(lib_home.join("skills/cloudflare-agents-sdk")).unwrap();
    fs::write(
        lib_home.join("skills/cloudflare-agents-sdk/SKILL.md"),
        "# body\n",
    )
    .unwrap();
    fs::write(
        lib_home.join("library.toml"),
        "version = 1\n\
         [[skill]]\n\
         name = \"cloudflare-agents-sdk\"\n\
         source = \"path\"\n\
         path = \"cloudflare-agents-sdk\"\n",
    )
    .unwrap();
    std::env::set_var("AGENTSTACK_HOME", &ashome);

    // A project whose plugin recipe references a skill that exists ONLY in the
    // central library — the exact shape the issue reproduces.
    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n\
         [plugins.cloudflare]\n\
         version = \"1.0.0\"\n\
         description = \"Cloudflare workflow\"\n\
         skills = [\"cloudflare-agents-sdk\"]\n",
    )
    .unwrap();

    let ctx = agentstack::commands::load(Some(&proj)).unwrap();
    let manifest = &ctx.loaded.manifest;

    // Inline-only view (the pre-fix preflight): the library-only skill is
    // wrongly flagged as unknown. This documents the bug.
    assert!(
        validate_with_targets(manifest, ctx.registry.ids())
            .iter()
            .any(|i| i.kind == IssueKind::UnknownSkillRef),
        "inline-only validation should still flag the library-only skill"
    );

    // Library-aware view (the fixed preflight, mirroring doctor/apply): the
    // plugin skill resolves from the central library, so validation is clean.
    let libctx = ctx.library_ctx();
    let vctx = libctx.validate_ctx(&ctx.dir);
    let issues = validate_with_context(manifest, ctx.registry.ids(), &vctx);
    assert!(
        !issues.iter().any(|i| i.kind == IssueKind::UnknownSkillRef),
        "plugin skill present in the central library must not be flagged unknown: {issues:?}"
    );

    std::env::remove_var("AGENTSTACK_HOME");
}
