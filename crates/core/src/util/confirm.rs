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

/// Ask a question with several named answers, returning `None` when the human
/// did not pick one.
///
/// `None` is not a fourth answer — it means *no decision was made*, and every
/// caller must respond by leaving its existing behaviour exactly as it was.
/// That is what keeps a new multi-way prompt from changing what CI, pipes, and
/// scripts do: not interactive, no answer, no change. Bare Enter, EOF, an
/// unrecognized word, and a read error all land here too, because a consent
/// question must never resolve itself by guessing.
///
/// `choices` are `(key, label)`; matching is case-insensitive on the key or the
/// full label.
pub fn choose(question: &str, choices: &[(&str, &str)]) -> std::io::Result<Option<String>> {
    if !is_interactive() {
        return Ok(None);
    }
    let stdin = std::io::stdin();
    prompt_choice(&mut stdin.lock(), &mut std::io::stdout(), question, choices)
}

/// The prompt-and-parse core for [`choose`], split from terminal I/O so tests
/// drive it with in-memory buffers — same shape and same reasoning as
/// [`prompt_yes_no`].
fn prompt_choice<R: std::io::BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    question: &str,
    choices: &[(&str, &str)],
) -> std::io::Result<Option<String>> {
    let rendered: Vec<String> = choices
        .iter()
        .map(|(key, label)| format!("{key}) {label}"))
        .collect();
    write!(writer, "{question}\n  {}\n> ", rendered.join("   "))?;
    writer.flush()?;
    let mut line = String::new();
    let read = match reader.read_line(&mut line) {
        Ok(read) => read,
        Err(_) => return Ok(None),
    };
    if read == 0 {
        return Ok(None); // EOF / closed stdin
    }
    let answer = line.trim().to_ascii_lowercase();
    if answer.is_empty() {
        return Ok(None);
    }
    Ok(choices
        .iter()
        .find(|(key, label)| {
            key.eq_ignore_ascii_case(&answer) || label.eq_ignore_ascii_case(&answer)
        })
        .map(|(key, _)| (*key).to_string()))
}

#[cfg(test)]
mod choice_tests {
    use super::*;

    const CHOICES: &[(&str, &str)] = &[("a", "accept"), ("k", "keep pinned"), ("b", "block")];

    fn ask(input: &str) -> Option<String> {
        let mut out = Vec::new();
        prompt_choice(&mut input.as_bytes(), &mut out, "What now?", CHOICES).unwrap()
    }

    #[test]
    fn a_key_or_its_full_label_selects() {
        assert_eq!(ask("a\n").as_deref(), Some("a"));
        assert_eq!(ask("K\n").as_deref(), Some("k"));
        assert_eq!(ask("block\n").as_deref(), Some("b"));
        assert_eq!(ask("  Keep Pinned  \n").as_deref(), Some("k"));
    }

    // The safety property: anything that is not an explicit choice leaves the
    // caller's behaviour unchanged. A consent question must never resolve
    // itself by guessing what the silence meant.
    #[test]
    fn silence_ambiguity_and_errors_all_decide_nothing() {
        assert_eq!(ask("\n"), None, "bare Enter must not pick an answer");
        assert_eq!(ask(""), None, "EOF must not pick an answer");
        assert_eq!(ask("maybe\n"), None, "an unrecognized word must not match");
        assert_eq!(ask("ac\n"), None, "a prefix must not match a longer label");
    }

    #[test]
    fn every_answer_is_shown_with_its_key() {
        let mut out = Vec::new();
        prompt_choice(&mut "a\n".as_bytes(), &mut out, "What now?", CHOICES).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("a) accept"), "{text}");
        assert!(text.contains("k) keep pinned"), "{text}");
        assert!(text.contains("b) block"), "{text}");
    }

    #[test]
    fn under_cargo_test_choose_never_prompts() {
        // Same net as `confirm`: no terminal, so no blocking and no decision.
        assert!(!is_interactive());
        assert_eq!(choose("What now?", CHOICES).unwrap(), None);
    }
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
