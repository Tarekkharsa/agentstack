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
provenance attestations for them. Review the draft, then publish it.

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

```sh
# compute the per-arch checksums from the published assets:
TAG="v$(grep -m1 '^version' crates/cli/Cargo.toml | cut -d'"' -f2)"
for t in aarch64-apple-darwin x86_64-apple-darwin \
         aarch64-unknown-linux-gnu x86_64-unknown-linux-gnu; do
  curl -fsSL "https://github.com/Tarekkharsa/agentstack/releases/download/$TAG/agentstack-$t.tar.gz" | shasum -a 256
done
```

Update `version` and the `sha256` fields in
`packaging/homebrew/agentstack.rb` (the checked-in file is a template whose
values are from the version named in its `version` line — regenerate per
release), commit it to a tap repo `tarekkh/homebrew-tap` (create it on first
use), then:

```sh
brew install Tarekkharsa/tap/agentstack
```

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
