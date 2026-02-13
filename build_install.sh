#!/usr/bin/env bash
set -euo pipefail

BIN_NAME="dagent"
BIN_DIR="$HOME/.cargo/bin"
PROFILE="release"
RUN_TESTS=1

usage() {
  cat <<USAGE
Usage: $(basename "$0") [options]

Build and install DAgent to $HOME/.cargo/bin.

Options:
  --debug               Build with debug profile (default: release)
  --skip-tests          Skip running cargo tests before build
  --bin-name <name>     Binary name to install (default: dagent)
  -h, --help            Show this help message

Examples:
  $(basename "$0")
  $(basename "$0") --skip-tests
  $(basename "$0") --debug
USAGE
}

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "error: missing required command: $1" >&2
    exit 1
  fi
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --debug)
      PROFILE="debug"
      shift
      ;;
    --skip-tests)
      RUN_TESTS=0
      shift
      ;;
    --bin-name)
      [[ $# -ge 2 ]] || { echo "error: --bin-name requires a value" >&2; exit 1; }
      BIN_NAME="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "error: unknown option: $1" >&2
      usage
      exit 1
      ;;
  esac
done

if ! command -v cargo >/dev/null 2>&1; then
  if [[ -f "$HOME/.cargo/env" ]]; then
    # shellcheck disable=SC1090
    source "$HOME/.cargo/env"
  fi
fi

require_cmd cargo
require_cmd install

echo "==> profile: $PROFILE"
echo "==> bin name: $BIN_NAME"
echo "==> install dir: $BIN_DIR"

if [[ $RUN_TESTS -eq 1 ]]; then
  echo "==> running tests"
  cargo test --quiet
else
  echo "==> skipping tests"
fi

if [[ "$PROFILE" == "release" ]]; then
  echo "==> building (release)"
  cargo build --release
  SRC_BIN="target/release/$BIN_NAME"
else
  echo "==> building (debug)"
  cargo build
  SRC_BIN="target/debug/$BIN_NAME"
fi

if [[ ! -x "$SRC_BIN" ]]; then
  echo "error: built binary not found or not executable: $SRC_BIN" >&2
  exit 1
fi

mkdir -p "$BIN_DIR"
if [[ ! -w "$BIN_DIR" ]]; then
  echo "error: no write permission for $BIN_DIR" >&2
  exit 1
fi
install -m 0755 "$SRC_BIN" "$BIN_DIR/$BIN_NAME"

TARGET_BIN="$BIN_DIR/$BIN_NAME"
echo "==> installed: $TARGET_BIN"
if [[ -x "$TARGET_BIN" ]]; then
  echo "==> version: $($TARGET_BIN --version 2>/dev/null || echo 'version command not available')"
fi

echo "done"
