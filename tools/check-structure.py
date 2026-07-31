#!/usr/bin/env python3
"""Phase 0 structural lint (TODO.md P0.3): every capability kind and policy
dimension in the manifest model must have real, matchable governance
artifacts — or an explicit, committed, honest reason why not.

Per capability kind (parsed FROM `crates/core/src/manifest/model.rs`'s
`Manifest` struct, so a newly added kind is lint-visible automatically):

  (a) manifest table   — definitionally true once parsed; not separately
                          checked as a failure mode.
  (b) lock pinning      — a `Locked<Kind>` struct in crates/core/src/lock.rs.
  (c) doctor probe       — evidence in crates/cli/src/commands/doctor.rs.
  (d) witness test       — a named test covering drift/pinning for the kind.
  (e) ENFORCEMENT.md row — a `### <Kind>` section or `**<Kind>**` matrix row
                          in docs/ENFORCEMENT.md (the .md source only).

Per policy dimension (parsed FROM the `Dimension` enum in the same file):

  a `### <Dimension>` row in docs/ENFORCEMENT.md. `FsDeny` is a deliberate
  exception: it is documented inline in the Filesystem read/write prose
  rather than as its own row, so it is matched by a literal prose anchor
  instead of a heading (see `check_dimension_row`).

Every requirement embeds WHERE it looks and WHAT counts as evidence in its
own docstring; `--explain` prints that evidence, found or missing, for every
kind and dimension.

A gap the code actually has is not a lint failure by itself — it only fails
if it is not recorded in the committed baseline (tools/check-structure-baseline.txt).
The lint also fails on a *stale* baseline entry: a line that no longer
corresponds to a real gap, because baselines drift stale exactly the way
code drifts stale, and a lint that only ever grows permissive isn't one.

Python 3 standard library only. Exits nonzero with a per-finding listing on
any failure.
"""

from __future__ import annotations

import re
import sys
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
MODEL_RS = REPO_ROOT / "crates/core/src/manifest/model.rs"
LOCK_RS = REPO_ROOT / "crates/core/src/lock.rs"
DOCTOR_RS = REPO_ROOT / "crates/cli/src/commands/doctor.rs"
ENFORCEMENT_MD = REPO_ROOT / "docs/ENFORCEMENT.md"
CRATES_DIR = REPO_ROOT / "crates"
BASELINE_FILE = REPO_ROOT / "tools/check-structure-baseline.txt"

# A capability kind's honest ENFORCEMENT.md section is not always titled
# exactly like its manifest field — extensions' section is "Native
# extensions", not "Extensions" (see docs/ENFORCEMENT.md's own per-cell
# notes). Every other kind is checked under its capitalized field name.
KIND_ENFORCEMENT_TITLES: dict[str, list[str]] = {
    "extensions": ["Extensions", "Native extensions"],
}

# Words this repo's own witness tests consistently use for drift/pin
# coverage (content_pinning.rs's *_drift_* tests, lock.rs's
# one_byte_extension_edit_refuses_locked_and_relock_regates). A test whose
# name only signals rendering/pruning/output-shape (e.g. hooks.rs's
# hooks_render_prune_and_preserve) correctly does not match any of these.
WITNESS_SIGNAL_WORDS: tuple[str, ...] = ("drift", "regate", "checksum")

# Human titles for policy dimensions that DO get a dedicated ENFORCEMENT.md
# section. FsDeny is deliberately absent from this map — see
# check_dimension_row.
DIMENSION_TITLES: dict[str, str] = {
    "Tools": "Tools",
    "Egress": "Egress",
    "Secrets": "Secrets",
    "FsRead": "Filesystem — read",
    "FsWrite": "Filesystem — write",
}

# FsDeny is documented inside the Filesystem read/write sections' prose,
# not as its own heading or matrix row (both subsections explicitly name
# "[policy.filesystem] deny" globs). This is a deliberate decision, recorded
# here rather than silently baselined as a gap or faked with an invented
# "### FsDeny" heading that doesn't exist in the doc.
FS_DENY_PROSE = "[policy.filesystem] deny"

_BASELINE_LINE_RE = re.compile(r"^([a-zA-Z0-9_]+:[a-zA-Z0-9_]+:[a-zA-Z0-9_]+)\s*#\s*(.+)$")
_TEST_FN_RE = re.compile(r"#\[test\](?:\s*#\[[^\]]*\])*\s*fn\s+(\w+)")


# --------------------------------------------------------------------------
# Parsing the manifest model itself (so a newly added kind/dimension is
# lint-visible automatically, per the spec).
# --------------------------------------------------------------------------
def parse_manifest_kinds(model_rs_text: str) -> list[str]:
    """Capability-bearing kind field names, in declared order.

    WHERE: the `pub struct Manifest { ... }` block in model.rs.
    WHAT: every `pub <field>: IndexMap<String, ...>` field, excluding
    `profiles` — a selection/bundling mechanism over the other kinds (its
    own doc comment calls it "Named bundles for selective loading"), not
    itself a pinned-content capability kind.
    """
    m = re.search(r"pub struct Manifest \{(.*?)\n\}", model_rs_text, re.DOTALL)
    if not m:
        raise RuntimeError("could not find `pub struct Manifest { ... }` in model.rs")
    fields = re.findall(r"pub (\w+): IndexMap<String,", m.group(1))
    return [f for f in fields if f != "profiles"]


def parse_dimensions(model_rs_text: str) -> list[str]:
    """Policy dimension enum variant names, in declared order.

    WHERE: `pub enum Dimension { ... }` in model.rs.
    WHAT: each bare variant name.
    """
    m = re.search(r"pub enum Dimension \{(.*?)\n\}", model_rs_text, re.DOTALL)
    if not m:
        raise RuntimeError("could not find `pub enum Dimension { ... }` in model.rs")
    return re.findall(r"^\s*(\w+),", m.group(1), re.MULTILINE)


def singular(kind: str) -> str:
    """Naive plural->singular: every current kind name ends in 's'
    (servers/skills/instructions/settings/hooks/extensions/workflows/packs),
    matching lock.rs's own `Locked<Singular>` naming convention. A future
    kind that doesn't end in 's' would mis-singularize here and likely fail
    the lock/doctor/witness checks below for the wrong reason — that failure
    would still be visible via --explain, which is the point: the lint
    surfaces evidence, it doesn't hide behind a heuristic silently.
    """
    return kind[:-1] if kind.endswith("s") else kind


# --------------------------------------------------------------------------
# Per-requirement checks. Each returns (found: bool, evidence: str).
# --------------------------------------------------------------------------
def check_lock_pin(kind: str, lock_rs_text: str) -> tuple[bool, str]:
    """(b) lock pinning.

    WHERE: crates/core/src/lock.rs.
    WHAT: `pub struct Locked<Singular>` (e.g. `LockedServer`, `LockedHook`) —
    the repo's own naming convention for a kind's pinned-source struct.
    """
    struct_name = f"Locked{singular(kind).capitalize()}"
    found = re.search(rf"pub struct {struct_name}\b", lock_rs_text) is not None
    evidence = f"pub struct {struct_name}" if found else f"no `pub struct {struct_name}` in lock.rs"
    return found, evidence


def check_doctor_probe(kind: str, doctor_rs_text: str) -> tuple[bool, str]:
    """(c) doctor probe.

    WHERE: crates/cli/src/commands/doctor.rs.
    WHAT: either a `report.section("<Kind capitalized>")` call (kinds probed
    under a shared section header, e.g. "Skills", "Hooks") OR a function
    named `fn check_<singular>_...` (kinds probed only through per-kind
    functions with no section header of their own, e.g.
    `check_server_reproducibility`, `check_workflow_ceilings`). Either is
    direct evidence of a dedicated doctor code path; neither is required
    exclusively of the other.
    """
    title = kind.capitalize()
    section_found = re.search(rf'report\.section\("{re.escape(title)}"\)', doctor_rs_text) is not None
    fn_found = re.search(rf"fn check_{re.escape(singular(kind))}_\w*", doctor_rs_text) is not None
    found = section_found or fn_found
    bits = [
        (f'report.section("{title}")' if section_found else f'no report.section("{title}")'),
        (f"fn check_{singular(kind)}_*" if fn_found else f"no fn check_{singular(kind)}_*"),
    ]
    return found, "; ".join(bits)


def find_test_fn_names(crates_dir: Path) -> list[tuple[Path, str]]:
    """Every `#[test]` fn name anywhere under crates_dir.

    WHERE: every *.rs file under crates_dir (excluding any `target` build
    dir) — not just crates/cli/tests: the extensions witness lives in an
    inline `#[cfg(test)] mod tests` inside commands/lock.rs, not a
    tests/*.rs integration file.
    WHAT: the function name immediately following a `#[test]` attribute,
    allowing intervening attributes (e.g. `#[should_panic]`).
    """
    out: list[tuple[Path, str]] = []
    if not crates_dir.is_dir():
        return out
    for rs_file in sorted(crates_dir.rglob("*.rs")):
        if "target" in rs_file.parts:
            continue
        text = rs_file.read_text(encoding="utf-8", errors="replace")
        for m in _TEST_FN_RE.finditer(text):
            out.append((rs_file, m.group(1)))
    return out


def check_witness_test(kind: str, test_fns: list[tuple[Path, str]]) -> tuple[bool, str]:
    """(d) witness test.

    WHERE: fn names collected by find_test_fn_names.
    WHAT: a test function whose (lowercased) name contains the kind's
    singular form AND one of WITNESS_SIGNAL_WORDS. A test that only signals
    rendering/pruning/output-shape does not count — that is a real,
    deliberate distinction (see WITNESS_SIGNAL_WORDS' docstring).
    """
    single = singular(kind)
    for path, name in test_fns:
        lname = name.lower()
        if single in lname and any(w in lname for w in WITNESS_SIGNAL_WORDS):
            return True, f"{name} ({path.name})"
    return (
        False,
        f'no #[test] fn name contains "{single}" + one of {WITNESS_SIGNAL_WORDS}',
    )


def check_enforcement_row(kind: str, enforcement_md_text: str) -> tuple[bool, str]:
    """(e) ENFORCEMENT.md row/statement.

    WHERE: docs/ENFORCEMENT.md (the .md source only — never the compiled
    .html build output).
    WHAT: a `### <Title>` section heading, or a `**<Title>**` bold matrix-row
    cell, for one of the kind's known title spellings
    (KIND_ENFORCEMENT_TITLES, defaulting to the capitalized field name).
    """
    titles = KIND_ENFORCEMENT_TITLES.get(kind, [kind.capitalize()])
    for title in titles:
        if re.search(rf"^### {re.escape(title)}\s*$", enforcement_md_text, re.MULTILINE):
            return True, f"### {title}"
        if re.search(rf"\*\*{re.escape(title)}\*\*", enforcement_md_text):
            return True, f"**{title}** matrix row"
    return False, f"no ### section or **bold** matrix row for any of {titles}"


def check_dimension_row(dim: str, enforcement_md_text: str) -> tuple[bool, str]:
    """Policy-dimension ENFORCEMENT.md row.

    WHERE: docs/ENFORCEMENT.md.
    WHAT: a `### <Title>` heading from DIMENSION_TITLES — except FsDeny,
    which is deliberately matched by the literal prose anchor FS_DENY_PROSE
    instead (see that constant's docstring for why).
    """
    if dim == "FsDeny":
        found = FS_DENY_PROSE in enforcement_md_text
        evidence = f'prose mention of "{FS_DENY_PROSE}"' if found else f'no prose mention of "{FS_DENY_PROSE}"'
        return found, evidence
    title = DIMENSION_TITLES.get(dim)
    if title is None:
        return False, f"no known title mapping for dimension {dim!r} — check-structure.py needs updating"
    found = re.search(rf"^### {re.escape(title)}\s*$", enforcement_md_text, re.MULTILINE) is not None
    return found, (f"### {title}" if found else f"no ### {title} section")


# --------------------------------------------------------------------------
# Gap computation
# --------------------------------------------------------------------------
def compute_evidence(
    model_text: str,
    lock_text: str,
    doctor_text: str,
    enforcement_text: str,
    test_fns: list[tuple[Path, str]],
) -> tuple[dict[str, tuple[bool, str]], list[str], list[str]]:
    kinds = parse_manifest_kinds(model_text)
    dims = parse_dimensions(model_text)
    evidence: dict[str, tuple[bool, str]] = {}

    for kind in kinds:
        evidence[f"kind:{kind}:lock"] = check_lock_pin(kind, lock_text)
        evidence[f"kind:{kind}:doctor"] = check_doctor_probe(kind, doctor_text)
        evidence[f"kind:{kind}:witness"] = check_witness_test(kind, test_fns)
        evidence[f"kind:{kind}:enforcement"] = check_enforcement_row(kind, enforcement_text)

    for dim in dims:
        evidence[f"dimension:{dim}:enforcement"] = check_dimension_row(dim, enforcement_text)

    return evidence, kinds, dims


# --------------------------------------------------------------------------
# Baseline
# --------------------------------------------------------------------------
def load_baseline(path: Path) -> dict[str, str]:
    """key -> reason. Lines: `<key> # <reason>`. Blank lines and lines
    starting with `#` (after stripping) are header/commentary, ignored."""
    out: dict[str, str] = {}
    if not path.is_file():
        return out
    for lineno, raw in enumerate(path.read_text(encoding="utf-8").splitlines(), start=1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        m = _BASELINE_LINE_RE.match(line)
        if not m:
            raise ValueError(f"{path}:{lineno}: malformed baseline line: {raw!r}")
        key, reason = m.group(1), m.group(2).strip()
        if key in out:
            raise ValueError(f"{path}:{lineno}: duplicate baseline key {key!r}")
        out[key] = reason
    return out


# --------------------------------------------------------------------------
# Top-level check, shared by real run and self-test
# --------------------------------------------------------------------------
def run_structure_check(
    findings: list[str],
    model_text: str,
    lock_text: str,
    doctor_text: str,
    enforcement_text: str,
    test_fns: list[tuple[Path, str]],
    baseline: dict[str, str],
) -> dict[str, tuple[bool, str]]:
    evidence, _kinds, _dims = compute_evidence(model_text, lock_text, doctor_text, enforcement_text, test_fns)
    gap_keys = {k for k, (ok, _) in evidence.items() if not ok}
    baseline_keys = set(baseline)

    for key in sorted(gap_keys - baseline_keys):
        _ok, ev = evidence[key]
        findings.append(f"real gap not in baseline: {key} -- evidence: {ev}")

    for key in sorted(baseline_keys - gap_keys):
        findings.append(f"stale baseline entry (no longer a real gap): {key} -- baseline reason: {baseline[key]!r}")

    return evidence


def print_explain(evidence: dict[str, tuple[bool, str]], baseline: dict[str, str]) -> None:
    print("\n--explain: per-requirement evidence\n")
    for key in sorted(evidence):
        ok, ev = evidence[key]
        status = "OK  " if ok else "GAP "
        baselined = " (baselined)" if (not ok and key in baseline) else ""
        print(f"  {status}{key}{baselined}: {ev}")


# --------------------------------------------------------------------------
# Self-test: proves the checker itself catches the three required breakages,
# using synthesized temp fixtures only — never the real repo tree.
# --------------------------------------------------------------------------
def self_test() -> int:
    failures: list[str] = []

    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        crates = root / "crates"
        (crates / "cli" / "tests").mkdir(parents=True)

        model_text = """
pub struct Manifest {
    pub version: u32,
    pub servers: IndexMap<String, Server>,
    pub skills: IndexMap<String, Skill>,
    pub profiles: IndexMap<String, Profile>,
}

pub enum Dimension {
    Tools,
    Egress,
}
"""
        lock_text = """
pub struct LockedServer {
    pub name: String,
}

pub struct LockedSkill {
    pub name: String,
}
"""
        doctor_text = """
fn run_checks() {
    report.section("Servers");
    report.section("Skills");
}
"""
        enforcement_text = """
### Servers

Servers are pinned and probed.

### Skills

Skills are pinned and probed.

### Tools

Tools are enforced in gateway mode.
"""
        test_file = crates / "cli" / "tests" / "fixture_test.rs"
        test_file.write_text(
            "#[test]\nfn server_drift_blocks_apply() {}\n"
            "#[test]\nfn skill_render_prune_and_preserve() {}\n",
            encoding="utf-8",
        )

        # Baseline deliberately: (1) omits the real skills:witness gap and
        # the real dimension:Egress:enforcement gap (both must surface as
        # "not in baseline" findings), and (2) includes a stale entry for
        # kind:servers:witness, which is NOT a real gap in this fixture
        # (must surface as a "stale baseline" finding).
        baseline = {"kind:servers:witness": "fixture stale entry"}

        test_fns = find_test_fn_names(crates)
        findings: list[str] = []
        run_structure_check(findings, model_text, lock_text, doctor_text, enforcement_text, test_fns, baseline)

        joined = "\n".join(findings)
        if "kind:skills:witness" not in joined or "not in baseline" not in joined:
            failures.append("self-test: kind missing a witness test, and missing from baseline, NOT caught")
        if "dimension:Egress:enforcement" not in joined or "not in baseline" not in joined:
            failures.append("self-test: policy dimension without an ENFORCEMENT.md row NOT caught")
        if "kind:servers:witness" not in joined or "stale" not in joined:
            failures.append("self-test: stale baseline entry NOT caught")
        # Kinds/dimensions that ARE fully covered must not spuriously appear
        # as gaps (sanity check on the checks themselves).
        if "kind:servers:lock" in joined or "kind:servers:doctor" in joined or "kind:servers:enforcement" in joined:
            failures.append("self-test: fully-covered servers requirement wrongly flagged as a gap")
        if "dimension:Tools:enforcement" in joined:
            failures.append("self-test: fully-covered Tools dimension wrongly flagged as a gap")

    if failures:
        print("SELF-TEST FAILED:")
        for f in failures:
            print(f"  - {f}")
        return 1
    print(
        "self-test: OK (missing witness, dimension without a row, and a stale "
        "baseline entry are all caught; fully-covered requirements are not)"
    )
    return 0


# --------------------------------------------------------------------------
# main
# --------------------------------------------------------------------------
def main(argv: list[str]) -> int:
    if "--self-test" in argv:
        return self_test()

    # The self-test always runs first so CI knows the checker itself works.
    rc = self_test()
    if rc != 0:
        return rc

    for path in (MODEL_RS, LOCK_RS, DOCTOR_RS, ENFORCEMENT_MD):
        if not path.is_file():
            print(f"ERROR: expected source file not found: {path}")
            return 2

    model_text = MODEL_RS.read_text(encoding="utf-8")
    lock_text = LOCK_RS.read_text(encoding="utf-8")
    doctor_text = DOCTOR_RS.read_text(encoding="utf-8")
    enforcement_text = ENFORCEMENT_MD.read_text(encoding="utf-8")
    test_fns = find_test_fn_names(CRATES_DIR)

    try:
        baseline = load_baseline(BASELINE_FILE)
    except ValueError as exc:
        print(f"ERROR: {exc}")
        return 2

    findings: list[str] = []
    evidence = run_structure_check(findings, model_text, lock_text, doctor_text, enforcement_text, test_fns, baseline)

    if "--explain" in argv:
        print_explain(evidence, baseline)

    if findings:
        print(f"\ncheck-structure: {len(findings)} finding(s):\n")
        for f in findings:
            print(f"  FAIL {f}")
        return 1

    print(
        f"check-structure: OK ({len(evidence)} requirement(s) checked, "
        f"{len(baseline)} baselined gap(s), all others satisfied)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
