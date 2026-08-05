#!/usr/bin/env python3
"""Enforcement/claim pairing gate (invariant 8: "claims match enforcement").

THE RULE
--------
A pull request that changes ENFORCEMENT BEHAVIOUR in

    crates/trust/   crates/policy/   crates/egress/

must, in the same pull request, either

  (a) change `docs/ENFORCEMENT.md` — the file where the enforcement claims
      live — or
  (b) carry an explicit written waiver:

          ENFORCEMENT-WAIVER: <one-line reason>

      in the pull-request body or in a commit-message trailer on the branch.

Nothing else satisfies it. The point is that the two halves of invariant 8 —
what the code enforces and what the docs claim it enforces — cannot drift
apart without somebody making a decision on the record.

WHAT COUNTS AS "ENFORCEMENT BEHAVIOUR"
--------------------------------------
Not every byte in those crates is a claim. The gate deliberately ignores:

  * test code — anything under a `tests/` or `benches/` directory of those
    crates, and any file named `tests.rs`;
  * test code written inline — a changed line that falls inside an item
    carrying a `#[cfg(test)]` attribute (the `mod tests { … }` convention this
    repository uses almost everywhere, and also a `#[cfg(test)]` on a single
    `fn`, `use`, or other item);
  * comment-only and blank-line changes in the remaining files — `//`,
    `/* … */` and `///` in Rust, `#` in `.toml`.

Rationale: added tests and rewritten comments cannot make the shipped
enforcement disagree with ENFORCEMENT.md — they are evidence and prose, not
behaviour. A gate that fires on "I added a witness test" gets disabled within
a week, and a disabled gate defends nothing. The cost of the exemption is
that a behaviour change smuggled in as a `#[cfg(test)]` item escapes; that is
an acceptable trade, because such a change cannot affect a release build.

HOW THE `#[cfg(test)]` EXEMPTION IS COMPUTED
--------------------------------------------
There is no Rust parser here, and there will not be one. The gate works from
a `-U0` unified diff plus the two file images it names:

  1. Hunk headers give every changed line a line number — on the pre-image
     for `-` lines, on the post-image for `+` lines.
  2. Each image is scanned for lines matching `#[cfg(test)]`. From the item
     that follows, a brace count runs to the matching `}` (or, for an item
     with no block, to the terminating `;`). That span is a test region.
  3. A file counts as an enforcement change only if at least one of its
     substantive changed lines falls OUTSIDE every test region of its own
     side's image.

Brace counting is done over a masked copy of the source in which string
literals, char literals, and comments have been blanked out, so a `"{"` or a
`// }` inside a test module cannot mis-close the region. The masker
understands line and block comments, ordinary and raw strings (`r#"…"#`,
including the `b` prefix), and char literals as distinct from lifetimes.

FAIL TOWARD FIRING, NEVER TOWARD SILENCE
-----------------------------------------
Every uncertainty in the analysis above resolves to "this line is NOT test
code", so the gate fires. If a `#[cfg(test)]` region never closes, if the
masker loses the plot, if a file image cannot be read out of git, if a hunk
header will not parse — no region is produced and the change counts. A false
positive costs one waiver line in a pull-request body. A false negative costs
the invariant. Those prices are not close.

NOT HANDLED (these fire; waive them if they are genuinely test-only)
---------------------------------------------------------------------
  * `#[cfg(all(test, …))]`, `#[cfg(any(test, …))]`, `#[cfg_attr(test, …)]`
    and other spellings that are not the literal token `#[cfg(test)]`.
  * `#[cfg(test)] mod helpers;` — the declaration line is exempt, but the
    out-of-line file it names is not, unless its own path is already exempt.
  * A `#[cfg(test)]` region whose extent changes across the diff such that
    production lines move in or out of it. Both sides are scanned
    independently, which catches the common cases, but it is a heuristic.

SCOPE
-----
`pull_request` events only. A `push` has no dependable base commit to diff
against (force-pushes, squash merges, and the initial push of a branch all
break `before`), and the waiver lives in the pull-request body, which does
not exist on a push. ci.yml gates the job on the event name.

USAGE
-----
    check-enforcement-pairing.py --base <sha> --head <sha> [--waiver-file F]

Exit 0 = pass, 1 = fail (with instructions), 2 = usage error.
`--self-test` runs the classifier regression cases and exits.
"""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys

WATCHED_CRATES = ("crates/trust/", "crates/policy/", "crates/egress/")
CLAIMS_DOC = "docs/ENFORCEMENT.md"
WAIVER_MARKER = "ENFORCEMENT-WAIVER:"
# The marker must be followed by a non-empty reason: a bare marker is not a
# decision, it is a way to shut the gate up.
WAIVER_RE = re.compile(r"^\s*" + re.escape(WAIVER_MARKER) + r"\s*(\S.*)$", re.MULTILINE)


def is_watched(path: str) -> bool:
    return any(path.startswith(c) for c in WATCHED_CRATES)


def is_test_path(path: str) -> bool:
    """Test/bench code in a watched crate is exempt (see module docstring)."""
    parts = path.split("/")
    if "tests" in parts or "benches" in parts:
        return True
    return parts[-1] == "tests.rs"


def is_ignorable_line(line: str, path: str = "") -> bool:
    """True for a changed line that cannot alter behaviour.

    `line` is a unified-diff body line WITHOUT its leading +/-. `path` selects
    the comment syntax; without it, only Rust comments are recognised.
    """
    s = line.strip()
    if not s:
        return True
    # `#` opens a comment in TOML but is an ATTRIBUTE in Rust (`#[derive]`),
    # so this is keyed on the file extension and must stay that way.
    if path.endswith(".toml") and s.startswith("#"):
        return True
    # A bare `version = "x.y.z"` line is the crate's own version field in
    # Cargo.toml. Release version bumps touch every crate and change no
    # enforcement claim; firing on each release is exactly the cry-wolf that
    # gets a gate disabled. Dependency lines (`name = { version = ... }`) do
    # NOT match this and still count.
    if re.fullmatch(r'version\s*=\s*"[^"]*"', s):
        return True
    # Line comments, doc comments, and the interior/edges of block comments.
    # `*/` and a leading `*` are treated as comment continuation lines; a line
    # that merely CONTAINS `//` after code is not ignorable.
    return (
        s.startswith("//")
        or s.startswith("/*")
        or s.startswith("*")
        or s == "*/"
    )


# --- inline `#[cfg(test)]` detection ---------------------------------------
#
# Everything below exists to answer one question: is this changed line inside a
# `#[cfg(test)]` item? Every failure mode answers "no", which makes the gate
# fire. See "FAIL TOWARD FIRING" in the module docstring.

RAW_STR_RE = re.compile(r'b?r(#*)"')
LIFETIME_RE = re.compile(r"'[A-Za-z_][A-Za-z0-9_]*")
CFG_TEST_RE = re.compile(r"^\s*#\[cfg\(test\)\]")
IDENT_CHAR_RE = re.compile(r"[A-Za-z0-9_]")


def mask_literals_and_comments(src: str) -> str:
    """Blank out comments and string/char literals, preserving line structure.

    Returns a string of the same length as `src` with every character that
    lives inside a comment or a literal replaced by a space (newlines are
    kept, so line numbers and columns still line up). Brace counting then sees
    only real Rust punctuation, and a `"{"` in a test fixture cannot close a
    module early.
    """
    out = list(src)
    n = len(src)
    i = 0

    def blank(a: int, b: int) -> None:
        for k in range(a, min(b, n)):
            if out[k] != "\n":
                out[k] = " "

    while i < n:
        c = src[i]
        # Comments.
        if src.startswith("//", i):
            j = src.find("\n", i)
            j = n if j < 0 else j
            blank(i, j)
            i = j
            continue
        if src.startswith("/*", i):
            # Rust block comments nest.
            depth, j = 1, i + 2
            while j < n and depth:
                if src.startswith("/*", j):
                    depth += 1
                    j += 2
                elif src.startswith("*/", j):
                    depth -= 1
                    j += 2
                else:
                    j += 1
            blank(i, j)
            i = j
            continue
        # Raw strings: r"…", r#"…"#, br#"…"#. The preceding character must not
        # be an identifier character, or `for#` style false matches creep in.
        m = RAW_STR_RE.match(src, i)
        if m and (i == 0 or not IDENT_CHAR_RE.match(src[i - 1])):
            close = '"' + m.group(1)
            j = src.find(close, m.end())
            j = n if j < 0 else j + len(close)
            blank(i, j)
            i = j
            continue
        # Ordinary strings, with escapes. A `b` prefix is just an identifier
        # character before the quote, so it needs no special case.
        if c == '"':
            j = i + 1
            while j < n:
                if src[j] == "\\":
                    j += 2
                    continue
                if src[j] == '"':
                    j += 1
                    break
                j += 1
            blank(i, j)
            i = j
            continue
        # `'` is either a lifetime (`'a`, `'static`) or a char literal (`'{'`).
        # A lifetime is an identifier NOT followed by a closing quote.
        if c == "'":
            lm = LIFETIME_RE.match(src, i)
            if lm and not src.startswith("'", lm.end()):
                i = lm.end()
                continue
            j = i + 1
            while j < n:
                if src[j] == "\\":
                    j += 2
                    continue
                if src[j] == "'":
                    j += 1
                    break
                j += 1
            blank(i, j)
            i = j
            continue
        i += 1
    return "".join(out)


def cfg_test_ranges(src: str | None) -> list[tuple[int, int]]:
    """1-based inclusive line spans covered by a `#[cfg(test)]` item.

    A missing or unreadable source yields no ranges, so nothing is exempt.
    """
    if not src:
        return []
    raw = src.splitlines()
    masked = mask_literals_and_comments(src).splitlines()
    # `splitlines` on the masked copy must agree line-for-line with the raw
    # copy; if it somehow does not, exempt nothing.
    if len(masked) != len(raw):
        return []

    ranges: list[tuple[int, int]] = []
    i = 0
    while i < len(raw):
        if not CFG_TEST_RE.match(raw[i]):
            i += 1
            continue
        # Walk forward over the attributed item. `depth` counts braces; an
        # item with no block (`use x;`, `const X: u8 = 1;`) ends at its `;`.
        depth, seen_brace, end = 0, False, None
        j = i
        while j < len(masked):
            for ch in masked[j]:
                if ch == "{":
                    depth += 1
                    seen_brace = True
                elif ch == "}":
                    depth -= 1
                    if seen_brace and depth <= 0:
                        end = j
                        break
                elif ch == ";" and depth == 0 and not seen_brace:
                    end = j
                    break
            if end is not None:
                break
            j += 1
        if end is None:
            # Unterminated: do not exempt anything, and do not swallow the
            # rest of the file looking for a close.
            i += 1
            continue
        ranges.append((i + 1, end + 1))
        i = end + 1
    return ranges


def in_ranges(ranges: list[tuple[int, int]], line_no: int) -> bool:
    return any(lo <= line_no <= hi for lo, hi in ranges)


def substantive_files(
    diff_text: str,
    old_source=None,
    new_source=None,
) -> list[str]:
    """Watched, non-test files with >=1 changed line that could alter behaviour.

    Parses `git diff -U0` output: `diff --git a/X b/X` headers, `@@` hunk
    headers for line numbers, then `+`/`-` body lines.

    `old_source`/`new_source` are optional callables taking a repository path
    and returning that file's text on the pre-image / post-image side (or
    None). They are what makes the inline `#[cfg(test)]` exemption possible;
    with neither supplied, no line is inside a test region and the classifier
    degrades to the conservative directory/comment rule.
    """
    # path -> {"old": [line numbers], "new": [line numbers], "was": old path},
    # in diff order.
    changed: dict[str, dict] = {}

    def entry(p: str) -> dict:
        return changed.setdefault(p, {"old": [], "new": [], "was": old_path or p})

    old_path: str | None = None
    path: str | None = None
    old_ln = new_ln = 0

    for line in diff_text.splitlines():
        if line.startswith("diff --git "):
            path, old_path = None, None
            m = re.match(r"diff --git a/(.*) b/(.*)$", line)
            if m:
                cand = m.group(2)
                if is_watched(cand) and not is_test_path(cand):
                    path, old_path = cand, m.group(1)
            continue
        if path is None:
            continue
        if line.startswith(("+++", "---")):
            continue
        if line.startswith("@@"):
            m = re.match(r"@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@", line)
            if not m:
                # An unparseable hunk header means no trustworthy line
                # numbers, so treat the whole file as substantive.
                entry(path)["new"].append(-1)
                continue
            old_ln, new_ln = int(m.group(1)), int(m.group(2))
            continue
        if line.startswith("+"):
            if not is_ignorable_line(line[1:], path):
                entry(path)["new"].append(new_ln)
            new_ln += 1
        elif line.startswith("-"):
            if not is_ignorable_line(line[1:], path):
                entry(path)["old"].append(old_ln)
            old_ln += 1
        elif line.startswith(" "):
            old_ln += 1
            new_ln += 1

    hits: list[str] = []
    for hit_path, sides in changed.items():
        ranges = {
            "old": cfg_test_ranges(
                old_source(sides["was"]) if old_source else None
            ),
            "new": cfg_test_ranges(new_source(hit_path) if new_source else None),
        }
        for side in ("old", "new"):
            if any(not in_ranges(ranges[side], n) for n in sides[side]):
                hits.append(hit_path)
                break
    return hits


def git(*args: str) -> str:
    return subprocess.run(
        ["git", *args], check=True, capture_output=True, text=True
    ).stdout


def failure_message(files: list[str]) -> str:
    listed = "\n".join(f"    {f}" for f in sorted(set(files)))
    return f"""
ENFORCEMENT PAIRING FAILED

This pull request changes enforcement code but does not change the file that
states what is enforced ({CLAIMS_DOC}). Invariant 8 of CLAUDE.md is "claims
match enforcement"; this gate is what makes that pairing mechanical.

Enforcement files changed (test-only and comment-only changes are exempt):
{listed}

Do exactly ONE of these:

  1. Update {CLAIMS_DOC} in this pull request so the documented
     claims match what the code now enforces. (Preferred.)

  2. If the change genuinely alters no enforcement claim, waive it in
     writing. Put this line, with a real reason, in the pull-request body
     or in a commit-message trailer on this branch:

         {WAIVER_MARKER} <why no claim in {CLAIMS_DOC} changes>

     e.g. "{WAIVER_MARKER} refactor only; moves the deny check into a
     helper with identical behaviour."

The waiver is greppable on purpose — reviewers can find every one of them
with: git log --grep '{WAIVER_MARKER}'
"""


def blob_reader(rev: str):
    """Return a memoised `path -> file text at rev` reader.

    A path that does not exist at that revision (added or deleted file) reads
    as None, which exempts nothing on that side. See "FAIL TOWARD FIRING".
    """
    cache: dict[str, str | None] = {}

    def read(path: str) -> str | None:
        if path not in cache:
            try:
                cache[path] = git("show", f"{rev}:{path}")
            except subprocess.CalledProcessError:
                cache[path] = None
        return cache[path]

    return read


def check(base: str, head: str, waiver_text: str) -> int:
    diff = git("diff", "-U0", f"{base}...{head}")
    # `base...head` diffs the merge base against head, so pre-image line
    # numbers belong to the merge base, not to `base` itself.
    try:
        pre = git("merge-base", base, head).strip() or base
    except subprocess.CalledProcessError:
        pre = base
    files = substantive_files(diff, blob_reader(pre), blob_reader(head))
    if not files:
        print("No substantive enforcement-crate changes; pairing gate not applicable.")
        return 0

    names = git("diff", "--name-only", f"{base}...{head}").split()
    if CLAIMS_DOC in names:
        print(f"Enforcement code changed and {CLAIMS_DOC} changed with it. OK.")
        return 0

    commits = git("log", "--format=%B", f"{base}...{head}")
    m = WAIVER_RE.search(waiver_text) or WAIVER_RE.search(commits)
    if m:
        print(f"Waived: {WAIVER_MARKER} {m.group(1).strip()}")
        return 0

    print(failure_message(files), file=sys.stderr)
    return 1


def self_test() -> int:
    def d(path: str, body: str) -> str:
        return f"diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -1 +1 @@\n{body}\n"

    # Real code in a watched crate: fires.
    assert substantive_files(d("crates/trust/src/grant.rs", "+    deny(x);")) == [
        "crates/trust/src/grant.rs"
    ]
    assert substantive_files(d("crates/egress/src/proxy.rs", "-    allow(y);")) == [
        "crates/egress/src/proxy.rs"
    ]
    # Test-only and bench-only changes: exempt.
    assert substantive_files(d("crates/trust/tests/gate.rs", "+    assert!(x);")) == []
    assert substantive_files(d("crates/policy/src/tests.rs", "+    assert!(x);")) == []
    assert substantive_files(d("crates/egress/benches/b.rs", "+    bench(x);")) == []
    # Comment-only and blank-only changes: exempt.
    assert substantive_files(d("crates/policy/src/lib.rs", "+// explain the rule")) == []
    assert substantive_files(d("crates/policy/src/lib.rs", "+/// doc comment")) == []
    assert substantive_files(d("crates/policy/src/lib.rs", "+/* block")) == []
    assert substantive_files(d("crates/policy/src/lib.rs", "+ * continued")) == []
    assert substantive_files(d("crates/policy/src/lib.rs", "+")) == []
    # A crate version bump is exempt; a dependency change is not.
    assert substantive_files(d("crates/trust/Cargo.toml", '+version = "0.9.1"')) == []
    assert substantive_files(
        d("crates/trust/Cargo.toml", '+serde = { version = "1" }')
    ) == ["crates/trust/Cargo.toml"]
    # A `#` comment is a comment in TOML and an attribute in Rust.
    assert substantive_files(d("crates/trust/Cargo.toml", "+# rule 6: strict list")) == []
    assert substantive_files(d("crates/trust/src/lib.rs", "+#[derive(Debug)]")) == [
        "crates/trust/src/lib.rs"
    ]
    # Code with a trailing comment is still code.
    assert substantive_files(d("crates/policy/src/lib.rs", "+let a = 1; // why")) == [
        "crates/policy/src/lib.rs"
    ]
    # Unwatched crates never fire.
    assert substantive_files(d("crates/cli/src/main.rs", "+    go();")) == []
    assert substantive_files(d("docs/ENFORCEMENT.md", "+claim")) == []
    # Each file is reported at most once.
    assert substantive_files(
        d("crates/trust/src/a.rs", "+one\n+two")
    ) == ["crates/trust/src/a.rs"]
    # --- inline `#[cfg(test)]` regression cases ---------------------------
    #
    # The gate shipped exempting `tests/` directories and `tests.rs` only,
    # while its docstring promised "test code". This repository writes almost
    # all of its tests as inline `#[cfg(test)] mod tests`, so every test-only
    # pull request fired. These cases pin the fix.

    def dh(path: str, header: str, body: str) -> str:
        """A diff for `path` with a real hunk header, so line numbers matter."""
        return (
            f"diff --git a/{path} b/{path}\n"
            f"--- a/{path}\n+++ b/{path}\n{header}\n{body}\n"
        )

    def src(text: str):
        return lambda _path: text

    P = "crates/trust/src/lib.rs"
    # A file whose test module holds a brace inside a string literal, a brace
    # inside a comment, and a nested block — none of which may end the region.
    fixture = (
        "pub fn allow(x: u8) -> bool {\n"  # 1
        "    x < 4\n"  # 2
        "}\n"  # 3
        "\n"  # 4
        "#[cfg(test)]\n"  # 5
        "mod tests {\n"  # 6
        "    use super::*;\n"  # 7
        "\n"  # 8
        "    #[test]\n"  # 9
        "    fn braces_in_literals_do_not_close_the_module() {\n"  # 10
        '        assert_eq!(render("{"), "{"); // }\n'  # 11
        "        assert!(allow(1));\n"  # 12
        "    }\n"  # 13
        "}\n"  # 14
    )
    assert cfg_test_ranges(fixture) == [(5, 14)], cfg_test_ranges(fixture)

    # The defect itself: 0ec701e appended a whole `#[cfg(test)] mod tests`
    # block to crates/trust/src/lib.rs and the gate fired. It must not.
    added_module = "\n".join("+" + ln for ln in fixture.splitlines()[4:])
    assert substantive_files(
        dh(P, "@@ -4,0 +5,10 @@", added_module), src(""), src(fixture)
    ) == []
    # One line of real code outside the module still fires...
    assert substantive_files(
        dh(P, "@@ -2 +2 @@", "-    x < 4\n+    x < 5"), src(fixture), src(fixture)
    ) == [P]
    # ...including when it is mixed in with genuine test additions.
    assert substantive_files(
        dh(P, "@@ -2 +2 @@", "-    x < 4\n+    x < 5")
        + dh(P, "@@ -12,0 +13 @@", "+        assert!(allow(2));"),
        src(fixture),
        src(fixture),
    ) == [P]
    # A line inside the module, addressed by number, is exempt on both sides:
    # `+` lines are judged against the post-image, `-` lines against the
    # pre-image, so deleting a test is exempt too.
    assert substantive_files(
        dh(P, "@@ -12 +12 @@", "-        assert!(allow(1));"), src(fixture), src("")
    ) == []
    # The string-literal `{` on line 11 must not have closed the region early;
    # if it had, line 12 would read as outside and this would fire.
    assert substantive_files(
        dh(P, "@@ -12,0 +13 @@", "+        assert!(allow(3));"), src(""), src(fixture)
    ) == []

    # `#[cfg(test)]` on a plain `fn` (crates/egress/src/sni.rs does this) and
    # on a single `use` item, which has no block and ends at its `;`.
    item_fixture = (
        "#[cfg(test)]\n"  # 1
        "use std::io;\n"  # 2
        "\n"  # 3
        "#[cfg(test)]\n"  # 4
        "pub(crate) fn fixture() -> Vec<u8> {\n"  # 5
        "    vec![0x16]\n"  # 6
        "}\n"  # 7
        "\n"  # 8
        "pub fn deny() -> bool {\n"  # 9
        "    true\n"  # 10
        "}\n"  # 11
    )
    assert cfg_test_ranges(item_fixture) == [(1, 2), (4, 7)]
    assert substantive_files(
        dh(P, "@@ -6 +6 @@", "+    vec![0x16, 0x03]"), src(""), src(item_fixture)
    ) == []
    assert substantive_files(
        dh(P, "@@ -10 +10 @@", "+    false"), src(""), src(item_fixture)
    ) == [P]

    # Fail toward firing, never toward silence.
    # 1. No file image available (renamed away, unreadable, no resolver).
    assert substantive_files(dh(P, "@@ -6 +6 @@", "+    vec![]")) == [P]
    # 2. A `#[cfg(test)]` region that never closes yields no region at all.
    assert cfg_test_ranges("#[cfg(test)]\nmod tests {\n    fn a() {}\n") == []
    assert substantive_files(
        dh(P, "@@ -3 +3 @@", "+    fn b() {}"),
        src(""),
        src("#[cfg(test)]\nmod tests {\n    fn a() {}\n"),
    ) == [P]
    # 3. A cfg spelling the scanner does not claim to understand.
    assert cfg_test_ranges("#[cfg(all(test, unix))]\nmod tests {\n}\n") == []
    # 4. An unparseable hunk header means untrustworthy line numbers.
    assert substantive_files(
        dh(P, "@@ garbled @@", "+    vec![]"), src(""), src(fixture)
    ) == [P]

    # Masking: comments and literals must not contribute braces.
    assert "{" not in mask_literals_and_comments('let s = "{";')
    assert "}" not in mask_literals_and_comments("// }")
    assert "}" not in mask_literals_and_comments("/* nested /* } */ */")
    assert "{" not in mask_literals_and_comments("let c = '{';")
    assert "}" not in mask_literals_and_comments('let r = r#"a } b"#;')
    assert "}" not in mask_literals_and_comments('let b = br#"}"#;')
    # A lifetime is not a char literal: the code after it stays visible.
    assert "{" in mask_literals_and_comments("fn f<'a>(x: &'a str) {")
    # Masking preserves length and line structure, which line numbers rely on.
    assert len(mask_literals_and_comments(fixture)) == len(fixture)
    assert mask_literals_and_comments(fixture).count("\n") == fixture.count("\n")

    # Waiver parsing: a reason is required, a bare marker is not a waiver.
    assert WAIVER_RE.search(f"body\n{WAIVER_MARKER} refactor only\n")
    assert WAIVER_RE.search(f"{WAIVER_MARKER}   trailing reason")
    assert not WAIVER_RE.search(f"{WAIVER_MARKER}\n")
    assert not WAIVER_RE.search(f"{WAIVER_MARKER}   \n")
    assert not WAIVER_RE.search("no marker here")
    print("enforcement-pairing self-test OK")
    return 0


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--base")
    p.add_argument("--head", default="HEAD")
    p.add_argument(
        "--waiver-file",
        help="file containing the pull-request body (searched for the waiver marker)",
    )
    p.add_argument("--self-test", action="store_true")
    a = p.parse_args()
    if a.self_test:
        return self_test()
    if not a.base:
        p.error("--base is required (pull_request base sha)")
    text = ""
    if a.waiver_file and os.path.exists(a.waiver_file):
        with open(a.waiver_file, encoding="utf-8", errors="replace") as fh:
            text = fh.read()
    return check(a.base, a.head, text)


if __name__ == "__main__":
    sys.exit(main())
