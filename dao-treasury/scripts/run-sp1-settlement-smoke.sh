#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRIPT_MANIFEST="$ROOT_DIR/sp1-settlement-smoke/script/Cargo.toml"

MODE="${1:-execute}"
shift || true

if [[ -f "$HOME/.zshenv" ]]; then
  # shellcheck disable=SC1090
  source "$HOME/.zshenv"
fi

if [[ "$(uname -s)" == "Darwin" ]]; then
  SDK_PATH="$(xcrun --sdk macosx --show-sdk-path)"
  export SP1_GNARK_FFI_GO_ENVS="CC=/usr/bin/clang;CGO_CFLAGS=-isysroot $SDK_PATH;CGO_LDFLAGS=-isysroot $SDK_PATH"
fi

if ! rustup toolchain list | grep -q '^succinct'; then
  echo "[sp1-smoke] installing succinct toolchain..."
  cargo prove install-toolchain
fi

case "$MODE" in
  execute)
    echo "[sp1-smoke] execute mode"
    cargo run --release --manifest-path "$SCRIPT_MANIFEST" -- --execute "$@"
    ;;
  core)
    echo "[sp1-smoke] core proof mode"
    cargo run --release --manifest-path "$SCRIPT_MANIFEST" -- --prove --system core "$@"
    ;;
  plonk)
    echo "[sp1-smoke] plonk proof mode"
    cargo run --release --manifest-path "$SCRIPT_MANIFEST" -- --prove --system plonk "$@"
    ;;
  *)
    echo "usage: $0 [execute|core|plonk] [extra args...]" >&2
    exit 1
    ;;
esac
