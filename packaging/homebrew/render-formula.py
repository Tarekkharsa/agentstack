#!/usr/bin/env python3
"""Render the Homebrew formula for one release from that release's own checksums.

    render-formula.py --version 0.16.0 --checksums assets/checksums.txt \
                      [--template packaging/homebrew/agentstack.rb.tmpl] \
                      [--out assets/agentstack.rb]

`release.yml` runs this right after it generates `checksums.txt`, so the formula
a release publishes is derived from the very bytes that release uploaded — never
from a hash someone pasted by hand. Publishing the rendered file to the tap
repository stays a human step (RELEASING.md).

Fails loudly and writes nothing when any expected target is missing from the
checksum file, because a formula with a stale or absent hash is exactly the
failure this replaces: a file that looks publishable and installs the wrong
bytes.
"""

import argparse
import re
import sys
from pathlib import Path

# The four targets Homebrew serves. `release.yml` also builds
# x86_64-pc-windows-msvc; Homebrew has no use for it, so it is not an error for
# it to be absent here.
TARGETS = [
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "aarch64-unknown-linux-gnu",
    "x86_64-unknown-linux-gnu",
]

SHA256_RE = re.compile(r"^[0-9a-f]{64}$")


def parse_checksums(text: str) -> dict[str, str]:
    """Map target -> sha256, from `sha256sum` output.

    Each line is `<64 hex>  <filename>`, and filenames look like
    `agentstack-<target>.tar.gz`. Anything that does not match that shape is
    skipped rather than guessed at.
    """
    found: dict[str, str] = {}
    for line in text.splitlines():
        parts = line.split()
        if len(parts) != 2:
            continue
        digest, name = parts[0], parts[1].lstrip("*")
        if not SHA256_RE.match(digest):
            continue
        for target in TARGETS:
            if name == f"agentstack-{target}.tar.gz":
                found[target] = digest
    return found


def render(template: str, version: str, sums: dict[str, str]) -> str:
    out = template.replace("__VERSION__", version)
    for target, digest in sums.items():
        out = out.replace(f"__SHA256_{target.upper().replace('-', '_')}__", digest)
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--version", required=True, help="release version, without a leading v")
    ap.add_argument("--checksums", required=True, type=Path)
    ap.add_argument(
        "--template",
        type=Path,
        default=Path(__file__).with_name("agentstack.rb.tmpl"),
    )
    ap.add_argument("--out", type=Path, required=True)
    args = ap.parse_args()

    version = args.version.lstrip("v")
    sums = parse_checksums(args.checksums.read_text())

    missing = [t for t in TARGETS if t not in sums]
    if missing:
        print(
            "render-formula: refusing to write a formula with missing checksums for:\n  "
            + "\n  ".join(missing)
            + f"\n(read {args.checksums})",
            file=sys.stderr,
        )
        return 1

    rendered = render(args.template.read_text(), version, sums)
    if "__" in rendered and re.search(r"__[A-Z0-9_]+__", rendered):
        leftover = sorted(set(re.findall(r"__[A-Z0-9_]+__", rendered)))
        print(
            "render-formula: template placeholders left unfilled: " + ", ".join(leftover),
            file=sys.stderr,
        )
        return 1

    args.out.write_text(rendered)
    print(f"render-formula: wrote {args.out} for v{version} ({len(sums)} targets)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
