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
  * comment-only and blank-line changes in the remaining files.

Rationale: added tests and rewritten comments cannot make the shipped
enforcement disagree with ENFORCEMENT.md — they are evidence and prose, not
behaviour. A gate that fires on "I added a witness test" gets disabled within
a week, and a disabled gate defends nothing. The cost of the exemption is
that a behaviour change smuggled in as a `#[cfg(test)]` module escapes; that
is an acceptable trade, because such a change cannot affect a release build.

  * `#[cfg(test)]` inline modules are NOT parsed out. Doing so needs a Rust
    parser; the directory/comment heuristic is the part that can be kept
    obviously correct in a hundred lines of stdlib Python.

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


def is_ignorable_line(line: str) -> bool:
    """True for a changed line that cannot alter behaviour.

    `line` is a unified-diff body line WITHOUT its leading +/-.
    """
    s = line.strip()
    if not s:
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


def substantive_files(diff_text: str) -> list[str]:
    """Watched, non-test files whose diff has >=1 non-comment changed line.

    Parses `git diff -U0` output: `diff --git a/X b/X` headers, then `+`/`-`
    body lines (skipping the `+++`/`---` file headers and `@@` hunk headers).
    """
    hits: list[str] = []
    path: str | None = None
    counted = False
    for line in diff_text.splitlines():
        if line.startswith("diff --git "):
            path, counted = None, False
            m = re.match(r"diff --git a/(.*) b/(.*)$", line)
            if m:
                cand = m.group(2)
                if is_watched(cand) and not is_test_path(cand):
                    path = cand
            continue
        if path is None or counted:
            continue
        if line.startswith(("+++", "---", "@@")):
            continue
        if line.startswith(("+", "-")) and not is_ignorable_line(line[1:]):
            hits.append(path)
            counted = True
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


def check(base: str, head: str, waiver_text: str) -> int:
    diff = git("diff", "-U0", f"{base}...{head}")
    files = substantive_files(diff)
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
