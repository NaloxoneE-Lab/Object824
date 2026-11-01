#!/usr/bin/env bash
# Activates the self-contained Rust toolchain that lives inside this repository.
#
#   source scripts/env.sh
#
# Nothing is installed outside o824-rs: rustup, the cargo home, and the portable
# mingw-w64 linker all live under .toolchain/ (git-ignored). The linker is
# needed because this machine has no Visual Studio build tools, so the toolchain
# is x86_64-pc-windows-gnu rather than the MSVC host CI uses.
#
# The temporary directory is redirected into the repository as well: the mingw
# linker cannot write to %TEMP% under the harness sandbox.

O824_RS_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export O824_RS_ROOT

export CARGO_HOME="$O824_RS_ROOT/.toolchain/cargo"
export RUSTUP_HOME="$O824_RS_ROOT/.toolchain/rustup"

O824_TMP="$O824_RS_ROOT/.toolchain/tmp"
mkdir -p "$O824_TMP"
export TMPDIR="$O824_TMP"
export TMP="$O824_TMP"
export TEMP="$O824_TMP"

O824_CARGO_BIN="$CARGO_HOME/bin"
O824_MINGW_BIN="$O824_RS_ROOT/.toolchain/mingw64/bin"
case ":$PATH:" in
  *":$O824_CARGO_BIN:"*) ;;
  *) PATH="$O824_CARGO_BIN:$PATH" ;;
esac
case ":$PATH:" in
  *":$O824_MINGW_BIN:"*) ;;
  *) PATH="$O824_MINGW_BIN:$PATH" ;;
esac
export PATH

echo "o824-rs: $(cargo --version 2>/dev/null || echo 'cargo not found')"
