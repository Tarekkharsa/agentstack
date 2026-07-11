# Releasing agentstack

Distribution is set up but **not yet published**. To cut the first release:

## 0. One-time

- Push to GitHub and set the real repo slug everywhere it says `Tarekkharsa/agentstack`
  (`Cargo.toml`, `install.sh`, `packaging/homebrew/agentstack.rb`, this file).
- CI (`.github/workflows/ci.yml`) runs fmt + clippy + tests on every push/PR.

## 1. Release binaries (GitHub Releases)

```sh
git tag v0.1.0
git push --tags
```

`.github/workflows/release.yml` builds for macOS (arm64/x64), Linux (arm64/x64),
and Windows (x64), and attaches `.tar.gz` / `.zip` assets to a **draft** release.
Review the draft, then publish it.

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
for t in aarch64-apple-darwin x86_64-apple-darwin \
         aarch64-unknown-linux-gnu x86_64-unknown-linux-gnu; do
  curl -fsSL "https://github.com/Tarekkharsa/agentstack/releases/download/v0.1.0/agentstack-$t.tar.gz" | shasum -a 256
done
```

Fill the `sha256` fields in `packaging/homebrew/agentstack.rb`, commit it to a tap
repo `tarekkh/homebrew-tap`, then:

```sh
brew install Tarekkharsa/tap/agentstack
```

## 4. Container images (sandbox / lockdown)

The tag also builds and pushes the **egress-proxy sidecar** image `--lockdown`
needs, to `ghcr.io/<owner>/agentstack-egress-proxy:{tag,latest}` (see the
`egress-image` job in `release.yml`, GHCR, no secrets). After a release, lockdown
users pull it and set `AGENTSTACK_EGRESS_IMAGE` to that tag.

**One decision before v1:** the code's default egress tag is
`agentstack/egress-proxy:latest` (Docker Hub). Either (a) keep GHCR and tell
users to set `AGENTSTACK_EGRESS_IMAGE`, or (b) publish to Docker Hub
`agentstack/egress-proxy` so lockdown works with no env var — that needs the
`agentstack` org + `DOCKERHUB_*` secrets; swap the registry in the `egress-image`
job.

The **sandbox runner** image (the harness cage) is *not* published: it must carry
your chosen harness. Users build it from
[`docker/sandbox.Dockerfile`](docker/sandbox.Dockerfile) and set
`AGENTSTACK_SANDBOX_IMAGE`.

## 5. crates.io (optional)

```sh
cargo publish --dry-run   # verify the package
cargo publish
```

`Cargo.toml` already has description/license/keywords/categories and an `exclude`
list so the crate stays lean.
