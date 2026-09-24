#!/usr/bin/env bash
# herdr `[[build]]` step: put the herdr-pr-modal binary at bin/herdr-pr-modal,
# where every manifest command execs it.
#
# Downloads the prebuilt binary for this platform from the GitHub Release whose
# tag is the manifest version, so no Rust toolchain is needed. Falls back to
# `cargo build` when no prebuilt binary fits (unknown platform, offline) and
# cargo is installed. HERDR_PR_MODAL_FROM_SOURCE=1 always builds from source.
#
# The plugin root is resolved from this script's location: build commands may
# not receive the runtime env ($HERDR_PLUGIN_ROOT).
set -euo pipefail

NAME="herdr-pr-modal"
REPO="tarektouati/herdr-pr-modal"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN_DIR="$ROOT/bin"

log() { echo "$NAME: $*" >&2; }

build_from_source() {
  if ! command -v cargo >/dev/null 2>&1; then
    log "no prebuilt binary available and cargo is not installed"
    log "install Rust (https://rustup.rs) and reinstall, or open an issue at https://github.com/$REPO/issues"
    exit 1
  fi
  log "building from source with cargo"
  (cd "$ROOT" && cargo build --release --locked)
  mkdir -p "$BIN_DIR"
  install -m 0755 "$ROOT/target/release/$NAME" "$BIN_DIR/$NAME"
  log "installed $BIN_DIR/$NAME"
}

if [ "${HERDR_PR_MODAL_FROM_SOURCE:-}" = 1 ]; then
  build_from_source
  exit 0
fi

# The release tag is the manifest version, so a checkout pulls its own release.
VERSION="$(grep -m1 '^version' "$ROOT/herdr-plugin.toml" | sed -E 's/.*"([^"]+)".*/\1/')"

os="$(uname -s)"
arch="$(uname -m)"
case "$os-$arch" in
  Darwin-arm64)                target="aarch64-apple-darwin" ;;
  Darwin-x86_64)               target="x86_64-apple-darwin" ;;
  Linux-aarch64 | Linux-arm64) target="aarch64-unknown-linux-musl" ;;
  Linux-x86_64)                target="x86_64-unknown-linux-musl" ;;
  *)
    log "no prebuilt binary for $os-$arch"
    build_from_source
    exit 0
    ;;
esac

archive="${NAME}-${target}.tar.gz"
# upload-rust-binary-action names the checksum <name>-<target>.sha256.
checksum="${NAME}-${target}.sha256"
base="https://github.com/${REPO}/releases/download/${VERSION}"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# GitHub's CDN can 404 for a few minutes right after a release publishes.
dl() { curl -fsSL --retry 5 --retry-delay 3 --retry-all-errors --retry-connrefused "$1" -o "$2"; }

log "downloading $archive ($VERSION)"
if ! dl "$base/$archive" "$tmp/$archive" || ! dl "$base/$checksum" "$tmp/$checksum"; then
  log "download failed"
  build_from_source
  exit 0
fi

expected="$(awk '{print $1}' "$tmp/$checksum")"
if command -v sha256sum >/dev/null 2>&1; then
  actual="$(sha256sum "$tmp/$archive" | awk '{print $1}')"
else
  actual="$(shasum -a 256 "$tmp/$archive" | awk '{print $1}')"
fi
if [ "$expected" != "$actual" ]; then
  log "checksum mismatch (expected $expected, got $actual)"
  exit 1
fi

mkdir -p "$BIN_DIR"
tar -xzf "$tmp/$archive" -C "$tmp"
install -m 0755 "$tmp/$NAME" "$BIN_DIR/$NAME"
log "installed $BIN_DIR/$NAME"
