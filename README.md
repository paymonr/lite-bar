# Lite Bar

Windows tray app with an FPS overlay, written in Rust. Developed in WSL2 and cross-compiled to Windows.

## Dev setup (WSL2)

- rustup + stable Rust (`source ~/.cargo/env`)
- `rustup target add x86_64-pc-windows-msvc`
- `rustup component add llvm-tools`
- `cargo install --locked cargo-xwin`
- `~/.local/bin/lld-link` symlinked to rust-lld (no system clang/lld/llvm needed)

`scripts/xwin.sh` wraps `cargo xwin` for the Windows target and sets the environment that makes this setup work:
`XWIN_CROSS_COMPILER=clang` (uses the GitHub-hosted windows-msvc-sysroot, since Microsoft's CRT download is not reachable)
and `CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUNNER=env` (test `.exe` files run directly via WSL interop, no wine).

Use pure-Rust dependencies only; crates with C build scripts will not build with this toolchain.

## Test

```bash
cargo test -p lite-bar-core          # native Linux
scripts/xwin.sh test -p lite-bar     # Windows, runs the .exe through WSL interop
```

## Build and run

```bash
scripts/xwin.sh build --release -p lite-bar
./target/x86_64-pc-windows-msvc/release/lite-bar.exe
```

FPS capture needs an elevated (administrator) run in M1.
