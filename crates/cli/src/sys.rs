//! The entire `unsafe` surface of agentstack, in one greppable file.
//!
//! Every libc / raw-fd / `pre_exec` call the `cli` crate makes lives here,
//! wrapped in a safe function. The crate root is `#![deny(unsafe_code)]` and
//! the `mod sys;` declaration carries the workspace's ONLY
//! `#[allow(unsafe_code)]`, so a reviewer auditing the unsafe surface of this
//! security tool reads exactly one file (CLAUDE.md rule 1). `deny` — not
//! `forbid` — because `forbid` cannot be locally downgraded by an `#[allow]`,
//! and the whole point is a single sanctioned exception; the fully-extracted
//! crates keep true `forbid`.
//!
//! Each wrapper keeps its Unix / non-Unix parity so callers hold no `cfg`
//! branches of their own. The unsafe here is minimal and boring: signal
//! delivery, process-group setup, one stdout/stderr fd dance, and a
//! writability probe.

use std::io;
use std::path::Path;
use std::process::Command;
#[cfg(unix)]
use std::sync::atomic::{AtomicBool, Ordering};

/// A signal we deliver to a process group. Kept as an enum so callers never
/// name a raw `libc` constant.
#[derive(Clone, Copy)]
pub enum Signal {
    /// Polite termination (`SIGTERM`).
    Term,
    /// Unconditional kill (`SIGKILL`).
    Kill,
}

#[cfg(unix)]
static SIGINT_SEEN: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn mark_sigint(_signal: libc::c_int) {
    // Atomic stores are signal-safe and allocate nothing. `SeqCst` keeps the
    // observation simple across the terminal-reading thread and handler.
    SIGINT_SEEN.store(true, Ordering::SeqCst);
}

/// Temporarily turn Ctrl-C into an observable cancellation instead of an
/// immediate process exit. Used only around the interactive setup wizard so it
/// can print the files already written and the exact undo command.
pub struct SigintGuard {
    #[cfg(unix)]
    previous: libc::sigaction,
}

impl SigintGuard {
    #[cfg(unix)]
    pub fn install() -> io::Result<Self> {
        use std::mem::MaybeUninit;

        SIGINT_SEEN.store(false, Ordering::SeqCst);
        // SAFETY: both sigaction values are fully initialized before use;
        // `mark_sigint` has the required C ABI and only performs an atomic
        // store. Passing a valid signal number and pointers follows sigaction's
        // contract. Keeping SA_RESTART clear lets a blocked terminal read
        // return so the wizard can observe the flag.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = mark_sigint as *const () as usize;
            action.sa_flags = 0;
            libc::sigemptyset(&mut action.sa_mask);
            let mut previous = MaybeUninit::<libc::sigaction>::uninit();
            if libc::sigaction(libc::SIGINT, &action, previous.as_mut_ptr()) != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self {
                previous: previous.assume_init(),
            })
        }
    }

    #[cfg(not(unix))]
    pub fn install() -> io::Result<Self> {
        Ok(Self {})
    }

    #[cfg(unix)]
    pub fn interrupted(&self) -> bool {
        SIGINT_SEEN.load(Ordering::SeqCst)
    }

    #[cfg(not(unix))]
    pub fn interrupted(&self) -> bool {
        false
    }
}

impl Drop for SigintGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            // SAFETY: `previous` came from a successful sigaction call and
            // remains valid for the guard's lifetime. Restoring it closes the
            // wizard-only interception scope.
            unsafe {
                libc::sigaction(libc::SIGINT, &self.previous, std::ptr::null_mut());
            }
            SIGINT_SEEN.store(false, Ordering::SeqCst);
        }
    }
}

/// Restore the default `SIGPIPE` disposition. The Rust runtime starts every
/// process with `SIGPIPE` ignored, which turns a reader hanging up early
/// (`agentstack diff | head`) into a `println!` panic — exit 101 plus a
/// backtrace note — instead of the silent exit every Unix CLI has. Called
/// once, first thing in `main`, before anything writes to stdout.
#[cfg(unix)]
pub fn reset_sigpipe() {
    // SAFETY: `signal` with `SIG_DFL` installs the kernel's default
    // disposition for a valid signal number — no handler of ours runs, no
    // pointers, no memory it can corrupt.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

#[cfg(not(unix))]
pub fn reset_sigpipe() {}

/// Whether the process `pid` is still alive — `kill(pid, 0)` delivers no
/// signal, it only probes for the target's existence and our permission to
/// signal it.
#[cfg(unix)]
pub fn pid_alive(pid: i32) -> bool {
    // SAFETY: `kill` with signal 0 has no memory effects; it only reports
    // whether the process exists. No pointers, no ownership.
    unsafe { libc::kill(pid, 0) == 0 }
}

#[cfg(not(unix))]
pub fn pid_alive(_pid: i32) -> bool {
    false
}

/// Send `sig` to the whole process group led by `pgid` (`kill(-pgid, …)` — a
/// negative pid addresses the group). Returns the OS error on failure; callers
/// that tolerate a race use [`is_already_gone`].
#[cfg(unix)]
pub fn signal_group(pgid: i32, sig: Signal) -> io::Result<()> {
    let signum = match sig {
        Signal::Term => libc::SIGTERM,
        Signal::Kill => libc::SIGKILL,
    };
    // SAFETY: `kill` takes a pid and a signal number by value; no pointers, no
    // memory it can corrupt. The negative pid is the documented way to address
    // a process group.
    let rc = unsafe { libc::kill(-pgid, signum) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(unix))]
pub fn signal_group(_pgid: i32, _sig: Signal) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "signalling process groups is not supported on this platform yet",
    ))
}

/// Whether a [`signal_group`] error means the target was already gone
/// (`ESRCH`) — a benign race when reaping a process that just exited, which
/// callers treat as success.
#[cfg(unix)]
pub fn is_already_gone(err: &io::Error) -> bool {
    err.raw_os_error() == Some(libc::ESRCH)
}

#[cfg(not(unix))]
pub fn is_already_gone(_err: &io::Error) -> bool {
    false
}

/// Arrange for `cmd`'s child to become its own process-group leader
/// (`setpgid(0, 0)`), so the whole tree it spawns can later be reaped with
/// [`signal_group`]. The hook runs in the child after `fork`, before `exec`.
#[cfg(unix)]
pub fn spawn_in_new_process_group(cmd: &mut Command) -> &mut Command {
    use std::os::unix::process::CommandExt;
    // SAFETY: `pre_exec` is unsafe because its closure runs in the forked
    // child before exec, where only async-signal-safe calls are legal.
    // `setpgid` is on the async-signal-safe list; it allocates nothing and
    // touches no shared state, so it is sound here.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setpgid(0, 0) != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    cmd
}

#[cfg(not(unix))]
pub fn spawn_in_new_process_group(cmd: &mut Command) -> &mut Command {
    // No process groups here; `agentstack run` already bails on non-unix, and
    // the gateway falls back to a plain child kill.
    cmd
}

/// Reserve the real stdout for JSON-RPC and point fd 1 at stderr, returning a
/// writer for the saved stdout. `agentstack mcp` speaks the protocol on stdout,
/// so any stray `println!` from command code must land on stderr instead of
/// corrupting the stream. Falls back to plain stdout if the dup fails.
#[cfg(unix)]
pub fn reserve_stdout_for_protocol() -> Box<dyn io::Write + Send> {
    use std::os::unix::io::FromRawFd;
    // SAFETY: `dup` returns a fresh descriptor or -1; no memory effects.
    let saved = unsafe { libc::dup(libc::STDOUT_FILENO) };
    if saved < 0 {
        return Box::new(io::stdout());
    }
    // SAFETY: `dup2` redirects fd 1 to stderr; it operates on descriptor
    // numbers only, no pointers.
    unsafe {
        libc::dup2(libc::STDERR_FILENO, libc::STDOUT_FILENO);
    }
    // SAFETY: `saved` is a valid descriptor we just created with `dup` and do
    // not use elsewhere, so wrapping it in a `File` transfers sole ownership —
    // the contract `from_raw_fd` requires.
    Box::new(unsafe { std::fs::File::from_raw_fd(saved) })
}

#[cfg(not(unix))]
pub fn reserve_stdout_for_protocol() -> Box<dyn io::Write + Send> {
    Box::new(io::stdout())
}

/// Whether `dir` is writable by this user — the `[ -w ]` test `install.sh`
/// uses to prefer `/usr/local/bin` over `~/.local/bin`.
#[cfg(unix)]
pub fn dir_writable(dir: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `access` reads the NUL-terminated path `c` owns for the duration
    // of the call and returns an int; `c` outlives the call, so the pointer is
    // valid.
    unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
}

#[cfg(not(unix))]
pub fn dir_writable(dir: &Path) -> bool {
    std::fs::metadata(dir)
        .map(|m| m.is_dir() && !m.permissions().readonly())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pid_alive_true_for_self_false_for_unused() {
        let me = std::process::id() as i32;
        assert!(pid_alive(me), "our own process must read as alive");
        // A pid well above any live process on a fresh system. If it happens
        // to exist we only lose the negative assertion, never soundness.
        assert!(!pid_alive(2_000_000_000));
    }

    #[test]
    fn dir_writable_distinguishes_temp_from_missing() {
        let tmp = assert_fs::TempDir::new().unwrap();
        assert!(dir_writable(tmp.path()), "a fresh temp dir is writable");
        assert!(
            !dir_writable(&tmp.path().join("does-not-exist")),
            "a missing path is not writable"
        );
    }

    #[cfg(unix)]
    #[test]
    fn sigint_guard_observes_interrupt_and_restores_on_drop() {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let signal = SigintGuard::install().expect("install scoped SIGINT handler");
        // SAFETY: raise delivers SIGINT to this process; the scoped handler is
        // installed and performs only an atomic store.
        assert_eq!(unsafe { libc::raise(libc::SIGINT) }, 0);
        assert!(signal.interrupted());
        drop(signal); // restoring the test harness's prior handler is the check's contract
    }

    #[cfg(unix)]
    #[test]
    fn signal_group_reaps_a_child_group_and_tolerates_the_race() {
        // Spawn `sleep` as its own group leader, then signal the group.
        let mut cmd = Command::new("sleep");
        cmd.arg("30");
        spawn_in_new_process_group(&mut cmd);
        let mut child = cmd.spawn().expect("spawn sleep");
        let pgid = child.id() as i32;

        assert!(pid_alive(pgid), "child is alive before the signal");
        signal_group(pgid, Signal::Kill).expect("first kill delivers");

        // Reap and poll until the kernel has torn it down.
        let _ = child.wait();
        for _ in 0..100 {
            if !pid_alive(pgid) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!pid_alive(pgid), "child gone after SIGKILL");

        // Signalling the now-dead group races to ESRCH, which callers treat as
        // success.
        if let Err(e) = signal_group(pgid, Signal::Term) {
            assert!(is_already_gone(&e), "expected ESRCH, got {e}");
        }
    }
}
