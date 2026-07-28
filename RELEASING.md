# Releasing agentstack

Releases are published from tags (v0.2.0 onward). Per release:

## 0. Before tagging

- CI (`.github/workflows/ci.yml`) must be green on `main` — fmt + clippy +
  tests + the asserted example suite + the Docker sandbox job.
- The README on `main` documents whatever binary the installer serves as
  `latest`, so tag the commit whose README matches the surface you are
  shipping — a version bump left untagged means the installer hands users a
  binary that no longer matches the docs.
- Update `CHANGELOG.md` with the release entry.
- Sweep doc version pins to the new tag — they lag silently otherwise:
  `rg -n "@v0\.|agentstack 0\." README.md docs/*.html docs/start.html action.yml`
  (the Action `uses:` examples and start.html's `--version` capture are the
  known offenders).
- Stamp `action.yml`'s pinned binary default to the new tag: the `version`
  input's `default: vX.Y.Z` line (under `inputs.version`) must equal this
  release's tag, so `uses: Tarekkharsa/agentstack@vX.Y.Z` installs the binary
  that shipped with it rather than a future one. The `@v` sweep above catches
  the `uses:` example comment but not this `default:`, so bump it explicitly.
  `docs_commands::action_default_binary_matches_this_release` fails CI when
  this stamp and the CLI crate version differ.

## 1. Release binaries (GitHub Releases)

The tag **must** be `v<version>` where `<version>` is the cli crate's
`version` in `crates/cli/Cargo.toml` — the binary's compiled-in default
egress-image tag is derived from it, and the `egress-image` job fails the
release on a mismatch. Bump the crate version first, then:

```sh
git tag "v$(grep -m1 '^version' crates/cli/Cargo.toml | cut -d'"' -f2)"
git push --tags
```

`.github/workflows/release.yml` builds for macOS (arm64/x64), Linux (arm64/x64),
and Windows (x64), with the `sandbox` feature enabled on every target. It
attaches `.tar.gz` / `.zip` assets to a **draft** release and records build
provenance attestations for them. The workflow verifies the draft by its exact
release ID and seven-asset count; a green run therefore means the draft still
exists. Review the draft, then publish it.

If the tag exists but its draft was lost, rebuild that exact tag through the
manual recovery input. This still creates a draft and never publishes:

```sh
gh workflow run release.yml --ref main -f release_tag=v0.16.0
gh run list --workflow release.yml --limit 1
gh run watch <run-id> --exit-status
```

Do not move or recreate the tag. The dispatch checks out the existing tag,
requires it to match the CLI crate version, rebuilds and re-attests all five
platform archives, then recreates the formula and draft through the same path
as an ordinary tag push.

After downloading an asset, verify that its provenance is tied to this
repository and GitHub Actions workflow:

```sh
gh attestation verify agentstack-<target>.tar.gz --repo Tarekkharsa/agentstack
```

The attestation establishes where the artifact was built; continue to compare
its SHA-256 digest with `checksums.txt` when validating a download.

## 2. curl installer

Once a release is published, this works:

```sh
curl -fsSL https://raw.githubusercontent.com/Tarekkharsa/agentstack/main/install.sh | sh
```

It detects OS/arch, downloads the matching `latest` asset, and installs the
binary to `/usr/local/bin` (or `~/.local/bin`).

## 3. Homebrew

**The formula is generated for you.** `release.yml` renders it from the
release's own `checksums.txt` and attaches `agentstack.rb` to the release
alongside the binaries, so its hashes are always the bytes that release
published. There is no checked-in formula to update — only
`packaging/homebrew/agentstack.rb.tmpl` and `render-formula.py`, and the
renderer refuses to write anything if a target's checksum is missing.

Publishing is the one manual step, because it writes to a different repository:

```sh
TAG="v$(grep -m1 '^version' crates/cli/Cargo.toml | cut -d'"' -f2)"

# 1. Fetch the formula this release generated.
gh release download "$TAG" --repo Tarekkharsa/agentstack --pattern agentstack.rb --clobber

# 2. Commit it to the tap. Create the repo on first use — it must be named
#    `homebrew-<tap>` for `brew install <owner>/<tap>/<formula>` to resolve:
#    gh repo create Tarekkharsa/homebrew-tap --public
git -C ../homebrew-tap pull
mkdir -p ../homebrew-tap/Formula && cp agentstack.rb ../homebrew-tap/Formula/
git -C ../homebrew-tap add Formula/agentstack.rb
git -C ../homebrew-tap commit -m "agentstack $TAG"
git -C ../homebrew-tap push
```

Then verify from a clean machine (or `brew uninstall` first):

```sh
brew install Tarekkharsa/tap/agentstack
agentstack --version   # must match $TAG
```

Homebrew-installed binaries are managed by brew: `agentstack self update`
detects this and points at `brew upgrade` rather than replacing the file
underneath the package manager.

## 4. Container images (sandbox / lockdown)

The tag also builds and pushes the **egress-proxy sidecar** image `--lockdown`
needs, to `ghcr.io/<owner>/agentstack-egress-proxy:{tag,latest}` (the
`egress-image` job in `release.yml` — GHCR, built-in token, no secrets). The
job attests the pushed image and appends its immutable
`ghcr.io/<owner>/agentstack-egress-proxy@sha256:...` reference to the draft
release notes.

Verify that image provenance against the immutable reference from the release:

```sh
gh attestation verify \
  oci://ghcr.io/tarekkharsa/agentstack-egress-proxy@sha256:<digest> \
  --repo Tarekkharsa/agentstack
```

Lockdown is **zero-config**: the binary's compiled-in default is exactly
`ghcr.io/tarekkharsa/agentstack-egress-proxy:v<its own version>`, and the
runtime pulls it on first use if it isn't present locally. The pin means a
binary never silently picks up a newer enforcement sidecar; `latest` exists
only for humans browsing the registry. `AGENTSTACK_EGRESS_IMAGE` overrides the
default (e.g. a locally built `docker/egress-proxy.Dockerfile` tag) — a
present local image is never re-pulled.

**One-time, after the first release:** GHCR packages are *private* by default.
Make `agentstack-egress-proxy` public (package settings → Danger Zone →
Change visibility), or anonymous pulls — i.e. every lockdown user — fail.

The **sandbox runner** image (the harness cage) is *not* published: it must carry
your chosen harness. Users build it from
[`docker/sandbox.Dockerfile`](docker/sandbox.Dockerfile) and set
`AGENTSTACK_SANDBOX_IMAGE`.

## Release credential compromise and revocation

If a release credential or GitHub Actions publishing path may be compromised,
stop publishing, revoke or rotate the affected credential, disable the affected
workflow, and mark suspect releases and image tags as untrusted. Remove suspect
artifacts/tags where practical, publish a security notice identifying the exact
versions and immutable digests involved, and rebuild replacements from a known
good commit only after the publishing path has been reviewed. Attestations and
checksums help identify what was built and distributed; they do not make a
compromised publisher trustworthy or revoke copies already downloaded.

## 5. crates.io (optional)

```sh
cargo publish --dry-run   # verify the package
cargo publish
```

`Cargo.toml` already has description/license/keywords/categories and an `exclude`
list so the crate stays lean.
