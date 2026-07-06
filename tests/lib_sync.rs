//! `agentstack lib sync` versions the central library (`~/.agentstack/lib`) as a
//! git repo and moves it across machines — without ever committing the content
//! store cache (a sibling dir) or a resolved secret.

use std::fs;
use std::sync::Mutex;

use agentstack::cli::{LibArgs, LibKind, LibSyncArgs};
use agentstack::commands::lib::{self, add_skill, LibSource};

// AGENTSTACK_HOME is process-global; serialize the tests in this binary.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn git_identity() {
    std::env::set_var("GIT_AUTHOR_NAME", "t");
    std::env::set_var("GIT_AUTHOR_EMAIL", "t@e.st");
    std::env::set_var("GIT_COMMITTER_NAME", "t");
    std::env::set_var("GIT_COMMITTER_EMAIL", "t@e.st");
}

fn sync(init: bool, remote: Option<&str>, status: bool) -> LibArgs {
    LibArgs {
        kind: LibKind::Sync(LibSyncArgs {
            init,
            remote: remote.map(str::to_string),
            status,
            message: None,
            allow_secrets: false,
        }),
    }
}

fn git(args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

#[test]
fn sync_pushes_the_library_and_excludes_the_store_cache() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    git_identity();

    let tmp = assert_fs::TempDir::new().unwrap();
    let ashome = tmp.path().join("ashome");
    fs::create_dir_all(&ashome).unwrap();
    std::env::set_var("AGENTSTACK_HOME", &ashome);
    let lib_home = ashome.join("lib");

    // Seed a skill into the library.
    let src = tmp.path().join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("SKILL.md"), "# demo\n").unwrap();
    add_skill(&lib_home, "demo", LibSource::Path(&src), false, true, false).unwrap();

    // A content-store cache sibling that must NOT travel.
    let cache = ashome.join("store/git/fake");
    fs::create_dir_all(&cache).unwrap();
    fs::write(cache.join("blob"), "cached\n").unwrap();

    // A bare remote.
    let bare = tmp.path().join("remote.git");
    git(&["init", "-q", "--bare", &bare.to_string_lossy()]);
    let url = format!("file://{}", bare.display());

    // init (local) + push.
    lib::run(&sync(true, Some(&url), false), None).unwrap();
    lib::run(&sync(false, None, false), None).unwrap();

    // Clone as a second machine.
    let dest = tmp.path().join("machine2");
    git(&["clone", "-q", &url, &dest.to_string_lossy()]);

    assert!(dest.join("library.toml").is_file(), "index traveled");
    assert!(
        dest.join("skills/demo/SKILL.md").is_file(),
        "skill body traveled"
    );
    assert!(
        !dest.join("store").exists(),
        "the content-store cache must never travel"
    );

    std::env::remove_var("AGENTSTACK_HOME");
}

#[test]
fn init_with_remote_clones_into_an_empty_library() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    git_identity();

    let tmp = assert_fs::TempDir::new().unwrap();

    // A remote that already holds a library (populated from a scratch repo).
    let bare = tmp.path().join("remote.git");
    git(&["init", "-q", "--bare", &bare.to_string_lossy()]);
    let url = format!("file://{}", bare.display());
    let seed = tmp.path().join("seed");
    fs::create_dir_all(seed.join("skills/demo")).unwrap();
    fs::write(seed.join("library.toml"), "version = 1\n").unwrap();
    fs::write(seed.join("skills/demo/SKILL.md"), "# demo\n").unwrap();
    for a in [
        vec!["-C", seed.to_str().unwrap(), "init", "-q"],
        vec!["-C", seed.to_str().unwrap(), "add", "-A"],
        vec!["-C", seed.to_str().unwrap(), "commit", "-qm", "seed"],
        vec![
            "-C",
            seed.to_str().unwrap(),
            "remote",
            "add",
            "origin",
            &url,
        ],
        vec![
            "-C",
            seed.to_str().unwrap(),
            "push",
            "-q",
            "-u",
            "origin",
            "HEAD",
        ],
    ] {
        git(&a);
    }

    // A fresh machine: AGENTSTACK_HOME with no library yet.
    let ashome = tmp.path().join("fresh");
    fs::create_dir_all(&ashome).unwrap();
    std::env::set_var("AGENTSTACK_HOME", &ashome);

    // --init --remote into the empty library clones it.
    lib::run(&sync(true, Some(&url), false), None).unwrap();

    let lib_home = ashome.join("lib");
    assert!(lib_home.join("library.toml").is_file(), "library cloned");
    assert!(lib_home.join("skills/demo/SKILL.md").is_file());

    std::env::remove_var("AGENTSTACK_HOME");
}

#[test]
fn sync_blocks_a_literal_secret_from_travelling() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    git_identity();
    let tmp = assert_fs::TempDir::new().unwrap();
    let ashome = tmp.path().join("ashome");
    let servers = ashome.join("lib/servers");
    fs::create_dir_all(&servers).unwrap();
    std::env::set_var("AGENTSTACK_HOME", &ashome);
    fs::write(
        ashome.join("lib/library.toml"),
        "version = 1\nserver = []\n",
    )
    .unwrap();
    // A server definition with a plaintext token instead of a ${REF}.
    fs::write(
        servers.join("leaky.toml"),
        "type = \"http\"\nurl = \"https://x/mcp\"\n\
         [headers]\nAuthorization = \"Bearer sk-REALSECRET\"\n",
    )
    .unwrap();

    lib::run(&sync(true, None, false), None).unwrap(); // init
    let err = lib::run(&sync(false, None, false), None).unwrap_err();
    assert!(
        err.to_string().contains("literal secret"),
        "sync must refuse to push a plaintext secret: {err}"
    );

    std::env::remove_var("AGENTSTACK_HOME");
}

#[test]
fn sync_blocks_a_secret_in_a_url_query_param() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    git_identity();
    let tmp = assert_fs::TempDir::new().unwrap();
    let ashome = tmp.path().join("ashome");
    let servers = ashome.join("lib/servers");
    fs::create_dir_all(&servers).unwrap();
    std::env::set_var("AGENTSTACK_HOME", &ashome);
    fs::write(ashome.join("lib/library.toml"), "version = 1\nserver = []\n").unwrap();
    fs::write(
        servers.join("leaky.toml"),
        "type = \"http\"\nurl = \"https://x/mcp?api_key=sk-REALSECRET\"\n",
    )
    .unwrap();

    lib::run(&sync(true, None, false), None).unwrap();
    let err = lib::run(&sync(false, None, false), None).unwrap_err();
    assert!(
        err.to_string().contains("literal secret"),
        "a secretish url query param must block the push: {err}"
    );

    std::env::remove_var("AGENTSTACK_HOME");
}

#[test]
fn sync_blocks_a_secret_in_args_and_allows_a_ref() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    git_identity();
    let tmp = assert_fs::TempDir::new().unwrap();
    let ashome = tmp.path().join("ashome");
    let servers = ashome.join("lib/servers");
    fs::create_dir_all(&servers).unwrap();
    std::env::set_var("AGENTSTACK_HOME", &ashome);
    fs::write(ashome.join("lib/library.toml"), "version = 1\nserver = []\n").unwrap();
    // A literal secret in the value following a secretish flag.
    fs::write(
        servers.join("leaky.toml"),
        "type = \"stdio\"\ncommand = \"mcp\"\nargs = [\"--token\", \"sk-REALSECRET\"]\n",
    )
    .unwrap();

    lib::run(&sync(true, None, false), None).unwrap();
    let err = lib::run(&sync(false, None, false), None).unwrap_err();
    assert!(
        err.to_string().contains("literal secret"),
        "a secretish flag value in args must block the push: {err}"
    );

    // The same shape with a ${REF} value must NOT block (url + args both exempt).
    fs::write(
        servers.join("leaky.toml"),
        "type = \"stdio\"\ncommand = \"mcp\"\n\
         url = \"https://x/mcp?api_key=${TOK}\"\nargs = [\"--token\", \"${TOK}\"]\n",
    )
    .unwrap();
    // No remote, so sync stops before pushing but only after clearing the gate.
    lib::run(&sync(false, None, false), None).unwrap();

    std::env::remove_var("AGENTSTACK_HOME");
}

#[test]
fn sync_blocks_an_unparseable_server_definition() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    git_identity();
    let tmp = assert_fs::TempDir::new().unwrap();
    let ashome = tmp.path().join("ashome");
    let servers = ashome.join("lib/servers");
    fs::create_dir_all(&servers).unwrap();
    std::env::set_var("AGENTSTACK_HOME", &ashome);
    fs::write(ashome.join("lib/library.toml"), "version = 1\nserver = []\n").unwrap();
    // Hand-broken TOML (unterminated table header) carrying a plaintext secret.
    fs::write(
        servers.join("broken.toml"),
        "type = \"http\"\n[headers\nAuthorization = \"Bearer sk-REALSECRET\"\n",
    )
    .unwrap();

    lib::run(&sync(true, None, false), None).unwrap();
    let err = lib::run(&sync(false, None, false), None).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("cannot be parsed"),
        "an unparseable server def must fail closed: {msg}"
    );

    std::env::remove_var("AGENTSTACK_HOME");
}

#[test]
fn sync_blocks_a_secret_left_in_outgoing_history() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    git_identity();
    let tmp = assert_fs::TempDir::new().unwrap();
    let ashome = tmp.path().join("ashome");
    let lib_home = ashome.join("lib");
    let servers = lib_home.join("servers");
    fs::create_dir_all(&servers).unwrap();
    std::env::set_var("AGENTSTACK_HOME", &ashome);
    fs::write(lib_home.join("library.toml"), "version = 1\nserver = []\n").unwrap();

    let bare = tmp.path().join("remote.git");
    git(&["init", "-q", "--bare", &bare.to_string_lossy()]);
    let url = format!("file://{}", bare.display());

    // init the local library repo (no remote yet).
    lib::run(&sync(true, None, false), None).unwrap();

    // Commit a leaky server def with PLAIN git, bypassing the sync gate.
    let leaky = servers.join("leaky.toml");
    fs::write(
        &leaky,
        "type = \"http\"\nurl = \"https://x/mcp\"\n\
         [headers]\nAuthorization = \"Bearer sk-REALSECRET\"\n",
    )
    .unwrap();
    let libp = lib_home.to_str().unwrap();
    git(&["-C", libp, "add", "-A"]);
    git(&["-C", libp, "commit", "-qm", "leaky"]);

    // Edit the secret out; the working tree is now clean of it.
    fs::write(
        &leaky,
        "type = \"http\"\nurl = \"https://x/mcp\"\n\
         [headers]\nAuthorization = \"Bearer ${TOK}\"\n",
    )
    .unwrap();
    git(&["-C", libp, "add", "-A"]);
    git(&["-C", libp, "commit", "-qm", "redact"]);

    // Point at the remote; sync must refuse — the secret is in an earlier commit.
    let err = lib::run(&sync(false, Some(&url), false), None).unwrap_err();
    assert!(
        err.to_string().contains("outgoing history"),
        "a secret in outgoing commits must block the push: {err}"
    );

    // --allow-secrets overrides and completes the push.
    lib::run(
        &LibArgs {
            kind: LibKind::Sync(LibSyncArgs {
                init: false,
                remote: None,
                status: false,
                message: None,
                allow_secrets: true,
            }),
        },
        None,
    )
    .unwrap();

    std::env::remove_var("AGENTSTACK_HOME");
}

#[test]
fn sync_without_a_repo_errors_with_guidance() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let ashome = tmp.path().join("ashome");
    fs::create_dir_all(ashome.join("lib")).unwrap();
    std::env::set_var("AGENTSTACK_HOME", &ashome);

    let err = lib::run(&sync(false, None, false), None).unwrap_err();
    assert!(
        err.to_string().contains("--init"),
        "should point at --init: {err}"
    );

    std::env::remove_var("AGENTSTACK_HOME");
}
