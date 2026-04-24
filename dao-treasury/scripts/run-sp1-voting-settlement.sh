#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRIPT_MANIFEST="$ROOT_DIR/sp1-voting-settlement/script/Cargo.toml"
DEFAULT_TRANSCRIPT="$ROOT_DIR/artifacts/zkvm-settlement-transcript-c27689d3b084.json"

MODE="${1:-execute}"
shift || true

TRANSCRIPT="${1:-$DEFAULT_TRANSCRIPT}"
if [[ $# -gt 0 ]]; then
  shift
fi

if [[ -f "$HOME/.zshenv" ]]; then
  # shellcheck disable=SC1090
  source "$HOME/.zshenv"
fi

if [[ "$(uname -s)" == "Darwin" ]]; then
  SDK_PATH="$(xcrun --sdk macosx --show-sdk-path)"
  export SP1_GNARK_FFI_GO_ENVS="CC=/usr/bin/clang;CGO_CFLAGS=-isysroot $SDK_PATH;CGO_LDFLAGS=-isysroot $SDK_PATH"
  if [[ "$(uname -m)" == "arm64" ]]; then
    export DOCKER_DEFAULT_PLATFORM="${DOCKER_DEFAULT_PLATFORM:-linux/amd64}"
  fi
fi

if ! rustup toolchain list | grep -q '^succinct'; then
  echo "[sp1-voting] installing succinct toolchain..."
  cargo prove install-toolchain
fi

case "$MODE" in
  execute)
    echo "[sp1-voting] execute mode"
    cargo run --release --manifest-path "$SCRIPT_MANIFEST" -- --execute --transcript "$TRANSCRIPT" "$@"
    ;;
  core)
    echo "[sp1-voting] core proof mode"
    cargo run --release --manifest-path "$SCRIPT_MANIFEST" -- --prove --system core --transcript "$TRANSCRIPT" "$@"
    ;;
  plonk)
    echo "[sp1-voting] plonk proof mode"
    if ! command -v docker >/dev/null 2>&1 || ! docker info >/dev/null 2>&1; then
      echo "[sp1-voting] plonk mode requires Docker for SP1 gnark FFI; start Docker and retry." >&2
      exit 1
    fi
    cargo run --release --manifest-path "$SCRIPT_MANIFEST" -- --prove --system plonk --transcript "$TRANSCRIPT" "$@"
    ;;
  groth16)
    echo "[sp1-voting] groth16 proof mode"
    if ! command -v docker >/dev/null 2>&1 || ! docker info >/dev/null 2>&1; then
      echo "[sp1-voting] groth16 mode requires Docker for SP1 gnark FFI; start Docker and retry." >&2
      exit 1
    fi
    cargo run --release --manifest-path "$SCRIPT_MANIFEST" -- --prove --system groth16 --transcript "$TRANSCRIPT" "$@"
    ;;
  *)
    echo "usage: $0 [execute|core|plonk|groth16] [transcript.json] [extra args...]" >&2
    exit 1
    ;;
esac
