class Axon < Formula
  desc "Domain Model Context Protocol Server — architectural meta-layer for GitHub Copilot"
  homepage "https://github.com/flavioaiello/axon"
  license "MIT"
  url "https://github.com/flavioaiello/axon/archive/refs/tags/v0.4.1.tar.gz"
  sha256 "5ee1c25c7fdafb323e6949ece2e99b2409881eaf830386b96cf4e3b165da9eb9"
  version "0.4.1"

  head "https://github.com/flavioaiello/axon.git", branch: "main"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args
    # Binary is named axon
  end

  def post_install
    # Ensure the data and log directories exist
    (var/"axon").mkpath
    (var/"log").mkpath
  end

  service do
    run [opt_bin/"axon", "daemon"]
    keep_alive true
    log_path var/"log/axon.log"
    error_log_path var/"log/axon.log"
  end

  def caveats
    <<~EOS
      Axon keeps domain models in memory. For one warm, shared brain across every
      VS Code / VSCodium window, run the daemon:

        brew services start axon

      It listens on ~/.axon/daemon.sock and holds each workspace's model
      separately in memory. Every editor still launches `axon serve` (stdio),
      which transparently bridges to the daemon — and falls back to a standalone
      in-process server if the daemon isn't running. So .mcp.json is unchanged:

        {
          "servers": {
            "axon": {
              "type": "stdio",
              "command": "axon",
              "args": ["serve", "--workspace", "${workspaceFolder}"]
            }
          }
        }

      To export the actual model:

        axon export model.json --workspace /path/to/project --state actual

      To list all crates in a workspace:

        axon list --workspace /path/to/project
    EOS
  end

  test do
    # Verify the binary starts and prints usage
    output = shell_output("#{bin}/axon 2>&1", 1)
    assert_match "axon", output
  end
end
