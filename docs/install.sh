#!/usr/bin/env bash
# Open Karaoke installer for macOS and Linux.
#
#   curl -fsSL https://osmanahmadxai.github.io/open-karaoke/install.sh | bash
#
# Downloads the latest release from GitHub and installs it. Files fetched
# with curl aren't flagged as "downloaded from the internet", so the app
# opens without the unverified-developer prompt that browser downloads get.
set -euo pipefail

REPO="osmanahmadxai/open-karaoke"
BASE="https://github.com/$REPO/releases/latest/download"

info() { printf '\033[1;35m==>\033[0m %s\n' "$*"; }
fail() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

tmp="$(mktemp -d)"
cleanup() {
  if [ -n "${mounted:-}" ]; then hdiutil detach "$tmp/mnt" -quiet 2>/dev/null || true; fi
  rm -rf "$tmp"
}
trap cleanup EXIT

install_macos() {
  local dest="${OPEN_KARAOKE_DEST:-/Applications}/Open Karaoke.app"
  info "Downloading Open Karaoke for macOS…"
  curl -fL --progress-bar "$BASE/OpenKaraoke-macOS.dmg" -o "$tmp/OpenKaraoke.dmg"

  info "Installing to ${dest%/*}…"
  mkdir -p "$tmp/mnt"
  hdiutil attach -nobrowse -readonly -noautoopen -mountpoint "$tmp/mnt" "$tmp/OpenKaraoke.dmg" >/dev/null
  mounted=1
  local app
  app="$(find "$tmp/mnt" -maxdepth 1 -name '*.app' | head -n 1)"
  [ -n "$app" ] || fail "The download didn't contain the app. Please try again."

  local sudo=""
  [ -w "${dest%/*}" ] || sudo="sudo"
  [ -n "$sudo" ] && info "Administrator password needed to write to ${dest%/*}."
  $sudo rm -rf "$dest"
  $sudo ditto "$app" "$dest"
  # Clear the flag in case an earlier browser download left one behind.
  $sudo xattr -dr com.apple.quarantine "$dest" 2>/dev/null || true

  info "Installed Open Karaoke $(defaults read "$dest/Contents/Info" CFBundleShortVersionString 2>/dev/null || echo "")."
  if [ -z "${OPEN_KARAOKE_NO_LAUNCH:-}" ]; then
    info "Opening Open Karaoke…"
    open "$dest"
  fi
}

install_linux() {
  [ "$(uname -m)" = "x86_64" ] || fail "Open Karaoke currently supports 64-bit Intel/AMD Linux only."
  local bin_dir="${OPEN_KARAOKE_DEST:-$HOME/.local/bin}"
  local app="$bin_dir/open-karaoke"
  info "Downloading Open Karaoke for Linux…"
  curl -fL --progress-bar "$BASE/OpenKaraoke-Linux.AppImage" -o "$tmp/OpenKaraoke.AppImage"

  info "Installing to $app…"
  mkdir -p "$bin_dir" "$HOME/.local/share/applications" "$HOME/.local/share/icons"
  install -m 755 "$tmp/OpenKaraoke.AppImage" "$app"
  curl -fsSL "https://osmanahmadxai.github.io/open-karaoke/icon.png" -o "$HOME/.local/share/icons/open-karaoke.png" || true
  cat > "$HOME/.local/share/applications/open-karaoke.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Open Karaoke
Comment=Open-source karaoke with real-time autotune
Exec="$app" %U
Icon=$HOME/.local/share/icons/open-karaoke.png
Categories=AudioVideo;Audio;Music;
Terminal=false
EOF

  info "Installed. Find Open Karaoke in your app menu, or run: $app"
  if ! ldconfig -p 2>/dev/null | grep -q 'libfuse\.so\.2'; then
    info "If it doesn't start, install FUSE 2 (Ubuntu/Debian: sudo apt install libfuse2t64 · Fedora: sudo dnf install fuse-libs)."
  fi
  if [ -z "${OPEN_KARAOKE_NO_LAUNCH:-}" ] && [ -n "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]; then
    nohup "$app" >/dev/null 2>&1 &
  fi
}

case "$(uname -s)" in
  Darwin) install_macos ;;
  Linux) install_linux ;;
  *) fail "On Windows, open PowerShell and run: irm https://raw.githubusercontent.com/$REPO/main/docs/install.ps1 | iex" ;;
esac
