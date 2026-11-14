#!/bin/sh
# Install navigera: a prebuilt binary from the latest GitHub release, or a
# source build with cargo when no binary fits this platform.
#
#   curl -fsSL https://raw.githubusercontent.com/andrey-usa/navigera/master/install.sh | sh
#
# On Windows it runs in Git Bash (Claude Code's shell there), MSYS2 or
# Cygwin and installs the Windows binary, as install.ps1 does in PowerShell.
#
# Env: NAVIGERA_INSTALL_DIR (default ~/.local/bin; on Windows
# %LOCALAPPDATA%\navigera\bin, install.ps1's), NAVIGERA_VERSION
# (a tag such as v0.2.0; default: latest release), NAVIGERA_GIT_REV
# (commit for the source build; default: master).
#
# The archive is checked against the SHA-256 the release publishes next to
# it; a mismatch or a missing checksum stops the install
# (NAVIGERA_NO_VERIFY=1 skips the check).
set -eu

REPO="andrey-usa/navigera"
VERSION="${NAVIGERA_VERSION:-latest}"

os=$(uname -s)
arch=$(uname -m)
exe=navigera
ext=tar.gz
default_dir="$HOME/.local/bin"
case "$os/$arch" in
  Linux/x86_64) target=x86_64-unknown-linux-musl ;;
  Linux/aarch64 | Linux/arm64) target=aarch64-unknown-linux-musl ;;
  Darwin/arm64) target=aarch64-apple-darwin ;;
  Darwin/x86_64) target=x86_64-apple-darwin ;;
  # The x64 binary; Windows on Arm runs it emulated (as install.ps1 says).
  MINGW*/* | MSYS*/* | CYGWIN*/*)
    target=x86_64-pc-windows-msvc exe=navigera.exe ext=zip
    if [ -n "${LOCALAPPDATA:-}" ] && command -v cygpath >/dev/null 2>&1; then
      default_dir="$(cygpath -u "$LOCALAPPDATA")/navigera/bin"
    fi ;;
  *) target="" ;;
esac
DIR="${NAVIGERA_INSTALL_DIR:-$default_dir}"

if [ "$VERSION" = latest ]; then
  url="https://github.com/$REPO/releases/latest/download/navigera-$target.$ext"
else
  url="https://github.com/$REPO/releases/download/$VERSION/navigera-$target.$ext"
fi

mkdir -p "$DIR"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

fetch() { curl --proto '=https' --tlsv1.2 -fsSL --retry 3 -o "$2" "$1"; }

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1
  elif command -v shasum >/dev/null 2>&1; then shasum -a 256 "$1" | cut -d' ' -f1
  else return 1
  fi
}

# A Windows zip: unzip where Git Bash has it, else PowerShell's own.
unpack() {
  if [ "$ext" = tar.gz ]; then
    tar -xzf "$tmp/navigera.$ext" -C "$tmp"
  elif command -v unzip >/dev/null 2>&1; then
    unzip -q -o "$tmp/navigera.$ext" -d "$tmp"
  else
    powershell.exe -NoProfile -NonInteractive -Command \
      "Expand-Archive -Force -LiteralPath '$(cygpath -w "$tmp/navigera.$ext")' -DestinationPath '$(cygpath -w "$tmp")'"
  fi
}

# `install` replaces a binary a running session holds (Linux, macOS);
# Windows has no such file to replace, and Git Bash may have no `install`.
place() {
  if [ "$ext" = zip ]; then cp "$1" "$DIR/$exe"; else install -m 0755 "$1" "$DIR/$exe"; fi
}

verify() {
  [ "${NAVIGERA_NO_VERIFY:-}" = 1 ] && return 0
  if ! fetch "$url.sha256" "$tmp/navigera.$ext.sha256" 2>/dev/null; then
    echo "error: no checksum published at $url.sha256 (NAVIGERA_NO_VERIFY=1 to install anyway)" >&2
    exit 1
  fi
  want=$(cut -d' ' -f1 < "$tmp/navigera.$ext.sha256")
  if ! got=$(sha256 "$tmp/navigera.$ext"); then
    echo "error: need sha256sum or shasum to verify the download (NAVIGERA_NO_VERIFY=1 to skip)" >&2
    exit 1
  fi
  if [ "$want" != "$got" ]; then
    echo "error: checksum mismatch for $url (want $want, got $got)" >&2
    exit 1
  fi
}

if [ -n "$target" ] && fetch "$url" "$tmp/navigera.$ext" 2>/dev/null; then
  verify
  unpack
  place "$tmp/$exe"
  echo "installed $DIR/$exe ($target, $VERSION)"
elif command -v cargo >/dev/null 2>&1; then
  echo "no prebuilt binary for ${target:-$os/$arch}; building from source with cargo (about a minute)…"
  # The repo also holds Rust benchmark contenders, so name the package.
  cargo install --locked --git "https://github.com/$REPO" ${NAVIGERA_GIT_REV:+--rev "$NAVIGERA_GIT_REV"} navigera --root "$tmp/root"
  place "$tmp/root/bin/$exe"
  echo "installed $DIR/$exe (built from source)"
else
  echo "error: no prebuilt binary for ${target:-$os/$arch} and no cargo; install Rust (https://rustup.rs) and rerun" >&2
  exit 1
fi

# ~/.local/bin is not on macOS's PATH, nor on Linux's until a login after
# the directory first exists. An agent's shell keeps no `export` from one
# command to the next, so the note names the full path too.
case ":$PATH:" in
  *":$DIR:"*) ;;
  *) echo "note: $DIR is not on PATH, so \`navigera\` alone is not found yet: run $DIR/navigera, or add the directory to PATH (export PATH=\"$DIR:\$PATH\" in your shell profile)" ;;
esac
"$DIR/$exe" --version
echo "next: navigera install-skill   # usage guide for your agent (./.agents/skills; --claude for ./.claude/skills)"
