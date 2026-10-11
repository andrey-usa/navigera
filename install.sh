#!/bin/sh
# Install navigera: a prebuilt binary from the latest GitHub release, or a
# source build with cargo when no binary fits this platform.
#
#   curl -fsSL https://raw.githubusercontent.com/andrey-usa/navigera/master/install.sh | sh
#
# Env: NAVIGERA_INSTALL_DIR (default ~/.local/bin), NAVIGERA_VERSION
# (a tag such as v0.2.0; default: latest release), NAVIGERA_GIT_REV
# (commit for the source build; default: master).
set -eu

REPO="andrey-usa/navigera"
DIR="${NAVIGERA_INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${NAVIGERA_VERSION:-latest}"

os=$(uname -s)
arch=$(uname -m)
case "$os/$arch" in
  Linux/x86_64) target=x86_64-unknown-linux-musl ;;
  Linux/aarch64 | Linux/arm64) target=aarch64-unknown-linux-musl ;;
  Darwin/arm64) target=aarch64-apple-darwin ;;
  Darwin/x86_64) target=x86_64-apple-darwin ;;
  *) target="" ;;
esac

if [ "$VERSION" = latest ]; then
  url="https://github.com/$REPO/releases/latest/download/navigera-$target.tar.gz"
else
  url="https://github.com/$REPO/releases/download/$VERSION/navigera-$target.tar.gz"
fi

mkdir -p "$DIR"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

if [ -n "$target" ] && curl -fsSL --retry 3 -o "$tmp/navigera.tar.gz" "$url" 2>/dev/null; then
  tar -xzf "$tmp/navigera.tar.gz" -C "$tmp"
  install -m 0755 "$tmp/navigera" "$DIR/navigera"
  echo "installed $DIR/navigera ($target, $VERSION)"
elif command -v cargo >/dev/null 2>&1; then
  echo "no prebuilt binary for ${target:-$os/$arch}; building from source with cargo (about a minute)…"
  # The repo also holds Rust benchmark contenders, so name the package.
  cargo install --locked --git "https://github.com/$REPO" ${NAVIGERA_GIT_REV:+--rev "$NAVIGERA_GIT_REV"} navigera --root "$tmp/root"
  install -m 0755 "$tmp/root/bin/navigera" "$DIR/navigera"
  echo "installed $DIR/navigera (built from source)"
else
  echo "error: no prebuilt binary for ${target:-$os/$arch} and no cargo; install Rust (https://rustup.rs) and rerun" >&2
  exit 1
fi

case ":$PATH:" in
  *":$DIR:"*) ;;
  *) echo "note: $DIR is not on PATH; run: export PATH=\"$DIR:\$PATH\"" ;;
esac
"$DIR/navigera" --version
echo "next: navigera install-skill   # usage guide for your agent (./.agents/skills; --claude for ./.claude/skills)"
