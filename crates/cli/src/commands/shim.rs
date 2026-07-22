//! `agentstack shim` — per-run identity for sessions started by external
//! supervisors (t3code, IDEs) that launch agent CLIs themselves.
//!
//! Those supervisors never go through `agentstack run`, so their sessions
//! attribute to the machine-global audit only. The bridge is a tiny
//! exec-through wrapper: `shim make <cli>` writes
//! `~/.agentstack/shims/<cli>`, the user points the supervisor's binary-path
//! setting at it, and every launch then runs `shim exec`, which mints a run
//! id, opens the run's `events.jsonl`, exports [`calllog::RUN_ID_ENV`], and
//! replaces itself with the real binary (`exec`, so signals, exit codes, and
//! stdio behave exactly as if the supervisor had spawned the CLI directly).
//! One shim invocation = one supervisor session = one run.
//!
//! The wrapper must stay exec-compatible with SDK spawns (no shell
//! required by the caller): it is an executable `#!/bin/sh` script whose
//! only job is `exec agentstack shim exec <real> -- "$@"`.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};

use crate::calllog;
use crate::cli::{ShimCmd, ShimExecArgs, ShimMakeArgs};
use crate::util::paths;

pub fn run(cmd: &ShimCmd) -> Result<()> {
    match cmd {
        ShimCmd::Make(args) => make(args),
        ShimCmd::Exec(args) => exec(args),
    }
}

fn shims_dir() -> PathBuf {
    paths::agentstack_home().join("shims")
}

fn make(args: &ShimMakeArgs) -> Result<()> {
    // The shim name becomes a file name — same safety rule as run ids.
    if !args
        .cli
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
        || args.cli.is_empty()
    {
        bail!("shim name must be alphanumeric/dash/underscore/dot");
    }

    let dir = shims_dir();
    let real = match &args.binary {
        Some(p) => {
            let p = paths::expand_tilde(&p.display().to_string());
            if !p.is_file() {
                bail!("--binary {} does not exist", p.display());
            }
            p
        }
        None => resolve_on_path(&args.cli, &dir)
            .ok_or_else(|| anyhow!("`{}` not found on PATH — pass --binary", args.cli))?,
    };
    let me = std::env::current_exe().context("resolving the agentstack executable")?;

    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join(&args.cli);
    std::fs::write(&path, shim_script(&me, &real))
        .with_context(|| format!("writing {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    }

    println!("Shim written: {}", path.display());
    println!("  wraps: {}", real.display());
    println!();
    println!("Point the supervisor's binary path at the shim — e.g. in t3code:");
    println!(
        "  Settings → Providers → <instance> → Binary path → {}",
        path.display()
    );
    println!();
    println!(
        "Each session it starts then records per-run evidence; inspect with \
         `agentstack report runs` / `agentstack report run <id>`."
    );
    Ok(())
}

/// First `name` on PATH that is a file outside `shims_dir` (so a shim on
/// PATH can never wrap itself).
fn resolve_on_path(name: &str, shims: &Path) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    std::env::split_paths(&path_var)
        .map(|d| d.join(name))
        .find(|c| c.is_file() && !c.starts_with(shims))
}

fn shim_script(agentstack: &Path, real: &Path) -> String {
    // Quoted paths; the trailing `--` stops clap so the wrapped CLI's own
    // flags pass through verbatim.
    format!(
        "#!/bin/sh\n\
         # agentstack shim — mints a per-run identity, then becomes the real CLI.\n\
         # Regenerate with: agentstack shim make (do not edit)\n\
         exec \"{}\" shim exec \"{}\" -- \"$@\"\n",
        agentstack.display(),
        real.display()
    )
}

fn exec(args: &ShimExecArgs) -> Result<()> {
    let (run_id, mut cmd) = prepare(&args.binary, &args.args);

    // From here the process IS the wrapped CLI — same pid, signals, exit
    // code, and stdio as a direct spawn. Only reached on failure.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = cmd.exec();
        Err(anyhow!(
            "shim {run_id}: failed to exec {}: {err}",
            args.binary.display()
        ))
    }
    #[cfg(not(unix))]
    {
        let status = cmd
            .status()
            .with_context(|| format!("shim {run_id}: spawning {}", args.binary.display()))?;
        std::process::exit(status.code().unwrap_or(1));
    }
}

/// The pre-exec half, factored for testing: mint the id, open the run's
/// event log (best-effort, like every recorder path), and build the command
/// with `AGENTSTACK_RUN_ID` set for the child.
fn prepare(binary: &Path, args: &[std::ffi::OsString]) -> (String, Command) {
    let run_id = crate::runs::gen_id();
    if let Some(log) = calllog::RunLog::create(&run_id) {
        log.append(&calllog::RunEvent::HostStarted {
            ts: calllog::now_epoch(),
            harness: binary
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| binary.display().to_string()),
            posture: "host".to_string(),
        });
    }
    let mut cmd = Command::new(binary);
    cmd.args(args).env(calllog::RUN_ID_ENV, &run_id);
    (run_id, cmd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_mints_id_creates_run_log_and_sets_env() {
        let home = assert_fs::TempDir::new().unwrap();
        // Serialized by nextest's per-binary process model; the env var is
        // how every recorder test relocates the home.
        std::env::set_var("AGENTSTACK_HOME", home.path());

        let (run_id, cmd) = prepare(Path::new("/bin/echo"), &["hi".into()]);
        assert!(run_id.starts_with("r-"), "run-id format: {run_id}");
        let events = home.path().join("runs").join(&run_id).join("events.jsonl");
        assert!(events.exists(), "per-run event log created");
        let body = std::fs::read_to_string(events).unwrap();
        assert!(body.contains("host_started") || body.contains("HostStarted"));
        let env: Vec<_> = cmd.get_envs().collect();
        assert!(
            env.iter().any(|(k, v)| *k == calllog::RUN_ID_ENV
                && v.map(|v| v.to_string_lossy() == run_id).unwrap_or(false)),
            "child env carries the run id"
        );

        std::env::remove_var("AGENTSTACK_HOME");
    }

    #[test]
    fn script_is_exec_through_and_names_both_binaries() {
        let s = shim_script(
            Path::new("/opt/agentstack"),
            Path::new("/usr/local/bin/claude"),
        );
        assert!(s.starts_with("#!/bin/sh\n"));
        assert!(
            s.contains("exec \"/opt/agentstack\" shim exec \"/usr/local/bin/claude\" -- \"$@\"")
        );
    }
}
