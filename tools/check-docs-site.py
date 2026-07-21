#!/usr/bin/env python3
"""CI checker over the deployable docs/ tree.

Validates, across the static site that GitHub Pages ships:

  a) Internal links & fragments in every docs/**/*.html
  b) Internal links & fragments in docs/*.md and docs/howto/*.md
  c) sitemap.xml consistency (both directions) and no redirect-stub entries
  d) a --self-test that proves the checker itself catches breakage

Python 3 standard library only. Exits nonzero with a per-finding listing on
any failure.
"""

from __future__ import annotations

import html
import re
import sys
import tempfile
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import unquote, urldefrag

REPO_ROOT = Path(__file__).resolve().parent.parent
DOCS = REPO_ROOT / "docs"

SITEMAP_PREFIX = "https://tarekkharsa.github.io/agentstack/"

# Directories whose HTML pages are allowed to be absent from the sitemap.
SITEMAP_EXEMPT_DIRS = ("design", "spikes", "demos", "theme")

# docs/design/ holds design scratch, ADRs, and Figma dev-mode (<x-dc>) export
# mockups that reference never-committed Figma export scaffolding (support.js,
# "*.dc.html"). Those pages are not part of the deployable navigable site — not
# in the sitemap, not linked from any real page — so the link crawl skips them.
# Every real site page (index/start/docs/howto/reference/…) is still crawled.
CRAWL_EXEMPT_DIRS = ("design",)


# --------------------------------------------------------------------------
# HTML parsing helpers
# --------------------------------------------------------------------------
class _LinkAndIdParser(HTMLParser):
    """Collect referencing attributes and anchor targets from an HTML file.

    - links: (attr, url) pairs for a[href], img[src], link[href], script[src]
    - ids:   set of every id="" and name="" value (fragment targets)
    """

    # Which attribute carries the reference for each element we care about.
    _REF_ATTR = {"a": "href", "img": "src", "link": "href", "script": "src"}

    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.links: list[tuple[str, str]] = []
        self.ids: set[str] = set()

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        d = {k: (v or "") for k, v in attrs}
        ref_attr = self._REF_ATTR.get(tag)
        if ref_attr and ref_attr in d and d[ref_attr]:
            self.links.append((f"{tag}[{ref_attr}]", d[ref_attr]))
        # Any element can be a fragment target via id= or name=.
        if "id" in d and d["id"]:
            self.ids.add(d["id"])
        if "name" in d and d["name"]:
            self.ids.add(d["name"])


def _parse_html(path: Path) -> _LinkAndIdParser:
    p = _LinkAndIdParser()
    p.feed(path.read_text(encoding="utf-8", errors="replace"))
    return p


# Cache of parsed HTML so target-fragment lookups don't re-read files.
_html_cache: dict[Path, _LinkAndIdParser] = {}


def _html_info(path: Path) -> _LinkAndIdParser:
    if path not in _html_cache:
        _html_cache[path] = _parse_html(path)
    return _html_cache[path]


def _is_external(url: str) -> bool:
    lo = url.strip().lower()
    return (
        lo.startswith("http://")
        or lo.startswith("https://")
        or lo.startswith("mailto:")
        or lo.startswith("data:")
        or lo.startswith("//")
        or lo.startswith("tel:")
        or lo.startswith("javascript:")
    )


def _resolve_target(containing: Path, url: str) -> Path:
    """Resolve a relative href/src to a filesystem path.

    "dir/" -> dir/index.html. Query strings are dropped.
    """
    # Drop any query string; fragment is handled by the caller.
    path_part = url.split("?", 1)[0]
    path_part = unquote(path_part)
    if path_part == "":
        return containing
    target = (containing.parent / path_part).resolve()
    if path_part.endswith("/"):
        target = target / "index.html"
    return target


def _is_redirect_stub(path: Path) -> bool:
    if not path.is_file() or path.suffix.lower() != ".html":
        return False
    text = path.read_text(encoding="utf-8", errors="replace")
    return 'http-equiv="refresh"' in text.lower().replace("'", '"')


# --------------------------------------------------------------------------
# (a) HTML internal link + fragment crawl
# --------------------------------------------------------------------------
def check_html_links(findings: list[str]) -> None:
    for htmlfile in sorted(DOCS.rglob("*.html")):
        relparts = htmlfile.relative_to(DOCS).parts
        if relparts and relparts[0] in CRAWL_EXEMPT_DIRS:
            continue
        info = _html_info(htmlfile)
        rel = _rel(htmlfile)
        for where, url in info.links:
            if _is_external(url):
                continue
            base, frag = urldefrag(url)
            # A bare "#frag" (or "#") targets the same file.
            if base == "":
                target = htmlfile
            else:
                target = _resolve_target(htmlfile, base)
                if not target.exists():
                    findings.append(f"{rel}: {where} -> {url} : target file not found ({_rel(target)})")
                    continue
            if frag:
                _check_fragment(target, frag, url, where, rel, findings)


def _rel(p: Path) -> Path | str:
    """Display path relative to the docs tree's parent (works for the
    tempdir tree the self-test builds too, where REPO_ROOT does not apply)."""
    for base in (DOCS.parent, REPO_ROOT):
        try:
            return p.relative_to(base)
        except ValueError:
            continue
    return p


def _under_repo(p: Path) -> bool:
    try:
        p.relative_to(DOCS.parent)
        return True
    except ValueError:
        return False


def _check_fragment(
    target: Path,
    frag: str,
    url: str,
    where: str,
    rel: Path,
    findings: list[str],
) -> None:
    frag = unquote(frag)
    if target.suffix.lower() == ".md":
        # Skip fragment checks into markdown targets (per spec).
        return
    if target.suffix.lower() != ".html" or not target.is_file():
        return
    ids = _html_info(target).ids
    if frag not in ids:
        findings.append(f"{rel}: {where} -> {url} : fragment #{frag} not found in {target.name}")


# --------------------------------------------------------------------------
# (b) Markdown internal links
# --------------------------------------------------------------------------
_MD_LINK_RE = re.compile(r"\[[^\]]*\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")
_MD_HEADING_RE = re.compile(r"^(#{1,6})\s+(.*?)\s*#*\s*$")
_MD_HTML_ID_RE = re.compile(r'<[a-zA-Z][^>]*\bid="([^"]+)"')


def _github_slug(heading: str) -> str:
    # Strip inline markdown/HTML, lowercase, spaces->-, drop punctuation
    # except hyphens, GitHub-style.
    text = re.sub(r"`([^`]*)`", r"\1", heading)
    # NB: do NOT strip <...> spans — GitHub keeps the inner text of things
    # like `<source>` in a code span (dropping only the angle brackets as
    # punctuation below), e.g. "add skill <source>" -> "...add-skill-source...".
    text = re.sub(r"\[([^\]]*)\]\([^)]*\)", r"\1", text)  # link -> text
    text = html.unescape(text)
    text = text.strip().lower()
    text = text.replace(" ", "-")
    # keep word chars and hyphens only
    text = re.sub(r"[^\w-]", "", text, flags=re.UNICODE)
    return text


def _md_anchors(path: Path) -> set[str]:
    anchors: set[str] = set()
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        m = _MD_HEADING_RE.match(line)
        if m:
            anchors.add(_github_slug(m.group(2)))
        for hid in _MD_HTML_ID_RE.findall(line):
            anchors.add(hid)
    return anchors


_md_anchor_cache: dict[Path, set[str]] = {}


def _md_anchors_cached(path: Path) -> set[str]:
    if path not in _md_anchor_cache:
        _md_anchor_cache[path] = _md_anchors(path)
    return _md_anchor_cache[path]


def check_md_links(findings: list[str]) -> None:
    md_files = sorted(DOCS.glob("*.md")) + sorted((DOCS / "howto").glob("*.md"))
    for mdfile in md_files:
        rel = _rel(mdfile)
        text = mdfile.read_text(encoding="utf-8", errors="replace")
        for m in _MD_LINK_RE.finditer(text):
            url = m.group(1)
            if _is_external(url) or url.startswith("#") is False and url.strip() == "":
                continue
            base, frag = urldefrag(url)
            if base.startswith("#"):
                base, frag = "", base.lstrip("#")
            if base == "":
                target = mdfile
            else:
                if _is_external(base):
                    continue
                # Markdown is "file existence only" per spec: a link to
                # "dir/" is satisfied by the directory itself — do NOT force
                # index.html here (that rule is HTML/sitemap-only).
                target = (mdfile.parent / unquote(base.split("?", 1)[0])).resolve()
                if not target.exists():
                    findings.append(f"{rel}: link -> {url} : target file not found")
                    continue
            if frag:
                if target.suffix.lower() == ".md":
                    if frag not in _md_anchors_cached(target):
                        findings.append(f"{rel}: link -> {url} : heading #{frag} not found in {target.name}")
                elif target.suffix.lower() == ".html" and target.is_file():
                    if frag not in _html_info(target).ids:
                        findings.append(f"{rel}: link -> {url} : fragment #{frag} not found in {target.name}")


# --------------------------------------------------------------------------
# (c) sitemap consistency
# --------------------------------------------------------------------------
_LOC_RE = re.compile(r"<loc>\s*([^<]+?)\s*</loc>")


def _loc_to_path(loc: str) -> Path | None:
    if not loc.startswith(SITEMAP_PREFIX):
        return None
    rest = loc[len(SITEMAP_PREFIX):]
    if rest == "" or rest.endswith("/"):
        rest = rest + "index.html"
    return DOCS / rest


def _canonical_pages() -> set[Path]:
    """Every docs/**/*.html that is NOT a redirect stub, minus exempt dirs.

    tutorial/index.html is represented by the "tutorial/" entry, so it is
    kept in the canonical set (it maps to the same file the sitemap entry
    resolves to).
    """
    out: set[Path] = set()
    for htmlfile in DOCS.rglob("*.html"):
        relparts = htmlfile.relative_to(DOCS).parts
        if relparts[0] in SITEMAP_EXEMPT_DIRS:
            continue
        if _is_redirect_stub(htmlfile):
            continue
        out.add(htmlfile.resolve())
    return out


def check_sitemap(findings: list[str]) -> None:
    sitemap = DOCS / "sitemap.xml"
    if not sitemap.is_file():
        findings.append("sitemap.xml: file not found")
        return
    text = sitemap.read_text(encoding="utf-8", errors="replace")
    locs = _LOC_RE.findall(text)

    seen: set[str] = set()
    mapped: set[Path] = set()
    for loc in locs:
        if loc in seen:
            findings.append(f"sitemap.xml: duplicate <loc> {loc}")
            continue
        seen.add(loc)
        target = _loc_to_path(loc)
        if target is None:
            findings.append(f"sitemap.xml: <loc> {loc} : does not start with {SITEMAP_PREFIX}")
            continue
        if not target.is_file():
            findings.append(f"sitemap.xml: <loc> {loc} : maps to missing file {_rel(target)}")
            continue
        if _is_redirect_stub(target):
            findings.append(f"sitemap.xml: <loc> {loc} : maps to a redirect stub ({target.name})")
            continue
        mapped.add(target.resolve())

    # Inverse: every canonical page must appear.
    for page in sorted(_canonical_pages()):
        if page not in mapped:
            findings.append(f"sitemap.xml: canonical page missing from sitemap: {_rel(page)}")


# --------------------------------------------------------------------------
# (d) self-test
# --------------------------------------------------------------------------
def self_test() -> int:
    """Build a tiny broken tree and confirm each defect is caught."""
    global DOCS
    saved_docs = DOCS
    failures: list[str] = []
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        d = root / "docs"
        (d / "howto").mkdir(parents=True)
        DOCS = d
        _html_cache.clear()
        _md_anchor_cache.clear()
        _md_cache_clear()

        # index.html: one broken file link, one broken fragment (into good.html).
        (d / "index.html").write_text(
            '<!doctype html><html><head><title>i</title></head><body>'
            '<a href="good.html">ok</a>'
            '<a href="missing-page.html">broken file link</a>'
            '<a href="good.html#nope">broken fragment</a>'
            '<a href="good.html#real">good fragment</a>'
            '</body></html>',
            encoding="utf-8",
        )
        (d / "good.html").write_text(
            '<!doctype html><html><head><title>g</title></head>'
            '<body><h2 id="real">Real</h2></body></html>',
            encoding="utf-8",
        )
        # A redirect stub that the sitemap wrongly references.
        (d / "old.html").write_text(
            '<!doctype html><meta http-equiv="refresh" content="0; url=good.html">',
            encoding="utf-8",
        )
        (d / "sitemap.xml").write_text(
            '<?xml version="1.0"?><urlset>'
            f"<url><loc>{SITEMAP_PREFIX}</loc></url>"
            f"<url><loc>{SITEMAP_PREFIX}good.html</loc></url>"
            f"<url><loc>{SITEMAP_PREFIX}old.html</loc></url>"
            "</urlset>",
            encoding="utf-8",
        )

        html_findings: list[str] = []
        check_html_links(html_findings)
        sitemap_findings: list[str] = []
        check_sitemap(sitemap_findings)

        joined = "\n".join(html_findings)
        if "missing-page.html" not in joined:
            failures.append("self-test: broken file link NOT caught")
        if "#nope" not in joined:
            failures.append("self-test: broken fragment NOT caught")
        sm = "\n".join(sitemap_findings)
        if "redirect stub" not in sm:
            failures.append("self-test: sitemap redirect entry NOT caught")

    DOCS = saved_docs
    _html_cache.clear()
    _md_anchor_cache.clear()
    _md_cache_clear()

    if failures:
        print("SELF-TEST FAILED:")
        for f in failures:
            print(f"  - {f}")
        return 1
    print("self-test: OK (broken link, broken fragment, and redirect-stub sitemap entry all caught)")
    return 0


def _md_cache_clear() -> None:
    _md_anchor_cache.clear()


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

    if not DOCS.is_dir():
        print(f"ERROR: docs directory not found at {DOCS}")
        return 2

    findings: list[str] = []
    check_html_links(findings)
    check_md_links(findings)
    check_sitemap(findings)

    if findings:
        print(f"\ncheck-docs-site: {len(findings)} finding(s):\n")
        for f in findings:
            print(f"  FAIL {f}")
        return 1

    print("check-docs-site: OK (links, fragments, and sitemap all consistent)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
