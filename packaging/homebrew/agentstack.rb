# Homebrew formula template for agentstack.
#
# After a release, fill in VERSION and the per-arch sha256 (from the release
# `.tar.gz` assets), then publish to a tap repo (e.g. tarekkh/homebrew-tap):
#   brew install Tarekkharsa/tap/agentstack
class Agentstack < Formula
  desc "One portable manifest, every agent CLI — manage MCP servers + skills across AI coding tools"
  homepage "https://github.com/Tarekkharsa/agentstack"
  version "0.10.1"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    on_arm do
      url "https://github.com/Tarekkharsa/agentstack/releases/download/v#{version}/agentstack-aarch64-apple-darwin.tar.gz"
      sha256 "992310c9d3bd1cd3df14abe4db3ebb0d48d1ca80951c5ae2183ea5f51a446d4a"
    end
    on_intel do
      url "https://github.com/Tarekkharsa/agentstack/releases/download/v#{version}/agentstack-x86_64-apple-darwin.tar.gz"
      sha256 "3c101ee4920b44de2fb9f75b3efb2c4733a53818268e93b766d5e5e49074b97b"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/Tarekkharsa/agentstack/releases/download/v#{version}/agentstack-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "4124e96d4dcf6e1e23c9e496dfede421a57e2de6ad73a31d7d7625c534f7170d"
    end
    on_intel do
      url "https://github.com/Tarekkharsa/agentstack/releases/download/v#{version}/agentstack-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "0e75fc3005068d91c74dfa88362fb58d6193c4dc49c2775981e1858bb364d0e3"
    end
  end

  def install
    bin.install Dir["agentstack-*/agentstack"].first => "agentstack"
  end

  test do
    assert_match "agentstack", shell_output("#{bin}/agentstack --help")
  end
end
