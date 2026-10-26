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

## Using Lite Bar

Lite Bar lives in the system tray and shows an FPS overlay for the foreground application. It shows `—` while the foreground application is not presenting frames. Quit it from the tray icon: right-click, then **Quit**.

The tray menu also has **Reload settings**, which re-reads the settings file and keeps the current on/off state of the overlay, and **Open settings file**.

The overlay toggles with `Ctrl+Alt+Shift+L` by default.

### Files

| File | Path |
| --- | --- |
| Settings | `%APPDATA%\LiteBar\settings.toml` |
| Log | `%APPDATA%\LiteBar\lite-bar.log` |

The settings file is created with the default values and a comment for every key on first run and is never overwritten afterwards. An invalid value falls back to its default and a warning is written to the log. Unknown keys are ignored.

### Settings

`[overlay]`

| Key | Type and values | Default |
| --- | --- | --- |
| `visible` | bool; whether the overlay is shown at startup | `true` |
| `location` | `"top-left"`, `"top-center"`, `"top-right"`, `"center-left"`, `"center-right"`, `"bottom-left"`, `"bottom-center"` or `"bottom-right"` | `"top-left"` |
| `stats` | live FPS statistics layout: `"off"`, `"inline"` or `"stacked"` | `"inline"` |
| `number_color` | `"#RRGGBB"` | `"#7CFC7C"` |
| `label_color` | `"#RRGGBB"` | `"#FFFFFF"` |
| `background_color` | `"#RRGGBB"` | `"#000000"` |
| `background_opacity` | integer percent, 0-100 | `55` |
| `text_size` | integer pixels, 8-72 | `18` |

`[hotkeys]`

| Key | Type and values | Default |
| --- | --- | --- |
| `toggle_overlay` | string, e.g. `"Ctrl+Alt+Shift+L"` | `"Ctrl+Alt+Shift+L"` |

Hotkey syntax: zero or more modifiers (`Ctrl`, `Alt`, `Shift`, `Win`) and exactly one key (`A`-`Z`, `0`-`9` or `F1`-`F24`), joined with `+`, case-insensitive. For letter and digit keys at least one of `Ctrl`, `Alt` or `Win` is required (`Shift` alone would stop you typing capital letters). Function keys also accept `Shift` alone, e.g. `Shift+F5`. If the combination is already taken by another program, the tray menu shows the hotkey as unavailable.

`[game_names]`

Optional display names for games. Each key is an exe file name or a full path, and each value is the name to show. Use a single-quoted string for a path so backslashes need no escaping. There are no entries by default. A value that is not a string is ignored with a warning.

```toml
[game_names]
"javaw.exe" = "Minecraft"
'C:\Games\Foo\foo.exe' = "Foo: Remastered"
```

If you ran an earlier build, delete `%APPDATA%\LiteBar\settings.toml` to get the commented version (the old `corner` key is no longer read).

### Known limitations

- Exclusive fullscreen games may hide the overlay. Use borderless or windowed fullscreen.
- FPS needs administrator rights in M1. The tray menu offers **Restart as administrator** when capture cannot start.
- The FPS value lags by roughly 0.5-1 s because of ETW buffering.
- A graceful `taskkill` (without `/F`) does not close the app. Use the tray icon's **Quit**.
