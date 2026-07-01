# Homebrew formula template for agentstack.
#
# After a release, fill in VERSION and the per-arch sha256 (from the release
# `.tar.gz` assets), then publish to a tap repo (e.g. Tarek-kharsa/homebrew-tap):
#   brew install tarek-kharsa/tap/agentstack
class Agentstack < Formula
  desc "One portable manifest, every agent CLI — manage MCP servers + skills across AI coding tools"
  homepage "https://github.com/Tarek-kharsa/agentstack"
  version "0.1.0"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    on_arm do
      url "https://github.com/Tarek-kharsa/agentstack/releases/download/v#{version}/agentstack-aarch64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_SHA256"
    end
    on_intel do
      url "https://github.com/Tarek-kharsa/agentstack/releases/download/v#{version}/agentstack-x86_64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_SHA256"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/Tarek-kharsa/agentstack/releases/download/v#{version}/agentstack-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "REPLACE_WITH_SHA256"
    end
    on_intel do
      url "https://github.com/Tarek-kharsa/agentstack/releases/download/v#{version}/agentstack-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "REPLACE_WITH_SHA256"
    end
  end

  def install
    bin.install Dir["agentstack-*/agentstack"].first => "agentstack"
  end

  test do
    assert_match "agentstack", shell_output("#{bin}/agentstack --help")
  end
end
