#!/bin/sh
# Glama's build of navigera's MCP server: Glama runs this as a build step on
# its Debian image, as root. Nothing else runs it, and no release includes it
# (CONTRIBUTING.md, "Glama").
#
#   sh packaging/glama/build.sh [<version>]
#
# <version> defaults to the one in this checkout's Cargo.toml: the release
# being built.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
version=${1:-$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$here/../../Cargo.toml" | head -n 1)}
if [ -z "$version" ]; then
  echo "glama build: no navigera version given, and none in Cargo.toml" >&2
  exit 1
fi

# The libraries chrome-headless-shell links against; Glama's base image has
# none of them. Newer Debian names some with a t64 suffix: take that one when
# the release has it.
export DEBIAN_FRONTEND=noninteractive
apt-get update
pkgs=""
for p in libglib2.0-0 libnss3 libnspr4 libatk1.0-0 libatk-bridge2.0-0 libatspi2.0-0 \
  libcups2 libdbus-1-3 libdrm2 libgbm1 libxkbcommon0 libxcomposite1 libxdamage1 \
  libxfixes3 libxrandr2 libxext6 libx11-6 libxcb1 libcairo2 libpango-1.0-0 libasound2; do
  if apt-cache show "${p}t64" >/dev/null 2>&1; then pkgs="$pkgs ${p}t64"; else pkgs="$pkgs $p"; fi
done
# shellcheck disable=SC2086 # one word per package
apt-get install -y --no-install-recommends ca-certificates $pkgs
rm -rf /var/lib/apt/lists/*

# navigera from PyPI at that version; PyPI can trail the GitHub release by a
# few minutes.
tries=0
until uv tool install "navigera==$version"; do
  tries=$((tries + 1))
  if [ "$tries" -ge 10 ]; then
    echo "glama build: navigera $version is not on PyPI" >&2
    exit 1
  fi
  sleep 30
done
nav="$HOME/.local/bin/navigera"
"$nav" --version

# The browser, and proof it loads: a missing library fails the build here
# rather than the first call a user makes.
"$nav" install-browser chrome-headless-shell
shell=$(find "$("$nav" install-browser --where)" -type f -name chrome-headless-shell | head -n 1)
if [ -z "$shell" ]; then
  echo "glama build: no chrome-headless-shell after install-browser" >&2
  exit 1
fi
if ldd "$shell" | grep "not found"; then
  echo "glama build: libraries missing for $shell" >&2
  exit 1
fi
echo "glama build: navigera $version with $shell"
