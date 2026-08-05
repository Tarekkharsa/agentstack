#!/usr/bin/env python3
"""Phase 0 structural lint (TODO.md P0.3): every capability kind and policy
dimension in the manifest model must have real, matchable governance
artifacts — or an explicit, committed, honest reason why not.

Per capability kind:

  (a) manifest table    — definitionally true once parsed; not separately
                           checked as a failure mode.
  (b) lock pinning       — a `Locked<Kind>` struct in crates/core/src/lock.rs.
  (c) doctor probe       — evidence in crates/cli/src/commands/doctor.rs.
  (d) witness test       — an EXPLICITLY REGISTERED (file, #[test] fn name)
                           pair in WITNESS_REGISTRY below — see that
                           constant's docstring for why this is a registry
                           and not fuzzy name-matching.
  (e) consent review    — a `diff.mark("<kind>"…)` call in
                           crates/cli/src/commands/trust.rs, i.e. the kind is
                           disclosed on the review card AND recorded in the
                           consented surface. See check_review_disclosure.
  (f) ENFORCEMENT.md row — a `### <Title>` section with a real (>= 3
                           non-blank line) body, AND — for the kinds in
                           KIND_MATRIX_ROW_REQUIRED — a matching
                           `| **<Title>** |` matrix row. See
                           check_enforcement_row's docstring.

Per policy dimension: the same heading+body(+row) requirement as (e) above,
via check_dimension_row (requirement (f) above) — except FsDeny, a deliberate exception matched by a
literal prose anchor instead (see FS_DENY_PROSE).

Independently of all the above, CRATE-EDGE INTEGRITY (verify_crate_edges)
parses the allowed internal crate edges straight out of docs/ARCHITECTURE.md
("Crate dependency rules") and compares them against the real
`crates/*/Cargo.toml` dependency tables. The doc says "anything not listed is
forbidden"; before this check that sentence was enforced by reviewer memory
alone. Like kind-set integrity it is NOT baseline-able: an undocumented edge
is an architecture change, and the only two honest fixes are to drop the
dependency or to change the doc on the record.

Independently of all the above, KIND-SET INTEGRITY (verify_kind_set) parses
every `pub <field>:` in the Manifest struct — regardless of declared type —
and asserts it is exactly EXPECTED_KINDS ∪ CONFIG_ALLOWLIST. This is not a
baseline-able gap: an unaccounted-for field or a missing EXPECTED_KINDS
member always fails the lint, on the theory that a manifest field is either a
governed capability kind or a config knob, and the person adding it has to
say which — the checker must not be foolable by hiding a new/removed kind
behind a type alias, a BTreeMap/Vec instead of an IndexMap, or silent
deletion.

Every requirement embeds WHERE it looks and WHAT counts as evidence in its
own docstring; `--explain` prints that evidence, found or missing, for every
kind and dimension.

A gap the code actually has is not a lint failure by itself — it only fails
if it is not recorded in the committed baseline (tools/check-structure-baseline.txt).
The lint also fails on a *stale* baseline entry: a line that no longer
corresponds to a real gap, because baselines drift stale exactly the way
code drifts stale, and a lint that only ever grows permissive isn't one.

ACCEPTED RESIDUALS (deliberate scope limits, not oversights — round-3
hardening closed the bypasses that were actually found; these two remain by
design):

  (a) Body-line thresholds (MIN_SECTION_BODY_LINES) can be met with three
      lines of filler prose that say nothing. This lint pins STRUCTURE — a
      heading exists, has a body of real length, and (where required) a
      matrix row — not MEANING. It cannot tell a genuine explanation of what
      is and isn't confined from three throwaway sentences that happen to
      clear the line count.
  (b) The WITNESS_REGISTRY only pins that a named `#[test]` fn exists in its
      registered file and is not `#[ignore]`d — i.e. that it will run. It
      does not and cannot pin that the fn's assertions are meaningful; a
      registered witness reduced to `assert!(true)` still passes this check.
      Semantic strength of a witness — whether it actually exercises the
      drift/pin/regate behavior it claims to — is the job of code review and
      mutation testing, not this script.

Python 3 standard library only. Exits nonzero with a per-finding listing on
any failure.
"""

from __future__ import annotations

import re
import sys
import tempfile
from pathlib import Path

try:
    import tomllib  # stdlib since Python 3.11
except ModuleNotFoundError:  # pragma: no cover - refusal path, not a lint result
    print("refusing: check-structure.py needs Python 3.11+ (stdlib tomllib) to read Cargo.toml")
    sys.exit(2)

REPO_ROOT = Path(__file__).resolve().parent.parent
MODEL_RS = REPO_ROOT / "crates/core/src/manifest/model.rs"
LOCK_RS = REPO_ROOT / "crates/core/src/lock.rs"
DOCTOR_RS = REPO_ROOT / "crates/cli/src/commands/doctor.rs"
TRUST_RS = REPO_ROOT / "crates/cli/src/commands/trust.rs"
ENFORCEMENT_MD = REPO_ROOT / "docs/ENFORCEMENT.md"
ARCHITECTURE_MD = REPO_ROOT / "docs/ARCHITECTURE.md"
CRATES_DIR = REPO_ROOT / "crates"
BASELINE_FILE = REPO_ROOT / "tools/check-structure-baseline.txt"

# --------------------------------------------------------------------------
# Finding 3 fixture: the manifest's capability kinds, named explicitly. This
# is deliberately NOT derived from a regex over field *types* (that is
# exactly what an IndexMap -> BTreeMap/Vec/type-alias tamper defeats) — it is
# the checker's own ground truth, which verify_kind_set() cross-checks
# against every `pub <field>:` name in the real Manifest struct regardless of
# type. Adding, removing, or renaming a capability kind in model.rs without
# updating this tuple is a hard failure, not a baseline entry.
EXPECTED_KINDS: tuple[str, ...] = (
    "servers",
    "skills",
    "instructions",
    "settings",
    "hooks",
    "extensions",
    "workflows",
    "packs",
)

# Manifest struct fields that are configuration/bookkeeping, not a pinned
# capability kind with its own governance surface. Verified against
# model.rs's actual field list (version, meta, servers, skills, profiles,
# instructions, settings, hooks, extensions, workflows, packs,
# package_overrides, targets, policy, guard, experimental, delivery) minus
# EXPECTED_KINDS.
CONFIG_ALLOWLIST: frozenset[str] = frozenset(
    {
        "version",
        "meta",
        "profiles",
        "targets",
        "policy",
        "guard",
        "experimental",
        # Per-member project divergence from a selected package: which
        # members a taken package's selection is remove/replace-overridden
        # for. It modifies package member selection, not a capability kind.
        "package_overrides",
        # The delivery-lane override table (render_locally, project-wide and
        # per-harness). It steers how already-declared capabilities are
        # delivered, not a capability kind of its own.
        "delivery",
    }
)

# --------------------------------------------------------------------------
# Finding 1 fixture: the witness-test registry.
#
# WHY A REGISTRY AND NOT FUZZY NAME-MATCHING: the previous version of this
# checker accepted any `#[test]` fn whose (lowercased) name merely contained
# the kind's singular form plus a signal word ("drift"/"regate"/"checksum").
# That is defeated by deleting the *specific* test that actually exercises
# drift/pin behavior — the checker doesn't know which fn it was, so it can't
# notice one is gone as long as some other fn with a matching name pattern
# still exists (or even by deleting the only one and having nothing else
# match, which just silently reports the same gap it already tolerates).
#
# The fix: name the exact witnesses. `<kind> -> [(file, exact fn name), ...]`.
# check_witness_test requires EVERY listed (file, fn) pair to exist, each
# preceded by `#[test]` (other attributes may sit between, e.g.
# `#[should_panic]`). Renaming or deleting a registered witness now fails the
# lint until this registry is consciously updated to match — that is the
# point, not a bug: a witness disappearing silently is exactly the tamper
# this lint exists to catch.
#
# A capability kind with NO entry here is a witness gap by definition (the
# baseline records it, same as any other gap) — this covers hooks, settings,
# and packs today, none of which have a drift/pin witness at all.
WITNESS_REGISTRY: dict[str, list[tuple[str, str]]] = {
    "skills": [
        ("crates/cli/tests/content_pinning.rs", "inline_skill_drift_blocks_activation_until_relocked"),
        ("crates/cli/tests/content_pinning.rs", "unpinned_first_activation_proceeds_and_pins"),
    ],
    "instructions": [
        ("crates/cli/tests/content_pinning.rs", "instruction_drift_blocks_apply_until_relocked"),
    ],
    "workflows": [
        ("crates/cli/tests/content_pinning.rs", "workflow_drift_and_roles_widening_block_trust_until_relocked"),
    ],
    "extensions": [
        ("crates/cli/src/commands/lock.rs", "one_byte_extension_edit_refuses_locked_and_relock_regates"),
    ],
    "servers": [
        ("crates/cli/src/resolve.rs", "server_definition_change_is_checksum_drift"),
        ("crates/cli/src/resolve.rs", "server_checksum_reflects_definition_file"),
    ],
}

# --------------------------------------------------------------------------
# Finding 2 fixture: ENFORCEMENT.md title/row registries.
#
# A capability kind's honest ENFORCEMENT.md section is not always titled
# exactly like its manifest field — extensions' section is "Native
# extensions", not "Extensions". Every other kind is checked under its
# capitalized field name (the dict's default via .get(kind, [kind.title()])).
KIND_ENFORCEMENT_TITLES: dict[str, list[str]] = {
    "extensions": ["Extensions", "Native extensions"],
}

# Kinds whose ENFORCEMENT.md matrix has a dedicated row today (as opposed to
# only a section, or nothing at all). For these, a heading+body alone is no
# longer sufficient evidence — the matrix row must independently survive
# too, closing the "delete just the row, keep the prose" tamper.
KIND_MATRIX_ROW_REQUIRED: frozenset[str] = frozenset({"extensions", "hooks"})

# Human titles for policy dimensions that DO get a dedicated ENFORCEMENT.md
# section + matrix row. FsDeny is deliberately absent — see check_dimension_row.
DIMENSION_TITLES: dict[str, str] = {
    "Tools": "Tools",
    "Egress": "Egress",
    "Secrets": "Secrets",
    "FsRead": "Filesystem — read",
    "FsWrite": "Filesystem — write",
}

# FsDeny is documented inside the Filesystem read/write sections' prose, not
# as its own heading or matrix row (both subsections explicitly name
# "[policy.filesystem] deny" globs). This is a deliberate decision, recorded
# here rather than silently baselined as a gap or faked with an invented
# "### FsDeny" heading that doesn't exist in the doc. The literal-string
# match is itself tamper-resistant in the same spirit as the registry above:
# there is exactly one place in the doc this can live, and it must say this
# exact phrase.
FS_DENY_PROSE = "[policy.filesystem] deny"

# A gutted section (heading kept, body hollowed out to a throwaway line) must
# still fail. Three is a deliberately low bar — every real section in this
# repo's ENFORCEMENT.md clears it by a wide margin — chosen so the check
# fails on an actual gutting, not on ordinary editorial trimming.
MIN_SECTION_BODY_LINES = 3

_BASELINE_LINE_RE = re.compile(r"^([a-zA-Z0-9_]+:[a-zA-Z0-9_]+:[a-zA-Z0-9_]+)\s*#\s*(.+)$")
_IGNORE_ATTR_RE = re.compile(r"#\[ignore\b")
_CFG_ATTR_RE = re.compile(r"#\[cfg\b")


def _attr_block_for_fn(text: str, fn_name: str) -> str | None:
    """The contiguous attribute lines directly above `fn <fn_name>`, or None
    when the fn does not exist. Scanning the WHOLE block (not just what
    follows `#[test]`) closes the round-3 bypasses where `#[ignore]` or a
    compiling-it-out `#[cfg(...)]` sits BEFORE `#[test]` — attribute order is
    irrelevant to rustc, so it must be irrelevant here too."""
    m = re.search(
        rf"((?:^[ \t]*#\[[^\n]*\n)*)[ \t]*(?:pub[ \t]+)?fn[ \t]+{re.escape(fn_name)}\b",
        text,
        re.MULTILINE,
    )
    return m.group(1) if m else None
_FENCED_BLOCK_RE = re.compile(r"```.*?```", re.DOTALL)


def strip_fenced_code_blocks(text: str) -> str:
    """B8c hardening: a ``` ... ``` fenced code block must never count as
    prose evidence. Without this, a section's body-line-count requirement or
    the FS_DENY_PROSE literal-anchor search can both be satisfied by planting
    filler or the exact anchor string inside a fence instead of real
    narrative text. Fences always come in open/close pairs in valid
    Markdown, so a non-greedy match from one ``` to the next removes each
    block whole; anything left over is real prose."""
    return _FENCED_BLOCK_RE.sub("", text)


# --------------------------------------------------------------------------
# Parsing the manifest model itself.
# --------------------------------------------------------------------------
def parse_manifest_fields(model_rs_text: str) -> list[str]:
    """Every `pub <field>:` name in the `pub struct Manifest { ... }` body,
    regardless of declared type.

    WHERE: model.rs's `Manifest` struct.
    WHAT: this is deliberately type-blind — the whole point of finding 3 is
    that a kind hidden behind a type alias, a BTreeMap/Vec swapped in for an
    IndexMap, or any other type-shape change must still show up as a field
    name here. verify_kind_set() is what turns this list into a pass/fail.
    """
    # Decoy guard (E3/1e/1f hardening): count the bare IDENTIFIER, not one
    # exact declaration spelling — a decoy struct planted to shadow this
    # parser must itself contain `struct Manifest`, whatever whitespace,
    # attributes, or where-clauses dress up either declaration. Exactly one
    # occurrence in the file or the parser refuses outright; a mention in a
    # doc comment trips this too, and the refusal message says how to fix it.
    count = len(re.findall(r"\bstruct\s+Manifest\b", model_rs_text))
    if count != 1:
        print(
            f"refusing: {count} occurrences of `struct Manifest` in model.rs — the "
            f"kind-set parser needs exactly one (no decoys, no doc-comment mentions)"
        )
        sys.exit(2)
    m = re.search(r"pub struct Manifest \{(.*?)\n\}", model_rs_text, re.DOTALL)
    if not m:
        raise RuntimeError("could not find `pub struct Manifest { ... }` in model.rs")
    return re.findall(r"^\s*pub (\w+):", m.group(1), re.MULTILINE)


def verify_kind_set(model_rs_text: str) -> list[str]:
    """Finding 3: kind-set integrity. Returns a list of error strings (empty
    = OK). NOT baseline-able — a mismatch here always fails the lint,
    regardless of tools/check-structure-baseline.txt, because it is a claim
    about the checker's own coverage being complete, not an honestly-recorded
    product gap.
    """
    fields = parse_manifest_fields(model_rs_text)
    errors: list[str] = []
    seen_kinds: set[str] = set()
    for field in fields:
        if field in EXPECTED_KINDS:
            seen_kinds.add(field)
        elif field in CONFIG_ALLOWLIST:
            continue
        else:
            errors.append(
                f"new manifest field {field!r}: declare it a kind (EXPECTED_KINDS) or "
                f"config (CONFIG_ALLOWLIST) in check-structure.py"
            )
    for kind in EXPECTED_KINDS:
        if kind not in seen_kinds:
            errors.append(f"EXPECTED_KINDS member {kind!r} is missing from the Manifest struct fields")
    return errors


# --------------------------------------------------------------------------
# Crate-edge integrity: docs/ARCHITECTURE.md is the authority.
#
# The doc carries the rule in a fenced block introduced by EDGE_RULES_ANCHOR:
#
#     core     → (nothing)
#     trust    → core, recorder
#     ...
#     cli      → everything
#
# That shape is machine-readable, so the edge set is PARSED rather than
# copied here — the doc stays the single authority and the two can never
# disagree. Node names are the crate DIRECTORY names under crates/ (`cli`,
# not the `agentstack` package name it publishes as).
EDGE_RULES_ANCHOR = "Exact internal edges (anything not listed is forbidden):"
EDGE_WILDCARD = "everything"   # `cli → everything`: every internal crate is allowed
EDGE_NONE = "(nothing)"        # `core → (nothing)`: no internal edge at all
_EDGE_LINE_RE = re.compile(r"^[ \t]*([a-z0-9_-]+)[ \t]*(?:→|->)[ \t]*(\S.*?)[ \t]*$")

# Which Cargo.toml tables carry an internal edge.
#
# DEV-DEPENDENCIES ARE IN SCOPE, deliberately. A dev-dependency is a real edge
# in the crate graph: it compiles into the crate's own test targets, it can
# create a dependency cycle, and it would let a security-critical crate
# (`trust`, `policy`) acquire authority through its test surface without the
# architecture rule ever being consulted. ARCHITECTURE.md says "exact internal
# edges" with no test exemption, so neither does this check. If a dev-only
# edge is ever genuinely wanted, the doc is where that decision gets made.
# Optional (feature-gated) dependencies are in scope for the same reason:
# `agentstack-egress` is still an edge, it is just one a feature turns on.
_DEP_TABLE_NAMES = ("dependencies", "dev-dependencies", "build-dependencies")


def parse_allowed_edges(architecture_md_text: str) -> dict[str, frozenset[str] | None]:
    """The allowed internal edges, straight from docs/ARCHITECTURE.md.

    WHERE: the fenced block introduced by EDGE_RULES_ANCHOR under
    "## Crate dependency rules".
    WHAT: `<crate> → <targets>` per line. Returns `crate -> frozenset(targets)`,
    with `None` standing for the EDGE_WILDCARD row (`cli → everything`) and an
    empty frozenset for EDGE_NONE (`core → (nothing)`).

    A crate that is ABSENT from the block is not an error here — the doc's own
    sentence ("anything not listed is forbidden") makes an unlisted crate a
    crate with zero allowed edges, which is how verify_crate_edges reads it.
    """
    # Same decoy guard as parse_manifest_fields: the anchor sentence must
    # occur exactly once, so a second copy planted above a permissive fake
    # block cannot silently become the one this parser reads.
    count = architecture_md_text.count(EDGE_RULES_ANCHOR)
    if count != 1:
        print(
            f"refusing: {count} occurrences of the crate-edge anchor sentence in ARCHITECTURE.md — "
            f"the edge parser needs exactly one (expected: {EDGE_RULES_ANCHOR!r})"
        )
        sys.exit(2)
    m = re.search(re.escape(EDGE_RULES_ANCHOR) + r"\s*\n+```[a-z]*\n(.*?)\n?```", architecture_md_text, re.DOTALL)
    if not m:
        print("refusing: no fenced edge block follows the crate-edge anchor sentence in ARCHITECTURE.md")
        sys.exit(2)
    allowed: dict[str, frozenset[str] | None] = {}
    for raw in m.group(1).splitlines():
        if not raw.strip():
            continue
        line = _EDGE_LINE_RE.match(raw)
        if not line:
            print(f"refusing: unparseable line in the ARCHITECTURE.md edge block: {raw!r}")
            sys.exit(2)
        crate, targets = line.group(1), line.group(2)
        if crate in allowed:
            print(f"refusing: crate {crate!r} listed twice in the ARCHITECTURE.md edge block")
            sys.exit(2)
        if targets == EDGE_WILDCARD:
            allowed[crate] = None
        elif targets == EDGE_NONE:
            allowed[crate] = frozenset()
        else:
            allowed[crate] = frozenset(t.strip() for t in targets.split(",") if t.strip())
    if not allowed:
        print("refusing: the ARCHITECTURE.md edge block parsed to zero edges")
        sys.exit(2)
    return allowed


def collect_internal_edges(crates_dir: Path) -> dict[str, dict[str, list[str]]]:
    """Every internal edge the workspace actually declares.

    WHERE: each `crates/<dir>/Cargo.toml`.
    WHAT: `<dir> -> {target dir -> [table labels it is declared in]}`, over
    `[dependencies]`, `[dev-dependencies]`, `[build-dependencies]` AND their
    `[target.'cfg(...)'.…]` variants — a cfg-gated table is an ordinary edge
    that happens to be conditional, and leaving it out would be a one-line
    bypass of the whole check.

    A dependency counts as internal when it resolves to a sibling crate
    directory: by its `path` first (so a `package = ` rename or an
    off-convention key name cannot hide it), then by package name, then by the
    bare key. Everything else is an external crate and none of this check's
    business.
    """
    pkg_to_dir: dict[str, str] = {}
    parsed: dict[str, dict] = {}
    for cargo_toml in sorted(crates_dir.glob("*/Cargo.toml")):
        dir_name = cargo_toml.parent.name
        try:
            data = tomllib.loads(cargo_toml.read_text(encoding="utf-8"))
        except tomllib.TOMLDecodeError as exc:
            print(f"refusing: {cargo_toml} is not parseable TOML: {exc}")
            sys.exit(2)
        parsed[dir_name] = data
        pkg_name = data.get("package", {}).get("name")
        if isinstance(pkg_name, str):
            pkg_to_dir[pkg_name] = dir_name

    def resolve(key: str, spec: object, crate_dir: Path) -> str | None:
        path = spec.get("path") if isinstance(spec, dict) else None
        if isinstance(path, str):
            resolved = (crate_dir / path).resolve()
            if resolved.parent == crates_dir.resolve() and resolved.name in parsed:
                return resolved.name
        package = spec.get("package") if isinstance(spec, dict) else None
        name = package if isinstance(package, str) else key
        return pkg_to_dir.get(name)

    edges: dict[str, dict[str, list[str]]] = {}
    for dir_name, data in parsed.items():
        crate_dir = crates_dir / dir_name
        tables: list[tuple[str, dict]] = []
        for table_name in _DEP_TABLE_NAMES:
            table = data.get(table_name)
            if isinstance(table, dict):
                tables.append((f"[{table_name}]", table))
        targets = data.get("target")
        if isinstance(targets, dict):
            for cfg, cfg_tables in sorted(targets.items()):
                if not isinstance(cfg_tables, dict):
                    continue
                for table_name in _DEP_TABLE_NAMES:
                    table = cfg_tables.get(table_name)
                    if isinstance(table, dict):
                        tables.append((f"[target.'{cfg}'.{table_name}]", table))
        found: dict[str, list[str]] = {}
        for label, table in tables:
            for key, spec in table.items():
                target = resolve(key, spec, crate_dir)
                if target is None or target == dir_name:
                    continue
                found.setdefault(target, []).append(label)
        edges[dir_name] = found
    return edges


def verify_crate_edges(
    edges: dict[str, dict[str, list[str]]],
    allowed: dict[str, frozenset[str] | None],
) -> list[str]:
    """Crate-edge integrity. Returns a list of error strings (empty = OK).

    NOT baseline-able, for the same reason kind-set integrity is not: an edge
    the doc does not list is not an honestly-recorded product gap, it is an
    architecture change nobody wrote down. The two honest fixes are named in
    the failure message — drop the dependency, or change the edge block in
    docs/ARCHITECTURE.md and take the review that comes with it.
    """
    errors: list[str] = []
    for crate in sorted(allowed):
        if crate not in edges:
            errors.append(
                f"docs/ARCHITECTURE.md lists edges for crate {crate!r}, but crates/{crate}/Cargo.toml "
                f"does not exist — remove the stale row from the edge block"
            )
    for crate in sorted(edges):
        allowed_targets = allowed.get(crate, frozenset())
        if allowed_targets is None:  # `→ everything`
            continue
        for target, tables in sorted(edges[crate].items()):
            if target in allowed_targets:
                continue
            if crate in allowed:
                permitted = ", ".join(sorted(allowed_targets)) or EDGE_NONE
                rule = f"ARCHITECTURE.md allows `{crate} → {permitted}`"
            else:
                rule = (
                    f"ARCHITECTURE.md does not list `{crate}` in its edge block at all, and "
                    f"anything not listed is forbidden"
                )
            errors.append(
                f"forbidden internal edge `{crate} → {target}`: declared in "
                f"{', '.join(sorted(set(tables)))} of crates/{crate}/Cargo.toml, but {rule}. "
                f"Fix: drop the dependency from crates/{crate}/Cargo.toml, or change the architecture "
                f'on the record in docs/ARCHITECTURE.md ("Crate dependency rules")'
            )
    return errors


def print_edge_explain(
    edges: dict[str, dict[str, list[str]]],
    allowed: dict[str, frozenset[str] | None],
) -> None:
    print("\n--explain: internal crate edges (allowed per docs/ARCHITECTURE.md)\n")
    for crate in sorted(edges):
        allowed_targets = allowed.get(crate, frozenset())
        if allowed_targets is None:
            permitted = EDGE_WILDCARD
        else:
            permitted = ", ".join(sorted(allowed_targets)) or EDGE_NONE
        declared = ", ".join(sorted(edges[crate])) or EDGE_NONE
        listed = "" if crate in allowed else " (crate NOT listed in ARCHITECTURE.md)"
        print(f"  {crate}: declared -> {declared} | allowed -> {permitted}{listed}")


def parse_dimensions(model_rs_text: str) -> list[str]:
    """Policy dimension enum variant names, in declared order.

    WHERE: `pub enum Dimension { ... }` in model.rs.
    WHAT: each bare variant name.
    """
    # Same decoy guard as parse_manifest_fields: bare-identifier count.
    count = len(re.findall(r"\benum\s+Dimension\b", model_rs_text))
    if count != 1:
        print(
            f"refusing: {count} occurrences of `enum Dimension` in model.rs — the "
            f"dimension parser needs exactly one (no decoys, no doc-comment mentions)"
        )
        sys.exit(2)
    m = re.search(r"pub enum Dimension \{(.*?)\n\}", model_rs_text, re.DOTALL)
    if not m:
        raise RuntimeError("could not find `pub enum Dimension { ... }` in model.rs")
    return re.findall(r"^\s*(\w+),", m.group(1), re.MULTILINE)


def singular(kind: str) -> str:
    """Naive plural->singular: every current kind name ends in 's'
    (servers/skills/instructions/settings/hooks/extensions/workflows/packs),
    matching lock.rs's own `Locked<Singular>` naming convention.
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
    WHAT: either a `report.section("<Kind capitalized>")` call OR a function
    named `fn check_<singular>_...`. Either is direct evidence of a
    dedicated doctor code path; neither is required exclusively of the other.
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


def check_review_disclosure(kind: str, trust_rs_text: str) -> tuple[bool, str]:
    """(e) consent-review disclosure.

    WHERE: crates/cli/src/commands/trust.rs (the review card in `grant_gated`).
    WHAT: a `diff.mark("<singular>"` or `diff.mark("<kind>"` call.

    WHY `mark` and not a print: `mark` is what records the item into the
    reviewed surface that the grant persists, so a kind that calls it provably
    appears on the card AND in the stored surface the next re-trust diffs
    against. A `println!` alone could show a kind while leaving it out of the
    consented surface — which is the weaker property, not the one that matters.

    This requirement exists because a real gap shipped: [hooks.*] and
    [settings.*] were declared, re-gated the trust digest when edited, and were
    disclosed NOWHERE on the review screen — so a user re-consented to a hook
    they were never shown. That is a consent surprise on an executable kind.
    Fixing it was a fact about one commit; this check is what makes it a
    property, and what stops kind #9 from repeating it.
    """
    # PRODUCTION code only. The unit tests in trust.rs drive `mark` directly
    # with fixture kind names, and those calls would otherwise satisfy this
    # requirement for a kind the real review never discloses — which is exactly
    # the bug this check exists to catch, passing itself off as the fix. Found
    # for real: `kind:skills:review` was briefly green off a line in the test
    # module while the production call site had been renamed.
    production = trust_rs_text.split("#[cfg(test)]", 1)[0]
    sing = singular(kind)
    # Both recorders count: `mark` and `mark_pinned` push the same SurfaceItem
    # into the consented surface; the latter additionally records the pin.
    pat = rf'diff\.mark(?:_pinned)?\(\s*"(?:{re.escape(sing)}|{re.escape(kind)})"'
    found = re.search(pat, production) is not None
    evidence = (
        f'diff.mark("{sing}"…) in trust.rs'
        if found
        else f'no diff.mark("{sing}"/"{kind}"…) in trust.rs (outside #[cfg(test)]) '
        f"— kind is not disclosed on the consent card"
    )
    return found, evidence


def check_witness_test(
    kind: str,
    repo_root: Path,
    registry: dict[str, list[tuple[str, str]]] | None = None,
) -> tuple[bool, str]:
    """(d) witness test, per WITNESS_REGISTRY.

    WHERE: the exact `(file, fn name)` pairs registered for `kind`.
    WHAT: each registered fn must exist in its registered file with `#[test]`
    somewhere in the contiguous attribute block directly above it, and that
    WHOLE block (attribute order is irrelevant to rustc, so it is irrelevant
    here) must contain neither `#[ignore]` nor any `#[cfg(...)]` — a witness
    that never runs, or that a cfg can compile out, proves nothing and fails
    the lint exactly like a deleted witness would. A kind with no registry
    entry is a gap by definition — the baseline decides whether that gap is
    acceptable, same as any other requirement.
    """
    if registry is None:
        registry = WITNESS_REGISTRY
    entries = registry.get(kind)
    if entries is None:
        return (
            False,
            f"no WITNESS_REGISTRY entry for kind {kind!r} — a kind with no registered "
            f"witness is a gap by definition (see WITNESS_REGISTRY's docstring)",
        )
    present: list[str] = []
    missing: list[str] = []
    for rel_path, fn_name in entries:
        file_path = repo_root / rel_path
        found = False
        if file_path.is_file():
            text = file_path.read_text(encoding="utf-8", errors="replace")
            block = _attr_block_for_fn(text, fn_name)
            if block is not None and "#[test]" in block:
                if _IGNORE_ATTR_RE.search(block):
                    return (
                        False,
                        f"registered witness {fn_name} is #[ignore]d — a witness that "
                        f"never runs is not a witness",
                    )
                if _CFG_ATTR_RE.search(block):
                    return (
                        False,
                        f"registered witness {fn_name} carries #[cfg(...)] — a witness "
                        f"that can be compiled out is not unconditional; registered "
                        f"witnesses must run everywhere",
                    )
                found = True
        (present if found else missing).append(f"{fn_name} ({rel_path})")
    if missing:
        return False, f"missing registered witness fn(s): {', '.join(missing)}"
    return True, f"registered witness fn(s) present: {', '.join(present)}"


def get_section_body(title: str, text: str) -> str | None:
    """Text of a `### <title>` section body: everything between the heading
    line and the next `##`- or `###`-level heading (or end of file), with any
    fenced code blocks (``` ... ```) stripped out first (B8c hardening) — a
    fence of filler lines must not be able to satisfy the non-blank-line
    floor, and a fence-planted anchor string must not be able to satisfy a
    prose-anchor search either. Returns None if no such heading exists.
    Shared by check_enforcement_row and check_dimension_row so a tamper only
    has one extraction path to defeat.
    """
    pattern = rf"^### {re.escape(title)}[ \t]*\n(.*?)(?=\n#{{2,3}}[ \t]|\Z)"
    m = re.search(pattern, text, re.MULTILINE | re.DOTALL)
    if not m:
        return None
    body = strip_fenced_code_blocks(m.group(1))
    # Round-3 hardening (2b): 4-space/tab-indented lines are Markdown code
    # blocks outside list context and must not count as prose either — the
    # real ENFORCEMENT.md bodies indent list continuations by two spaces, so
    # this strips only genuine code plants, verified against the live doc.
    return "\n".join(
        line for line in body.splitlines() if not re.match(r"^(?: {4}|\t)\S", line)
    )


def _count_nonblank_lines(body: str) -> int:
    return sum(1 for line in body.splitlines() if line.strip())


def has_matrix_row(title: str, text: str) -> bool:
    return re.search(rf"^\|\s*\*\*{re.escape(title)}\*\*\s*\|", text, re.MULTILINE) is not None


def check_enforcement_row(
    kind: str,
    enforcement_md_text: str,
    titles_map: dict[str, list[str]] | None = None,
    row_required_kinds: frozenset[str] | None = None,
) -> tuple[bool, str]:
    """(e) ENFORCEMENT.md evidence for a capability kind.

    WHERE: docs/ENFORCEMENT.md (the .md source only — never the compiled
    .html build output).
    WHAT: BOTH (a) a `### <Title>` heading whose body (text before the next
    `##`/`###` heading) has at least MIN_SECTION_BODY_LINES non-blank lines,
    AND (b) — only for kinds in KIND_MATRIX_ROW_REQUIRED, i.e. kinds that
    have a dedicated matrix row today — a `| **<Title>** |` line in the
    matrix. A bare bold mention with no real heading/body no longer counts
    (closes the "throwaway bold mention" tamper), and a heading whose row was
    quietly deleted no longer counts either (closes "row-only deletion").
    """
    if titles_map is None:
        titles_map = KIND_ENFORCEMENT_TITLES
    if row_required_kinds is None:
        row_required_kinds = KIND_MATRIX_ROW_REQUIRED
    titles = titles_map.get(kind, [kind.capitalize()])
    best_reason: str | None = None
    for title in titles:
        body = get_section_body(title, enforcement_md_text)
        if body is None:
            if best_reason is None:
                best_reason = f"no ### {title} heading"
            continue
        n = _count_nonblank_lines(body)
        if n < MIN_SECTION_BODY_LINES:
            best_reason = (
                f"### {title} heading found but body has only {n} non-blank line(s) "
                f"(need >= {MIN_SECTION_BODY_LINES})"
            )
            continue
        if kind in row_required_kinds and not has_matrix_row(title, enforcement_md_text):
            best_reason = f"### {title} heading + body OK, but no '| **{title}** |' matrix row"
            continue
        row_note = ", matrix row present" if kind in row_required_kinds else ""
        return True, f"### {title} (body {n} non-blank line(s){row_note})"
    return False, best_reason or f"no ### heading found for any of {titles}"


def check_dimension_row(
    dim: str,
    enforcement_md_text: str,
    titles_map: dict[str, str] | None = None,
) -> tuple[bool, str]:
    """Policy-dimension ENFORCEMENT.md evidence: the same heading+body+row
    requirement as check_enforcement_row, collapsed to a single title (no
    aliasing) — except FsDeny, matched by the literal FS_DENY_PROSE anchor
    instead of a heading, because it deliberately has neither a heading nor a
    matrix row of its own (see FS_DENY_PROSE's docstring). Every other
    dimension in DIMENSION_TITLES has a real matrix row today, so the row
    check is unconditional for them.
    """
    if titles_map is None:
        titles_map = DIMENSION_TITLES
    if dim == "FsDeny":
        # Round-3 hardening (2c): the anchor must live where the doc claims
        # it lives — inside a Filesystem section's own (code-stripped) body —
        # not merely anywhere in the file, so a literal planted at EOF or in
        # a trailing code region is never evidence.
        found = any(
            body is not None and FS_DENY_PROSE in body
            for body in (
                get_section_body(titles_map["FsWrite"], enforcement_md_text),
                get_section_body(titles_map["FsRead"], enforcement_md_text),
            )
        )
        evidence = (
            f'prose mention of "{FS_DENY_PROSE}" inside a Filesystem section'
            if found
            else f'no prose mention of "{FS_DENY_PROSE}" inside a Filesystem section'
        )
        return found, evidence
    title = titles_map.get(dim)
    if title is None:
        return False, f"no known title mapping for dimension {dim!r} — check-structure.py needs updating"
    body = get_section_body(title, enforcement_md_text)
    if body is None:
        return False, f"no ### {title} heading"
    n = _count_nonblank_lines(body)
    if n < MIN_SECTION_BODY_LINES:
        return False, (
            f"### {title} heading found but body has only {n} non-blank line(s) "
            f"(need >= {MIN_SECTION_BODY_LINES})"
        )
    if not has_matrix_row(title, enforcement_md_text):
        return False, f"### {title} heading + body OK, but no '| **{title}** |' matrix row"
    return True, f"### {title} (body {n} non-blank line(s), matrix row present)"


# --------------------------------------------------------------------------
# Gap computation
# --------------------------------------------------------------------------
def compute_evidence(
    model_text: str,
    lock_text: str,
    doctor_text: str,
    trust_text: str,
    enforcement_text: str,
    repo_root: Path,
    kinds: list[str] | None = None,
    dims: list[str] | None = None,
    witness_registry: dict[str, list[tuple[str, str]]] | None = None,
    kind_titles: dict[str, list[str]] | None = None,
    kind_row_required: frozenset[str] | None = None,
    dim_titles: dict[str, str] | None = None,
) -> tuple[dict[str, tuple[bool, str]], list[str], list[str]]:
    if kinds is None:
        kinds = list(EXPECTED_KINDS)
    if dims is None:
        dims = parse_dimensions(model_text)
    evidence: dict[str, tuple[bool, str]] = {}

    for kind in kinds:
        evidence[f"kind:{kind}:lock"] = check_lock_pin(kind, lock_text)
        evidence[f"kind:{kind}:doctor"] = check_doctor_probe(kind, doctor_text)
        evidence[f"kind:{kind}:witness"] = check_witness_test(kind, repo_root, witness_registry)
        evidence[f"kind:{kind}:review"] = check_review_disclosure(kind, trust_text)
        evidence[f"kind:{kind}:enforcement"] = check_enforcement_row(
            kind, enforcement_text, kind_titles, kind_row_required
        )

    for dim in dims:
        evidence[f"dimension:{dim}:enforcement"] = check_dimension_row(dim, enforcement_text, dim_titles)

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
    trust_text: str,
    enforcement_text: str,
    repo_root: Path,
    baseline: dict[str, str],
    **compute_evidence_kwargs,
) -> dict[str, tuple[bool, str]]:
    evidence, _kinds, _dims = compute_evidence(
        model_text, lock_text, doctor_text, trust_text, enforcement_text, repo_root, **compute_evidence_kwargs
    )
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
# Self-test: proves the checker itself catches every required breakage,
# using synthesized temp fixtures only — never the real repo tree.
# --------------------------------------------------------------------------
def self_test() -> int:
    failures: list[str] = []

    # ---- Finding 3: kind-set integrity ----------------------------------
    # A field's declared TYPE changing (IndexMap -> BTreeMap) must not hide
    # it: parse_manifest_fields/verify_kind_set only look at field names.
    bad_model_text = """
pub struct Manifest {
    pub version: u32,
    pub servers: IndexMap<String, Server>,
    pub skills: BTreeMap<String, Skill>,
    pub profiles: IndexMap<String, Profile>,
    pub rogue_field: Vec<RogueThing>,
}

pub enum Dimension {
    Tools,
    Egress,
}
"""
    kind_set_errors = verify_kind_set(bad_model_text)
    joined_kind_set_errors = "\n".join(kind_set_errors)
    if "rogue_field" not in joined_kind_set_errors:
        failures.append("self-test: manifest field not in EXPECTED_KINDS/CONFIG_ALLOWLIST NOT caught")
    if "'instructions'" not in joined_kind_set_errors or "missing from the Manifest struct" not in joined_kind_set_errors:
        failures.append("self-test: EXPECTED_KINDS member missing from the Manifest struct NOT caught")
    if "'servers'" in joined_kind_set_errors or "'skills'" in joined_kind_set_errors:
        failures.append("self-test: present-and-accounted-for kind wrongly flagged by kind-set integrity")

    fully_covered_model_text = "pub struct Manifest {\n" + "\n".join(
        f"    pub {field}: SomeType," for field in (*EXPECTED_KINDS, *sorted(CONFIG_ALLOWLIST))
    ) + "\n}\n"
    if verify_kind_set(fully_covered_model_text):
        failures.append("self-test: fully-covered Manifest field set wrongly flagged by kind-set integrity")

    # ---- Findings F1/F2: check_lock_pin / check_doctor_probe must actually
    # fail on a fixture missing their evidence, not merely pass when the
    # evidence happens to be present. Without a fixture exercising the
    # "missing" branch directly, an always-pass stub of either function
    # (`return True, "stub"` regardless of input) would sail through
    # self-test undetected, since every other self-test fixture only ever
    # calls these through a fully-covered lock_text/doctor_text. -----------
    lock_present_text = "pub struct LockedServer { pub name: String }\n"
    lock_missing_text = "pub struct SomethingElseEntirely { pub name: String }\n"
    ok_lock_present, _ev_lock_present = check_lock_pin("servers", lock_present_text)
    ok_lock_missing, ev_lock_missing = check_lock_pin("servers", lock_missing_text)
    if not ok_lock_present:
        failures.append("self-test: check_lock_pin wrongly failed on a present LockedServer struct")
    if ok_lock_missing or "no `pub struct LockedServer`" not in ev_lock_missing:
        failures.append(
            "self-test: check_lock_pin did NOT fail on a lock.rs fixture missing LockedServer "
            "(an always-pass stub would go undetected)"
        )

    doctor_present_text = 'fn run_checks() { report.section("Servers"); }\n'
    doctor_missing_text = 'fn run_checks() { report.section("SomethingElse"); }\n'
    ok_doctor_present, _ev_doctor_present = check_doctor_probe("servers", doctor_present_text)
    ok_doctor_missing, ev_doctor_missing = check_doctor_probe("servers", doctor_missing_text)
    if not ok_doctor_present:
        failures.append('self-test: check_doctor_probe wrongly failed on a present report.section("Servers") probe')
    if (
        ok_doctor_missing
        or 'no report.section("Servers")' not in ev_doctor_missing
        or "no fn check_server_*" not in ev_doctor_missing
    ):
        failures.append(
            "self-test: check_doctor_probe did NOT fail on a doctor.rs fixture missing the probe "
            "(an always-pass stub would go undetected)"
        )

    # ---- check_review_disclosure: the requirement added because [hooks.*]
    # and [settings.*] really did ship undisclosed on the consent screen. The
    # "missing" branch is the whole point of this check, so it is exercised
    # directly — an always-pass stub here would silently re-open exactly the
    # gap the requirement exists to close. ---------------------------------
    review_present_text = 'fn grant_gated() { let mk = diff.mark("hook", name, &identity); }\n'
    # Printing a kind is NOT disclosure: only `mark` records it into the
    # consented surface, so a review line without a mark must still read as a
    # gap. This fixture is the one that would pass a laxer `"hook" in text`.
    review_printonly_text = 'fn grant_gated() { say!("  hooks: {}", hook.command); }\n'
    ok_review_present, _ev = check_review_disclosure("hooks", review_present_text)
    ok_review_missing, ev_review_missing = check_review_disclosure("hooks", review_printonly_text)
    if not ok_review_present:
        failures.append('self-test: check_review_disclosure wrongly failed on a present diff.mark("hook") call')
    if ok_review_missing or "is not disclosed on the consent card" not in ev_review_missing:
        failures.append(
            "self-test: check_review_disclosure did NOT fail on a trust.rs fixture that only PRINTS "
            "the kind without marking it into the consented surface "
            "(an always-pass stub, or a mere substring check, would go undetected)"
        )
    # Plural-named kinds mark under their plural (`settings`), singular-named
    # ones under the singular (`hook`) — both spellings must count, or the
    # check would report a false gap for whichever convention it missed.
    ok_plural, _ev = check_review_disclosure("settings", 'diff.mark("settings", a, &i);\n')
    if not ok_plural:
        failures.append('self-test: check_review_disclosure rejected the plural spelling diff.mark("settings")')
    # `mark_pinned` records the same item plus its pin, so it is disclosure too.
    ok_pinned, _ev = check_review_disclosure("skills", 'diff.mark_pinned("skill", n, o, p);\n')
    if not ok_pinned:
        failures.append('self-test: check_review_disclosure did not accept diff.mark_pinned("skill")')
    # A mark that exists ONLY in the test module must NOT satisfy the
    # requirement — found for real, see check_review_disclosure's docstring.
    test_only_text = (
        'fn grant_gated() { say!("skills:"); }\n'
        "#[cfg(test)]\nmod tests {\n    fn t() { diff.mark(\"skill\", n, i); }\n}\n"
    )
    ok_test_only, ev_test_only = check_review_disclosure("skills", test_only_text)
    if ok_test_only or "outside #[cfg(test)]" not in ev_test_only:
        failures.append(
            "self-test: check_review_disclosure was satisfied by a diff.mark call that exists only "
            "in the test module (production disclosure could be deleted undetected)"
        )

    # ---- Crate-edge integrity -------------------------------------------
    # Proves the check CATCHES a forbidden edge, not merely that it runs: a
    # synthetic workspace in a temp dir declares one allowed edge, one
    # forbidden edge in [dependencies], one forbidden dev-dependency, one
    # forbidden cfg-gated dependency, and one edge hidden behind a `package =`
    # rename. All four bad ones must be named; the good ones must not be.
    fixture_architecture = (
        "## Crate dependency rules\n\n"
        f"{EDGE_RULES_ANCHOR}\n\n"
        "```\n"
        "core     → (nothing)\n"
        "policy   → core\n"
        "adapters → core\n"
        "cli      → everything\n"
        "ghost    → core\n"
        "```\n\nProse after the block.\n"
    )
    fixture_allowed = parse_allowed_edges(fixture_architecture)
    if fixture_allowed.get("core") != frozenset():
        failures.append("self-test: `core → (nothing)` did not parse to an empty allowed set")
    if fixture_allowed.get("cli") is not None:
        failures.append("self-test: `cli → everything` did not parse to the wildcard")
    if fixture_allowed.get("policy") != frozenset({"core"}):
        failures.append("self-test: a single-target edge row did not parse to that one target")

    with tempfile.TemporaryDirectory() as edge_tmp:
        edge_crates = Path(edge_tmp) / "crates"
        for dir_name, body in (
            ("core", '[package]\nname = "agentstack-core"\n'),
            # allowed: policy → core
            ("policy", '[package]\nname = "agentstack-policy"\n\n[dependencies]\nagentstack-core = { path = "../core" }\n'),
            # forbidden: adapters → policy (a real, deliberate rule — see
            # ARCHITECTURE.md's "adapters deliberately does not depend on policy")
            (
                "adapters",
                '[package]\nname = "agentstack-adapters"\n\n[dependencies]\n'
                'agentstack-core = { path = "../core" }\n'
                'agentstack-policy = { path = "../policy" }\n',
            ),
            # forbidden, and only visible if dev-dependencies are in scope
            (
                "core-dev",
                '[package]\nname = "agentstack-core-dev"\n\n[dev-dependencies]\n'
                'agentstack-policy = { path = "../policy" }\n',
            ),
            # forbidden, and only visible if target-cfg tables are in scope
            (
                "cfgcrate",
                '[package]\nname = "agentstack-cfgcrate"\n\n'
                "[target.'cfg(unix)'.dependencies]\n"
                'agentstack-policy = { path = "../policy" }\n',
            ),
            # forbidden, and only visible if `package =` renames are resolved
            (
                "renamer",
                '[package]\nname = "agentstack-renamer"\n\n[dependencies]\n'
                'aliased = { path = "../policy", package = "agentstack-policy" }\n',
            ),
            # allowed by the wildcard row
            (
                "cli",
                '[package]\nname = "agentstack"\n\n[dependencies]\n'
                'agentstack-core = { path = "../core" }\n'
                'agentstack-policy = { path = "../policy" }\n'
                'serde = "1"\n',
            ),
        ):
            (edge_crates / dir_name).mkdir(parents=True)
            (edge_crates / dir_name / "Cargo.toml").write_text(body, encoding="utf-8")

        fixture_edges = collect_internal_edges(edge_crates)
        edge_errors = "\n".join(verify_crate_edges(fixture_edges, fixture_allowed))

        if "`adapters → policy`" not in edge_errors:
            failures.append("self-test: forbidden [dependencies] edge NOT caught")
        if "crates/adapters/Cargo.toml" not in edge_errors:
            failures.append("self-test: forbidden-edge message does not name the file to change")
        if "`core-dev → policy`" not in edge_errors or "[dev-dependencies]" not in edge_errors:
            failures.append("self-test: forbidden [dev-dependencies] edge NOT caught")
        if "`cfgcrate → policy`" not in edge_errors or "target.'cfg(unix)'" not in edge_errors:
            failures.append("self-test: forbidden cfg-gated edge NOT caught")
        if "`renamer → policy`" not in edge_errors:
            failures.append("self-test: forbidden edge hidden behind a `package =` rename NOT caught")
        # An unlisted crate has zero allowed edges, and the message must say so
        # rather than implying the doc granted it an empty set on purpose.
        if "does not list `cfgcrate`" not in edge_errors:
            failures.append("self-test: a crate absent from the edge block was not reported as unlisted")
        if "`policy → core`" in edge_errors or "`cli →" in edge_errors:
            failures.append("self-test: an ALLOWED edge (or a wildcard row's edge) wrongly flagged as forbidden")
        if "core" not in fixture_edges or fixture_edges["core"]:
            failures.append("self-test: a crate with no internal dependencies did not collect to zero edges")
        if "serde" in edge_errors:
            failures.append("self-test: an external dependency was mistaken for an internal edge")
        if "'ghost'" not in edge_errors or "does not exist" not in edge_errors:
            failures.append("self-test: an edge-block row for a crate that does not exist NOT caught")

    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        tests_dir = root / "crates" / "cli" / "tests"
        tests_dir.mkdir(parents=True)

        # ---- Finding 1: witness registry -----------------------------
        (tests_dir / "fixture_witness.rs").write_text(
            "#[test]\nfn kind_a_drift_blocks_apply() {}\n",
            encoding="utf-8",
        )
        fixture_registry = {
            "kinda": [("crates/cli/tests/fixture_witness.rs", "kind_a_drift_blocks_apply")],
            # kind_b's registered fn was never written to the fixture file —
            # this is the "witness deleted" scenario.
            "kindb": [("crates/cli/tests/fixture_witness.rs", "kind_b_drift_blocks_apply")],
        }
        ok_a, ev_a = check_witness_test("kinda", root, registry=fixture_registry)
        ok_b, ev_b = check_witness_test("kindb", root, registry=fixture_registry)
        ok_c, ev_c = check_witness_test("kindc", root, registry=fixture_registry)
        if not ok_a:
            failures.append("self-test: registered witness fn that DOES exist wrongly flagged missing")
        if ok_b or "kind_b_drift_blocks_apply" not in ev_b:
            failures.append("self-test: deleted registered witness fn NOT caught")
        if ok_c or "no WITNESS_REGISTRY entry" not in ev_c:
            failures.append("self-test: kind with no witness registry entry NOT treated as a gap")

        # Round-3: the WHOLE attribute block is scanned — #[ignore] and
        # #[cfg(...)] must fail whether they sit before or after #[test].
        for label, attrs in [
            ("ignore-after-test", "#[test]\n#[ignore]\n"),
            ("ignore-before-test", "#[ignore]\n#[test]\n"),
            ("ignore-with-reason-before-test", '#[ignore = "wip"]\n#[test]\n'),
            ("cfg-before-test", "#[cfg(any())]\n#[test]\n"),
            ("cfg-after-test", "#[test]\n#[cfg(any())]\n"),
        ]:
            (tests_dir / "fixture_witness.rs").write_text(
                f"{attrs}fn kind_a_drift_blocks_apply() {{}}\n", encoding="utf-8"
            )
            ok_dis, _ = check_witness_test("kinda", root, registry=fixture_registry)
            if ok_dis:
                failures.append(f"self-test: disabled registered witness ({label}) NOT caught")
        # Restore the healthy fixture for anything below that reuses it.
        (tests_dir / "fixture_witness.rs").write_text(
            "#[test]\nfn kind_a_drift_blocks_apply() {}\n", encoding="utf-8"
        )

        # ---- Finding 2: gutted section body / missing matrix row -----
        gutted_text = "### Widget\n\none line only.\n\n| **Widget** | enforced |\n"
        missing_row_text = (
            "### Widget\n\nline one of real prose.\nline two of real prose.\nline three of real prose.\n"
        )
        healthy_text = missing_row_text + "\n| **Widget** | enforced |\n"

        ok_gutted, ev_gutted = check_enforcement_row(
            "widgets", gutted_text, titles_map={"widgets": ["Widget"]}, row_required_kinds=frozenset({"widgets"})
        )
        if ok_gutted or "body has only" not in ev_gutted:
            failures.append("self-test: gutted section body under a surviving heading NOT caught (kind)")

        ok_no_row, ev_no_row = check_enforcement_row(
            "widgets", missing_row_text, titles_map={"widgets": ["Widget"]}, row_required_kinds=frozenset({"widgets"})
        )
        if ok_no_row or "matrix row" not in ev_no_row:
            failures.append("self-test: missing matrix row with a surviving section NOT caught (kind)")

        ok_healthy, _ = check_enforcement_row(
            "widgets", healthy_text, titles_map={"widgets": ["Widget"]}, row_required_kinds=frozenset({"widgets"})
        )
        if not ok_healthy:
            failures.append("self-test: fully-covered kind (heading+body+row) wrongly flagged as a gap")

        # A kind NOT in row_required_kinds must pass on heading+body alone.
        ok_no_row_required, _ = check_enforcement_row(
            "widgets", missing_row_text, titles_map={"widgets": ["Widget"]}, row_required_kinds=frozenset()
        )
        if not ok_no_row_required:
            failures.append("self-test: kind outside row_required_kinds wrongly required to have a matrix row")

        # Dimension variant of the same two breakages.
        ok_dim_gutted, ev_dim_gutted = check_dimension_row("Widg", gutted_text, titles_map={"Widg": "Widget"})
        if ok_dim_gutted or "body has only" not in ev_dim_gutted:
            failures.append("self-test: gutted section body under a surviving heading NOT caught (dimension)")

        ok_dim_no_row, ev_dim_no_row = check_dimension_row("Widg", missing_row_text, titles_map={"Widg": "Widget"})
        if ok_dim_no_row or "matrix row" not in ev_dim_no_row:
            failures.append("self-test: missing matrix row with a surviving section NOT caught (dimension)")

        ok_dim_healthy, _ = check_dimension_row("Widg", healthy_text, titles_map={"Widg": "Widget"})
        if not ok_dim_healthy:
            failures.append("self-test: fully-covered dimension (heading+body+row) wrongly flagged as a gap")

        # FsDeny stays anchor-only, but the anchor must sit INSIDE a
        # Filesystem section's own body (round-3 scoping) — a plant at EOF,
        # in a fence, or in an indented code line is never evidence.
        fs_write_title = DIMENSION_TITLES["FsWrite"]
        fsdeny_scoped = (
            f"### {fs_write_title}\n\nreal prose mentioning {FS_DENY_PROSE} here.\nmore prose.\nand more.\n"
        )
        ok_fsdeny, _ = check_dimension_row("FsDeny", fsdeny_scoped)
        if not ok_fsdeny:
            failures.append("self-test: FsDeny prose anchor wrongly flagged as missing")
        ok_fsdeny_missing, _ = check_dimension_row("FsDeny", "no anchor text here.")
        if ok_fsdeny_missing:
            failures.append("self-test: FsDeny prose anchor absence NOT caught")
        ok_fsdeny_eof, _ = check_dimension_row(
            "FsDeny",
            f"### {fs_write_title}\n\nprose without the anchor.\nmore.\nand more.\n\n## Other\n\n{FS_DENY_PROSE}\n",
        )
        if ok_fsdeny_eof:
            failures.append("self-test: FsDeny anchor OUTSIDE a Filesystem section wrongly accepted")
        ok_fsdeny_indent, _ = check_dimension_row(
            "FsDeny",
            f"### {fs_write_title}\n\nprose without the anchor.\nmore.\nand more.\n\n    {FS_DENY_PROSE}\n",
        )
        if ok_fsdeny_indent:
            failures.append("self-test: FsDeny anchor in an indented code line wrongly accepted")

        # ---- Baseline mechanics (missing-from-baseline + stale entry) -
        (tests_dir / "servers_witness.rs").write_text(
            "#[test]\nfn server_definition_change_is_checksum_drift() {}\n"
            "#[test]\nfn server_checksum_reflects_definition_file() {}\n",
            encoding="utf-8",
        )
        # skills is intentionally left with only ONE of its two registered
        # witnesses -> a real, uncovered gap this run must surface.
        (tests_dir / "skills_witness.rs").write_text(
            "#[test]\nfn inline_skill_drift_blocks_activation_until_relocked() {}\n",
            encoding="utf-8",
        )
        baseline_registry = {
            "servers": [
                ("crates/cli/tests/servers_witness.rs", "server_definition_change_is_checksum_drift"),
                ("crates/cli/tests/servers_witness.rs", "server_checksum_reflects_definition_file"),
            ],
            "skills": [
                ("crates/cli/tests/skills_witness.rs", "inline_skill_drift_blocks_activation_until_relocked"),
                ("crates/cli/tests/skills_witness.rs", "unpinned_first_activation_proceeds_and_pins"),
            ],
        }
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
        lock_text = "pub struct LockedServer { pub name: String }\npub struct LockedSkill { pub name: String }\n"
        doctor_text = 'fn run_checks() { report.section("Servers"); report.section("Skills"); }\n'
        # Both fixture kinds ARE disclosed on the card, so `:review` adds no
        # finding here — this self-test is about the OTHER requirements, and a
        # new requirement must not silently change what it proves.
        trust_fixture_text = (
            'fn grant_gated() { diff.mark("server", n, i); diff.mark("skill", n, i); }\n'
        )
        enforcement_text = (
            "### Servers\n\n- Servers are pinned.\n- Servers are probed.\n- Servers are reviewed.\n\n"
            "### Skills\n\n- Skills are pinned.\n- Skills are probed.\n- Skills are reviewed.\n\n"
            "### Tools\n\n- Tools are pinned.\n- Tools are probed.\n- Tools are reviewed.\n\n"
            "| **Servers** | enforced |\n"
            "| **Skills** | enforced |\n"
            "| **Tools** | enforced |\n"
        )
        # Baseline deliberately: (1) omits the real skills:witness gap and
        # the real dimension:Egress:enforcement gap (both must surface as
        # "not in baseline" findings), and (2) includes a stale entry for
        # kind:servers:witness, which is NOT a real gap in this fixture.
        baseline = {"kind:servers:witness": "fixture stale entry"}

        findings: list[str] = []
        run_structure_check(
            findings,
            model_text,
            lock_text,
            doctor_text,
            trust_fixture_text,
            enforcement_text,
            root,
            baseline,
            kinds=["servers", "skills"],
            dims=["Tools", "Egress"],
            witness_registry=baseline_registry,
            kind_titles={},
            kind_row_required=frozenset(),
            dim_titles={"Tools": "Tools", "Egress": "Egress"},
        )

        joined = "\n".join(findings)
        if "kind:skills:witness" not in joined or "not in baseline" not in joined:
            failures.append("self-test: kind missing a registered witness, and missing from baseline, NOT caught")
        if "dimension:Egress:enforcement" not in joined or "not in baseline" not in joined:
            failures.append("self-test: policy dimension without an ENFORCEMENT.md row NOT caught")
        if "kind:servers:witness" not in joined or "stale" not in joined:
            failures.append("self-test: stale baseline entry NOT caught")
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
        "self-test: OK (kind-set-integrity mismatch, a forbidden internal crate edge — in "
        "[dependencies], [dev-dependencies], a cfg-gated table, or behind a `package =` rename — "
        "a deleted registered witness, a gutted section body, a missing matrix row, a "
        "missing-from-baseline gap, and a stale baseline entry are all caught; fully-covered "
        "requirements and allowed edges are not)"
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

    for path in (MODEL_RS, LOCK_RS, DOCTOR_RS, TRUST_RS, ENFORCEMENT_MD, ARCHITECTURE_MD):
        if not path.is_file():
            print(f"ERROR: expected source file not found: {path}")
            return 2
    if not CRATES_DIR.is_dir():
        print(f"ERROR: expected crate directory not found: {CRATES_DIR}")
        return 2

    model_text = MODEL_RS.read_text(encoding="utf-8")
    lock_text = LOCK_RS.read_text(encoding="utf-8")
    doctor_text = DOCTOR_RS.read_text(encoding="utf-8")
    trust_text = TRUST_RS.read_text(encoding="utf-8")
    enforcement_text = ENFORCEMENT_MD.read_text(encoding="utf-8")

    try:
        baseline = load_baseline(BASELINE_FILE)
    except ValueError as exc:
        print(f"ERROR: {exc}")
        return 2

    findings: list[str] = []

    # Finding 3: kind-set integrity is a hard, non-baseline-able assertion.
    for err in verify_kind_set(model_text):
        findings.append(f"kind-set integrity: {err}")

    # Crate-edge integrity: likewise hard and non-baseline-able.
    allowed_edges = parse_allowed_edges(ARCHITECTURE_MD.read_text(encoding="utf-8"))
    declared_edges = collect_internal_edges(CRATES_DIR)
    for err in verify_crate_edges(declared_edges, allowed_edges):
        findings.append(f"crate-edge integrity: {err}")

    evidence = run_structure_check(
        findings, model_text, lock_text, doctor_text, trust_text, enforcement_text, REPO_ROOT, baseline
    )

    if "--explain" in argv:
        print_explain(evidence, baseline)
        print_edge_explain(declared_edges, allowed_edges)

    if findings:
        print(f"\ncheck-structure: {len(findings)} finding(s):\n")
        for f in findings:
            print(f"  FAIL {f}")
        return 1

    print(
        f"check-structure: OK ({len(evidence)} requirement(s) checked, "
        f"{sum(len(t) for t in declared_edges.values())} internal crate edge(s) all allowed by "
        f"ARCHITECTURE.md, {len(baseline)} baselined gap(s), all others satisfied)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
