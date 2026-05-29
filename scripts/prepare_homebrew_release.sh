#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "Usage: $(basename "$0") <version> [remote]" >&2
  echo "Example: $(basename "$0") 0.3.2 origin" >&2
  exit 1
}

version="${1:-}"
remote="${2:-origin}"

if [[ -z "$version" ]]; then
  usage
fi

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
formula_path="$root_dir/Formula/axon.rb"
tag="v$version"
archive_url="https://github.com/flavioaiello/axon/archive/refs/tags/$tag.tar.gz"

if [[ ! -f "$formula_path" ]]; then
  echo "Formula not found: $formula_path" >&2
  exit 1
fi

cargo_version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$root_dir/Cargo.toml" | head -n 1)"
if [[ "$cargo_version" != "$version" ]]; then
  echo "Cargo.toml version is '$cargo_version', expected '$version'" >&2
  exit 1
fi

if ! git -C "$root_dir" ls-remote --exit-code --tags "$remote" "refs/tags/$tag" >/dev/null; then
  echo "Remote tag '$tag' was not found on '$remote'." >&2
  echo "Push the release commit and tag first, then rerun this script." >&2
  exit 1
fi

archive_path="$(mktemp "/tmp/axon-${version}.XXXXXX.tar.gz")"
trap 'rm -f "$archive_path"' EXIT

curl -fsSL "$archive_url" -o "$archive_path"
sha256="$(shasum -a 256 "$archive_path" | awk '{print $1}')"

perl -0pi -e '
  s{url ".*?/archive/refs/tags/v[^"]+\.tar\.gz"}{url "'"$archive_url"'"};
  s{sha256 "[0-9a-f]+"}{sha256 "'"$sha256"'"};
  s{version "[^"]+"}{version "'"$version"'"};
' "$formula_path"

echo "Updated $formula_path"
echo "url: $archive_url"
echo "sha256: $sha256"