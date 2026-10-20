#!/usr/bin/env bash
# Fotowache installer for macOS and Linux.
#
#   curl -fsSL https://raw.githubusercontent.com/robeertm/Fotowache/main/install.sh | bash
#
# Downloads the latest release for this computer, checks its SHA-256 checksum
# and installs it:
#   macOS: /Applications/Fotowache.app (or ~/Applications if /Applications is not writable)
#   Linux: ~/.local/opt/fotowache, command `fotowache` in ~/.local/bin, menu entry
# Nothing is installed system-wide on Linux, no sudo needed.
# Uninstall: run this script with --uninstall.
set -euo pipefail

REPO="robeertm/Fotowache"
os="$(uname -s)"
arch="$(uname -m)"

say() { printf '\033[1m%s\033[0m\n' "$*"; }
fail() { printf 'Error: %s\n' "$*" >&2; exit 1; }

uninstall() {
  case "$os" in
    Darwin)
      for d in /Applications "$HOME/Applications"; do
        [ -d "$d/Fotowache.app" ] && rm -rf "$d/Fotowache.app" && say "Removed $d/Fotowache.app"
      done ;;
    Linux)
      rm -rf "$HOME/.local/opt/fotowache" "$HOME/.local/bin/fotowache" \
        "$HOME/.local/share/applications/fotowache.desktop" \
        "$HOME/.local/share/icons/hicolor/256x256/apps/fotowache.png"
      say "Fotowache removed." ;;
  esac
  say "Your photos and the .fotowache.json edit files next to them were not touched."
  exit 0
}
[ "${1:-}" = "--uninstall" ] && uninstall

case "$os/$arch" in
  Darwin/arm64) suffix="macos-arm64.zip" ;;
  Darwin/x86_64) suffix="macos-x86_64.zip" ;;
  Linux/x86_64) suffix="linux-x86_64.tar.gz" ;;
  Linux/aarch64 | Linux/arm64) suffix="linux-aarch64.tar.gz" ;;
  *) fail "no Fotowache build for $os $arch" ;;
esac
command -v curl >/dev/null || fail "curl is required"

say "Looking up the latest Fotowache release ..."
release="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest")" ||
  fail "could not reach the release server"
asset_url() { # file name suffix -> download URL
  printf '%s' "$release" | grep -o "\"browser_download_url\": *\"[^\"]*$1\"" |
    head -1 | sed 's/.*"\(https[^"]*\)"$/\1/'
}
url="$(asset_url "$suffix")"
sums_url="$(asset_url "SHA256SUMS.txt")"
[ -n "$url" ] || fail "the latest release has no package for $os $arch"
file="${url##*/}"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
say "Downloading $file ..."
curl -fL --progress-bar -o "$tmp/$file" "$url"
if [ -n "$sums_url" ]; then
  curl -fsSL -o "$tmp/SHA256SUMS.txt" "$sums_url"
  expected="$(grep " $file\$" "$tmp/SHA256SUMS.txt" | awk '{print $1}')"
  if command -v sha256sum >/dev/null; then
    actual="$(sha256sum "$tmp/$file" | awk '{print $1}')"
  else
    actual="$(shasum -a 256 "$tmp/$file" | awk '{print $1}')"
  fi
  [ -n "$expected" ] && [ "$expected" = "$actual" ] || fail "checksum mismatch for $file"
  say "Checksum OK."
fi

case "$os" in
  Darwin)
    target="/Applications"
    [ -w "$target" ] || { target="$HOME/Applications"; mkdir -p "$target"; }
    ditto -x -k "$tmp/$file" "$tmp/app"
    pkill -x Fotowache 2>/dev/null || true
    rm -rf "$target/Fotowache.app"
    mv "$tmp/app/Fotowache.app" "$target/Fotowache.app"
    # Downloaded with curl, so macOS did not mark it as quarantined; clear the
    # flag anyway in case an older copy carried it.
    xattr -dr com.apple.quarantine "$target/Fotowache.app" 2>/dev/null || true  # Kennung: Dateiattribut
    say "Fotowache installed: $target/Fotowache.app"
    say "Start it from the Applications folder or with: open -a Fotowache"
    ;;
  Linux)
    dest="$HOME/.local/opt/fotowache"
    rm -rf "$dest"
    mkdir -p "$dest" "$HOME/.local/bin" "$HOME/.local/share/applications" \
      "$HOME/.local/share/icons/hicolor/256x256/apps"
    tar xzf "$tmp/$file" -C "$dest" --strip-components=1
    ln -sf "$dest/fotowache" "$HOME/.local/bin/fotowache"
    cp "$dest/fotowache.png" "$HOME/.local/share/icons/hicolor/256x256/apps/fotowache.png"
    sed "s|^Exec=fotowache|Exec=$dest/fotowache|" "$dest/fotowache.desktop" \
      >"$HOME/.local/share/applications/fotowache.desktop"
    command -v update-desktop-database >/dev/null &&
      update-desktop-database "$HOME/.local/share/applications" 2>/dev/null || true
    say "Fotowache installed: $dest"
    say "Start it from your application menu or with: fotowache"
    case ":$PATH:" in
      *":$HOME/.local/bin:"*) ;;
      *) say "Note: add ~/.local/bin to your PATH to start it from a terminal." ;;
    esac
    ;;
esac
