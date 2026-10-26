#!/usr/bin/env bash
# Run `cargo xwin <cmd>` for the Windows target from WSL.
# Usage: scripts/xwin.sh test -p lite-bar
#        scripts/xwin.sh build --release -p lite-bar
set -euo pipefail

[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"

# lld-link (symlink to rust-lld) lives in ~/.local/bin
export PATH="$HOME/.local/bin:$PATH"
# Use the GitHub-hosted windows-msvc-sysroot instead of Microsoft's CRT download
export XWIN_CROSS_COMPILER=clang
# Run Windows test .exe files directly through WSL interop (no wine)
export CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUNNER=env

exec cargo xwin "$1" --target x86_64-pc-windows-msvc "${@:2}"
