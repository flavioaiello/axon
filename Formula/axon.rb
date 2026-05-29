class Axon < Formula
  desc "Domain Model Context Protocol Server — architectural meta-layer for GitHub Copilot"
  homepage "https://github.com/flavioaiello/axon"
  license "MIT"
  url "https://github.com/flavioaiello/axon/archive/refs/tags/v0.4.0.tar.gz"
  sha256 "a4970f17217d993e4c7fcc19e957f39bf3d0d6202a4c555affeabcc375b7b243"
  version "0.4.0"

  head "https://github.com/flavioaiello/axon.git", branch: "main"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args
    # Binary is named axon
  end

  def post_install
    # Ensure the data directory exists
    (var/"axon").mkpath
  end

  def caveats
    <<~EOS
      Axon stores domain models per-crate in <crate_root>/.axon/store.db (SQLite).

      To use with VS Code / GitHub Copilot, add to .vscode/mcp.json:

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

      To list all stored projects:

        axon list
    EOS
  end

  test do
    # Verify the binary starts and prints usage
    output = shell_output("#{bin}/axon 2>&1", 1)
    assert_match "axon", output
  end
end
