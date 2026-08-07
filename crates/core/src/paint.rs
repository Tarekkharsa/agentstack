//! The one place that decides whether ANSI colour is written.
//!
//! # Why this module exists at all
//!
//! Every human-facing screen in the workspace reaches for `owo_colors`'
//! `OwoColorize` extension trait and writes `.green()`, `.dimmed()`, `.bold()`
//! inline — roughly 1250 call sites across ~52 files. Those methods are
//! unconditional: `"ok".green()` renders `\x1b[32mok\x1b[39m` whether stdout is
//! a terminal, a pipe, a file, or a CI log. Upstream's `set_override` does NOT
//! help — it is consulted only by `if_supports_color`, so using it would mean
//! rewriting all 1250 call sites. (TODO gap P8-G1.)
//!
//! So the gate goes one level lower, in the trait itself. This module declares
//! its own `OwoColorize` with the same method names and the same call shape,
//! and each method returns a [`Painted`] wrapper that asks [`enabled()`] at
//! *format* time. When colour is on, `Painted` delegates to the real
//! `owo_colors` wrapper, so the emitted bytes stay byte-identical to what this
//! workspace printed before the gate existed. When colour is off, it formats
//! the inner value and writes nothing else.
//!
//! # How the call sites reach it
//!
//! By importing [`OwoColorize`] from here instead of from upstream — the import
//! line says which trait it means, and that is the whole mechanism. The call
//! sites themselves are untouched: the method names and the call shape are
//! identical, so `.green()` reads the same and now asks the gate.
//!
//! Upstream `owo-colors` stays a dependency of THIS crate, because [`Painted`]
//! delegates to it. Reaching for it anywhere else is a bypass, and
//! `crates/cli/tests/color_is_gated.rs::every_screen_uses_the_gated_trait`
//! fails on one: a single stray `use owo_colors::OwoColorize;` would restore
//! the defect in that file alone, where no behavioural test would notice.
//!
//! # What this is not
//!
//! It is not an output sanitizer. Remote text that may itself carry escapes
//! (git stderr, registry descriptions, child process output) is still the job
//! of the `cli` crate's `text::sanitize_*`, which strips escapes regardless of
//! this gate. This module only governs the colour *we* add.

use std::fmt;
use std::sync::atomic::{AtomicU8, Ordering};

const UNDECIDED: u8 = 0;
const ON: u8 = 1;
const OFF: u8 = 2;

/// Process-wide, decided once. `Relaxed` is enough: the value is written once
/// at startup (or on first use) and only ever read afterwards; a racing pair of
/// first-use callers computes the same answer from the same environment, so
/// there is nothing for a stronger ordering to protect.
static STATE: AtomicU8 = AtomicU8::new(UNDECIDED);

/// The environment as this decision sees it. Split out from [`decide`] so the
/// rules can be tested as a table without mutating the real process
/// environment (which no parallel test runner tolerates).
#[derive(Debug, Clone, Copy, Default)]
pub struct Conditions<'a> {
    /// `NO_COLOR` — <https://no-color.org>. Any value that is not empty
    /// disables colour, whatever it says.
    pub no_color: Option<&'a str>,
    /// `CLICOLOR_FORCE` — the opt-in for "I am piping into a pager and I want
    /// the colour". Any value except empty and `0` forces colour on.
    pub clicolor_force: Option<&'a str>,
    /// `TERM`. The single value `dumb` means a terminal that cannot render
    /// escapes.
    pub term: Option<&'a str>,
    /// Whether stdout is a terminal.
    pub stdout_is_tty: bool,
}

/// The rules, in precedence order:
///
/// 1. `CLICOLOR_FORCE` (non-empty, not `0`) — colour on. It is an explicit
///    request from someone who knows the output is not a terminal, so it wins
///    over the two negative signals below. This matches `anstream`, `termcolor`
///    and clap.
/// 2. `NO_COLOR` (non-empty) — colour off.
/// 3. `TERM=dumb` — colour off.
/// 4. Otherwise: on if and only if stdout is a terminal.
///
/// The decision is taken from **stdout** and then applies to stderr too. One
/// process, one answer: `agentstack apply > log.txt` writes a narrative to the
/// file and its warnings to the terminal, and it would be strange for half of
/// one screen to be coloured. It also keeps the gate a single global rather
/// than a per-stream lookup at 1250 call sites that do not know which stream
/// they are bound for.
pub fn decide(c: Conditions<'_>) -> bool {
    if let Some(v) = c.clicolor_force {
        if !v.is_empty() && v != "0" {
            return true;
        }
    }
    if c.no_color.is_some_and(|v| !v.is_empty()) {
        return false;
    }
    if c.term == Some("dumb") {
        return false;
    }
    c.stdout_is_tty
}

fn from_environment() -> bool {
    use std::io::IsTerminal;
    let no_color = std::env::var("NO_COLOR").ok();
    let clicolor_force = std::env::var("CLICOLOR_FORCE").ok();
    let term = std::env::var("TERM").ok();
    decide(Conditions {
        no_color: no_color.as_deref(),
        clicolor_force: clicolor_force.as_deref(),
        term: term.as_deref(),
        stdout_is_tty: std::io::stdout().is_terminal(),
    })
}

/// Take the decision now, from the real environment. Called once at the top of
/// `main` so a reader can see where it happens and so the answer cannot depend
/// on which screen printed first.
///
/// Not required for correctness — [`enabled`] decides lazily on first use if
/// nobody called this, which is what keeps embedded entry points (the MCP
/// server, tests calling a command function directly) correct without each
/// having to remember.
pub fn configure() {
    let state = if from_environment() { ON } else { OFF };
    STATE.store(state, Ordering::Relaxed);
}

/// Whether ANSI colour may be written. Decides from the environment on first
/// call if [`configure`] has not run.
pub fn enabled() -> bool {
    match STATE.load(Ordering::Relaxed) {
        ON => true,
        OFF => false,
        _ => {
            let on = from_environment();
            STATE.store(if on { ON } else { OFF }, Ordering::Relaxed);
            on
        }
    }
}

/// Force the answer, ignoring the environment. For tests, and for a future
/// explicit `--color=always|never` should the CLI ever grow one.
pub fn set(on: bool) {
    STATE.store(if on { ON } else { OFF }, Ordering::Relaxed);
}

/// Which of the eight styles this workspace actually uses. Kept closed on
/// purpose: adding a variant is the moment to ask whether the screens need
/// another colour, and it keeps [`Painted`]'s delegation exhaustive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ink {
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    Bold,
    Dimmed,
}

/// A borrowed value plus the style it would like to wear. Borrowing (rather
/// than owning) mirrors upstream's wrappers exactly, so no call site's
/// temporaries or lifetimes change.
pub struct Painted<'a, T: ?Sized> {
    inner: &'a T,
    ink: Ink,
}

impl<T: ?Sized + fmt::Display> fmt::Display for Painted<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !enabled() {
            return self.inner.fmt(f);
        }
        // Delegate to the real crate rather than writing escape literals here:
        // the bytes then stay identical to what every screen emitted before the
        // gate, including how upstream passes the formatter (and therefore any
        // width or fill flag) through to the inner value.
        //
        // Fully qualified, and it has to be. Our own `OwoColorize` is in scope
        // in this module and its blanket impl also covers `&T`, so a plain
        // `self.inner.red()` resolves back to *this* trait and recurses until
        // the stack ends. Naming upstream's trait leaves nothing to infer.
        use owo_colors::OwoColorize as Upstream;
        let v = &self.inner;
        match self.ink {
            Ink::Red => Upstream::red(v).fmt(f),
            Ink::Green => Upstream::green(v).fmt(f),
            Ink::Yellow => Upstream::yellow(v).fmt(f),
            Ink::Blue => Upstream::blue(v).fmt(f),
            Ink::Magenta => Upstream::magenta(v).fmt(f),
            Ink::Cyan => Upstream::cyan(v).fmt(f),
            Ink::Bold => Upstream::bold(v).fmt(f),
            Ink::Dimmed => Upstream::dimmed(v).fmt(f),
        }
    }
}

/// The gated stand-in for `owo_colors::OwoColorize`, carrying only the methods
/// this workspace uses. A call site that reaches for a ninth style fails to
/// compile, which is the point: it lands here, in front of the gate, instead of
/// quietly becoming the one screen that ignores `NO_COLOR`.
///
/// The blanket impl covers `?Sized` receivers so `"literal".red()` keeps
/// working, and covers [`Painted`] itself so chains like `.green().bold()`
/// nest exactly as they did before.
pub trait OwoColorize {
    fn red(&self) -> Painted<'_, Self> {
        Painted {
            inner: self,
            ink: Ink::Red,
        }
    }
    fn green(&self) -> Painted<'_, Self> {
        Painted {
            inner: self,
            ink: Ink::Green,
        }
    }
    fn yellow(&self) -> Painted<'_, Self> {
        Painted {
            inner: self,
            ink: Ink::Yellow,
        }
    }
    fn blue(&self) -> Painted<'_, Self> {
        Painted {
            inner: self,
            ink: Ink::Blue,
        }
    }
    fn magenta(&self) -> Painted<'_, Self> {
        Painted {
            inner: self,
            ink: Ink::Magenta,
        }
    }
    fn cyan(&self) -> Painted<'_, Self> {
        Painted {
            inner: self,
            ink: Ink::Cyan,
        }
    }
    fn bold(&self) -> Painted<'_, Self> {
        Painted {
            inner: self,
            ink: Ink::Bold,
        }
    }
    fn dimmed(&self) -> Painted<'_, Self> {
        Painted {
            inner: self,
            ink: Ink::Dimmed,
        }
    }
}

impl<T: ?Sized + fmt::Display> OwoColorize for T {}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gate is a process-wide static, so the two tests that move it must
    /// not run beside each other.
    static GATE: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn precedence_table() {
        let tty = |stdout_is_tty| Conditions {
            stdout_is_tty,
            ..Conditions::default()
        };
        // Nothing set: a terminal is coloured, a pipe is not.
        assert!(decide(tty(true)));
        assert!(!decide(tty(false)));

        // NO_COLOR: any non-empty value, on a real terminal.
        for v in ["1", "0", "yes", "false", " "] {
            assert!(
                !decide(Conditions {
                    no_color: Some(v),
                    ..tty(true)
                }),
                "NO_COLOR={v:?} must disable colour"
            );
        }
        // Empty is "not set" — no-color.org is explicit about this.
        assert!(decide(Conditions {
            no_color: Some(""),
            ..tty(true)
        }));

        // CLICOLOR_FORCE wins over both negative signals.
        assert!(decide(Conditions {
            clicolor_force: Some("1"),
            no_color: Some("1"),
            term: Some("dumb"),
            ..tty(false)
        }));
        // ...but not when it is empty or explicitly 0.
        for v in ["", "0"] {
            assert!(
                !decide(Conditions {
                    clicolor_force: Some(v),
                    ..tty(false)
                }),
                "CLICOLOR_FORCE={v:?} must not force colour"
            );
        }

        // TERM=dumb, and only that exact value.
        assert!(!decide(Conditions {
            term: Some("dumb"),
            ..tty(true)
        }));
        assert!(decide(Conditions {
            term: Some("xterm-256color"),
            ..tty(true)
        }));
    }

    #[test]
    fn painted_is_byte_identical_when_on_and_bare_when_off() {
        let _g = GATE.lock().unwrap_or_else(|e| e.into_inner());

        set(true);
        // The exact bytes the screens emitted before this gate existed.
        assert_eq!("ok".green().to_string(), "\u{1b}[32mok\u{1b}[39m");
        assert_eq!("hi".bold().to_string(), "\u{1b}[1mhi\u{1b}[0m");
        assert_eq!("x".dimmed().to_string(), "\u{1b}[2mx\u{1b}[0m");
        // Chained styles still nest.
        assert_eq!(
            "go".green().bold().to_string(),
            "\u{1b}[1m\u{1b}[32mgo\u{1b}[39m\u{1b}[0m"
        );
        // A width flag reaches the inner value, as upstream passes it through.
        assert_eq!(format!("{:>4}", "ok".red()), "\u{1b}[31m  ok\u{1b}[39m");

        set(false);
        for s in [
            "ok".green().to_string(),
            "ok".red().to_string(),
            "ok".bold().to_string(),
            "ok".dimmed().to_string(),
            "ok".yellow().cyan().to_string(),
        ] {
            assert_eq!(s, "ok");
        }
        assert_eq!(format!("{:>4}", "ok".red()), "  ok");

        // Non-string receivers keep working: the blanket impl is over Display.
        set(true);
        assert_eq!(7.magenta().to_string(), "\u{1b}[35m7\u{1b}[39m");
        set(false);
        assert_eq!(7.magenta().to_string(), "7");
    }
}
