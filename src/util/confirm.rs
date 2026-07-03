//! Interactive confirmation for write-class commands.
//!
//! The contract: a mutating command run *without* `--write` shows its dry-run
//! diff and then, only when attached to a real terminal, asks before writing.
//! Non-interactive callers (CI, pipes, redirects) never see a prompt and never
//! block — they stay in dry-run. `--write` skips the prompt entirely, so it
//! remains the scripting / CI escape hatch.

use std::io::{IsTerminal, Write};

/// True only when both stdin and stdout are attached to a terminal. A prompt
/// needs a human who can see it (stdout) *and* answer it (stdin); if either end
/// is a pipe, redirect, or CI runner we must not block waiting for input.
pub fn is_interactive() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

/// Ask `question` and return whether the user assented. Returns `Ok(false)`
/// *without prompting* when not interactive, so a caller can gate a write on
/// `confirm(..)?` unconditionally and trust that CI/pipes stay in dry-run.
pub fn confirm(question: &str) -> std::io::Result<bool> {
    if !is_interactive() {
        return Ok(false);
    }
    let stdin = std::io::stdin();
    prompt_yes_no(&mut stdin.lock(), &mut std::io::stdout(), question)
}

/// The prompt-and-parse core, split out from terminal I/O so tests can drive it
/// with in-memory buffers. Anything but an explicit yes is a no; empty input,
/// EOF, and read errors all default to no (the safe choice for a write).
fn prompt_yes_no<R: std::io::BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    question: &str,
) -> std::io::Result<bool> {
    write!(writer, "{question} [y/N] ")?;
    writer.flush()?;
    let mut line = String::new();
    let read = match reader.read_line(&mut line) {
        Ok(read) => read,
        Err(_) => return Ok(false),
    };
    if read == 0 {
        return Ok(false); // EOF (e.g. closed stdin)
    }
    Ok(matches!(line.trim(), "y" | "Y" | "yes" | "Yes" | "YES"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(input: &str) -> bool {
        let mut out = Vec::new();
        prompt_yes_no(&mut input.as_bytes(), &mut out, "Apply these changes?").unwrap()
    }

    #[test]
    fn only_explicit_yes_confirms() {
        assert!(ask("y\n"));
        assert!(ask("Y\n"));
        assert!(ask("yes\n"));
        assert!(ask("  yes  \n")); // surrounding whitespace is trimmed
    }

    #[test]
    fn anything_else_declines() {
        assert!(!ask("n\n"));
        assert!(!ask("no\n"));
        assert!(!ask("\n")); // bare Enter defaults to No
        assert!(!ask("")); // EOF / closed stdin defaults to No
        assert!(!ask("yep\n")); // not an exact yes
    }

    #[test]
    fn the_prompt_is_written_with_a_no_default() {
        let mut out = Vec::new();
        prompt_yes_no(&mut "n\n".as_bytes(), &mut out, "Apply these changes?").unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "Apply these changes? [y/N] "
        );
    }

    struct ErrorReader;

    impl std::io::Read for ErrorReader {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("closed"))
        }
    }

    impl std::io::BufRead for ErrorReader {
        fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
            Err(std::io::Error::other("closed"))
        }

        fn consume(&mut self, _amt: usize) {}
    }

    #[test]
    fn read_errors_decline() {
        let mut out = Vec::new();
        assert!(
            !prompt_yes_no(&mut ErrorReader, &mut out, "Apply these changes?").unwrap(),
            "stdin read errors must default to No"
        );
    }

    #[test]
    fn under_cargo_test_stdin_is_not_a_terminal() {
        // The whole non-TTY safety net rests on this: the test runner's stdin is
        // not a terminal, so `confirm` must return without prompting or blocking.
        assert!(!is_interactive());
        assert!(!confirm("Apply these changes?").unwrap());
    }
}
