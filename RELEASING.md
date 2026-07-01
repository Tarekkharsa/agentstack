# Releasing agentstack

Distribution is set up but **not yet published**. To cut the first release:

## 0. One-time

- Push to GitHub and set the real repo slug everywhere it says `Tarek-kharsa/agentstack`
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
curl -fsSL https://raw.githubusercontent.com/Tarek-kharsa/agentstack/main/install.sh | sh
```

It detects OS/arch, downloads the matching `latest` asset, and installs the
binary to `/usr/local/bin` (or `~/.local/bin`).

## 3. Homebrew

```sh
# compute the per-arch checksums from the published assets:
for t in aarch64-apple-darwin x86_64-apple-darwin \
         aarch64-unknown-linux-gnu x86_64-unknown-linux-gnu; do
  curl -fsSL "https://github.com/Tarek-kharsa/agentstack/releases/download/v0.1.0/agentstack-$t.tar.gz" | shasum -a 256
done
```

Fill the `sha256` fields in `packaging/homebrew/agentstack.rb`, commit it to a tap
repo `Tarek-kharsa/homebrew-tap`, then:

```sh
brew install tarek-kharsa/tap/agentstack
```

## 4. crates.io (optional)

```sh
cargo publish --dry-run   # verify the package
cargo publish
```

`Cargo.toml` already has description/license/keywords/categories and an `exclude`
list so the crate stays lean.
