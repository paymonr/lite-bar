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

The tray menu also has **Open settings file**, **Open game stats** (see [Stats and history](#stats-and-history)), and **Reload settings**, which re-reads the settings file and keeps the current on/off state of the overlay.

The overlay toggles with `Ctrl+Alt+Shift+L` by default.

### Files

| File | Path |
| --- | --- |
| Settings | `%APPDATA%\LiteBar\settings.toml` |
| Log | `%APPDATA%\LiteBar\lite-bar.log` |
| Game stats | `%APPDATA%\LiteBar\stats\` (see [Stats and history](#stats-and-history)) |

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
| `counter_size` | height of the FPS number, integer pixels, 8-72 | `18` |
| `text_size` | height of the "FPS" label and the statistics, integer pixels, 6-48 | `11` |

`[hotkeys]`

| Key | Type and values | Default |
| --- | --- | --- |
| `toggle_overlay` | string, e.g. `"Ctrl+Alt+Shift+L"` | `"Ctrl+Alt+Shift+L"` |

Hotkey syntax: zero or more modifiers (`Ctrl`, `Alt`, `Shift`, `Win`) and exactly one key (`A`-`Z`, `0`-`9` or `F1`-`F24`), joined with `+`, case-insensitive. For letter and digit keys at least one of `Ctrl`, `Alt` or `Win` is required (`Shift` alone would stop you typing capital letters). Function keys also accept `Shift` alone, e.g. `Shift+F5`. If the combination is already taken by another program, the tray menu shows the hotkey as unavailable.

`[colors]`

| Key | Type and values | Default |
| --- | --- | --- |
| `saved` | list of `"#RRGGBB"` colors saved in the settings window's color picker, at most 16 (extra and invalid entries are ignored with a warning) | `[]` |

`[game_names]`

Optional display names for games, used in the game stats files (`games.csv` and `sessions.csv`). Each key is an exe file name or a full path (a full path wins over a file name), and each value is the name to use. Use a single-quoted string for a path so backslashes need no escaping. There are no entries by default. A value that is not a string is ignored with a warning.

```toml
[game_names]
"javaw.exe" = "Minecraft"
'C:\Games\Foo\foo.exe' = "Foo: Remastered"
```

If you ran an earlier build, delete `%APPDATA%\LiteBar\settings.toml` to get the commented version (the old `corner` key is no longer read).

### Stats and history

With `stats` set to `"inline"` or `"stacked"`, the overlay shows three numbers next to the FPS, counted since the game started:

- **avg**: frames divided by the total frame time.
- **1% low** (`1%L`): the frame rate of the slowest 1% of frames (1 divided by the 99th-percentile frame time).
- **1% high** (`1%H`): the frame rate of the fastest 1% of frames (1 divided by the 1st-percentile frame time).

They show `—` until the game has at least 100 counted frames. Frame times are kept in buckets about 1% wide, so the percentiles are accurate to about 1%.

Frames only count while the counter is on (capture stops while the overlay is hidden) and while the game is the foreground app. Alt-tabbing away pauses its stats, and coming back resumes them. A gap of more than 2 s between two frames (sleep or hibernate, a modal dialog, a game that stops drawing) is a break, not a frame time, so it adds nothing to the play time.

A play session runs from a game's first counted frame until its process exits, or until Lite Bar quits or you sign out. Sessions with less than 60 s of counted play are dropped, which keeps out launchers and quick alt-tabs. Common programs that aren't games never get a session (the overlay still shows their live stats). They are matched by exe file name:

- browsers: `chrome`, `msedge`, `firefox`, `opera`, `brave`, `vivaldi`
- Windows shell and system: `explorer`, `dwm`, `applicationframehost`, `systemsettings`, `searchhost`, `startmenuexperiencehost`, `shellexperiencehost`, `textinputhost`, `msedgewebview2`
- chat: `discord`
- launchers: `steam`, `steamwebhelper`, `epicgameslauncher`, `galaxyclient`, `battle.net`, `eadesktop`, `ubisoftconnect`, `upc`
- media and streaming: `obs64`, `vlc`, `mpc-hc64`, `spotify`
- editors and tools: `code`, `devenv`, `photoshop`, `blender`
- Lite Bar itself: `lite-bar`

The list is built in for now; a later version (M4) will let you mark apps as games or non-games yourself. If the game exits while the overlay is hidden, the session ends at the last time Lite Bar saw it running. Kept sessions are saved in `%APPDATA%\LiteBar\stats\`:

| File | Contents |
| --- | --- |
| `games.csv` | One row per game: name, sessions, play time, first and last played, lifetime avg / 1% low / 1% high, exe path. Newest first. Regenerated after every session. |
| `sessions.csv` | One row per session: start, end, game, play time, frames, avg / 1% low / 1% high, exe path. New rows are appended. |
| `games.toml` | Lite Bar's own store, with each game's frame-time histogram so lifetime percentiles come from every frame. Don't edit it. |
| `pending.toml` | Only while needed: sessions and rows that couldn't be written at quit. Written at the next start, then deleted. |

**Open game stats** in the tray menu opens `games.csv` in its default program (the stats folder, while there is no CSV yet). If a CSV is open in Excel and can't be written, the write is retried after the next session and at quit, and what still can't be written at quit or sign-out is saved to `stats\pending.toml` and retried later, even after a restart; `games.toml` stays the source of truth.

Games are kept apart by the exe's full path. For Microsoft Store and Game Pass apps under `WindowsApps`, the version and architecture are dropped from the package folder in the key, so a game update doesn't start a new entry. For Java games (`java.exe`, `javaw.exe`) the name from the window title is part of the key too, so Minecraft and another Java game stay separate. The name is the first of these that works:

1. the launcher's records: Steam, Epic, GOG, or the Xbox app / Game Pass;
2. the exe's product name or file description, skipping generic ones such as "Unity Player";
3. the window title, without version numbers and `- ...` suffixes;
4. the exe file name without `.exe`.

To rename a game, add it to `[game_names]` in the settings file and choose **Reload settings**: `games.csv` is regenerated with the new name, and new `sessions.csv` rows use it (existing rows keep the old name).

Anti-cheat: Lite Bar never injects into, hooks or reads the memory of a game. Its only contact with a game's process is `OpenProcess` with `PROCESS_QUERY_LIMITED_INFORMATION` to read the exe path, the start time and whether the process has exited, and the handle is closed straight away. Lite Bar queries a recorded game's process when its session starts and then about once a second only while the game has stopped presenting frames; while it presents, that alone shows it is running, and it is queried at most every 30 s. The window title comes from `GetWindowTextW`, once, when the session starts. Everything else reads files and the registry, with no network lookups. If a game's process refuses even that limited query, its live stats still show on the overlay, but it isn't recorded in the history (a warning goes to the log). Lite Bar asks such a process at most once each time it gets the focus.

### Known limitations

- Exclusive fullscreen games may hide the overlay. Use borderless or windowed fullscreen.
- FPS needs administrator rights in M1. The tray menu offers **Restart as administrator** when capture cannot start.
- The FPS value lags by roughly 0.5-1 s because of ETW buffering.
- A graceful `taskkill` (without `/F`) does not close the app. Use the tray icon's **Quit**.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this project by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
