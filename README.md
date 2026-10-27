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

The tray menu, top to bottom:

- **Settings…** (bold; double-clicking the tray icon does the same) opens the [settings window](#settings-window).
- **Show FPS overlay** / **Hide FPS overlay**, with the toggle hotkey beside it (marked unavailable if another app holds it).
- **Open settings file** opens `settings.toml` in its default program.
- **Open game stats** (see [Stats and history](#stats-and-history)).
- **Reload settings** re-reads the settings file (and refreshes the settings window if it is open), keeping the current on/off state of the overlay.
- **Restart as administrator to show FPS**, only when capture needs it.
- **Update to v…**, only while a newer release is available (see [Updates](#updates)).
- **Quit** (closes the settings window first, which saves its last changes).

The overlay toggles with `Ctrl+Alt+Shift+L` by default.

### Settings window

**Settings…** opens a window with every setting and a live preview of the counter. A second **Settings…** brings the open window to the front. Every change shows on the overlay straight away and is written to `settings.toml` 300 ms after the last change. Only the changed values are rewritten: your comments, blank lines and key order stay as they are. If the file has become invalid TOML, the change stays in effect until Lite Bar quits but is not written, and a warning goes to the log.

- **Overlay**: show or hide the counter, its location on screen, and the statistics layout.
- **Appearance**: number, text and background colors (hex fields, a color wheel, default chips and up to 16 saved colors; right-click a saved color to remove it), background opacity, counter size and text size.
- **General**: the **Start with Windows** switch (see [Start with Windows](#start-with-windows)).
- **Hotkeys**: click the field and press the new combination; Esc cancels. A combination another app already holds is refused with "Already used by another app" and the old hotkey stays.
- **Updates**: the **Check for updates** switch, **Check now** and the update status (see [Updates](#updates)).
- **Game names**: rename any game you've played. The name is saved under `[game_names]` and `games.csv` is regenerated; **Open stats folder** opens the stats folder.
- **Restore defaults** (click twice) puts every overlay setting and the hotkey back to how Lite Bar ships. Game names and saved colors are kept.

The window takes about 10–20 MB while open; closing it frees all of it.

### Updates

Lite Bar asks GitHub whether a newer release exists. That request to `api.github.com` (the latest release of `paymonr/lite-bar`) is its only network traffic, apart from the download you start with **Update now**. It runs on a background thread about 30 s after Lite Bar starts and then every 24 h while it runs, only while **Check for updates** is on (`[updates] check`). Turning the switch on checks 30 s later; turning it off cancels the next check. **Check now** checks right away, even with the switch off.

When a newer release is out, Settings shows "Update available: v…" with an **Update now** button and an **Open release page** link, and the tray menu gets **Update to v…** above **Quit**. Nothing pops up and nothing is downloaded or installed until you click **Update now**. A failed check is only logged; Settings shows "Couldn't check for updates" and the next scheduled check tries again.

**Update now** (in Settings or the tray) installs the release in place:

1. Settings shows "Downloading v…". A background thread downloads `lite-bar.exe.sha256` and `lite-bar.exe` from GitHub. Only https links on `github.com`, `api.github.com`, `objects.githubusercontent.com` and `github-releases.githubusercontent.com` are used, and the download must finish within 5 minutes. The checksum file may be at most 64 KB and the exe at most 50 MB.
2. The exe's SHA-256 must match the checksum file. Only then is it written next to the running exe as `lite-bar.exe.new`. Until this point, nothing touches the running `lite-bar.exe`.
3. The running exe is renamed to `lite-bar.exe.old`, and `lite-bar.exe.new` is renamed to `lite-bar.exe`. If the second rename fails, the first is undone, so the current version stays in place and keeps running. If even that undo fails, the error names where the previous exe is (`lite-bar.exe.old`) and the log has the details. Rename it back by hand.
4. Lite Bar quits the same way as **Quit**: the open settings are saved and the play sessions are written. It then starts the new `lite-bar.exe --updated`. If Lite Bar was running as administrator, the new instance is too. The new instance waits up to 10 s for the old one to exit. Every start of Lite Bar also deletes a leftover `lite-bar.exe.old` in the background.

Any failure shows "Update failed: …" in red in Settings, with **Open release page**, and leaves the running version untouched. When trying again may help, Settings also shows **Try again**, which downloads the same release again. For example, the checksum may not match, or the download may be cut off or time out. If Lite Bar's folder can't be written to (for example, under Program Files without administrator rights), Settings says "Can't update in this folder — download it from GitHub". In that case, download `lite-bar.exe` from the release page and replace the file yourself. If the new exe is in place but couldn't be started, Settings says "Update installed — restart Lite Bar to finish" and offers **Restart now** to try again. Until then, the old version keeps running, and Lite Bar doesn't offer the installed version again. If only `lite-bar.exe.old` is left in Lite Bar's folder (for example, after a crash mid-update), rename it back to `lite-bar.exe`.

### Start with Windows

FPS capture needs administrator rights, so Lite Bar starts at sign-in through a Windows scheduled task rather than the Startup folder. The task runs Lite Bar elevated with no UAC prompt at each boot. It is off by default. Turn it on with **Settings → General → Start with Windows**.

- The task is named `Lite Bar`, in the root folder of Task Scheduler. It runs at sign-in of the current user with highest privileges, has no time limit and also runs on battery. It is created with `schtasks /Create /XML` and removed with `schtasks /Delete`.
- The switch shows whether the task really exists. Lite Bar checks it when the window opens and after each change.
- If Lite Bar is already running as administrator, the switch changes the task directly. If not, Lite Bar starts `lite-bar.exe --startup on` (or `off`) as administrator, so you see one UAC prompt. That process changes the task and exits. Declining the prompt leaves everything as it was. After that, every sign-in starts Lite Bar elevated with no prompt.
- The task starts the `lite-bar.exe` it was created from. **Update now** replaces that file in place, so the task keeps working after updates. If you move `lite-bar.exe`, the switch shows "On (points to another copy)". Turn it on again to point the task at the copy you're running.
- To remove the task by hand, run this from an administrator prompt: `schtasks /Delete /TN "Lite Bar" /F`.

### Files

| File | Path |
| --- | --- |
| Settings | `%APPDATA%\LiteBar\settings.toml` |
| Log | `%APPDATA%\LiteBar\lite-bar.log` |
| Game stats | `%APPDATA%\LiteBar\stats\` (see [Stats and history](#stats-and-history)) |

The settings file is created with the default values and a comment for every key on first run. Afterwards Lite Bar only rewrites the values you change in the settings window. An invalid value falls back to its default and a warning is written to the log. Unknown keys are ignored.

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

`text_size` used to set the size of the FPS number. That is now `counter_size`, and `text_size` sizes only the "FPS" label and the statistics, so an old `text_size = 18` makes them as big as the number. Use **Restore defaults** in the settings window, or set `text_size = 11`.

`[hotkeys]`

| Key | Type and values | Default |
| --- | --- | --- |
| `toggle_overlay` | string, e.g. `"Ctrl+Alt+Shift+L"` | `"Ctrl+Alt+Shift+L"` |

Hotkey syntax: zero or more modifiers (`Ctrl`, `Alt`, `Shift`, `Win`) and exactly one key (`A`-`Z`, `0`-`9` or `F1`-`F24`), joined with `+`, case-insensitive. For letter and digit keys at least one of `Ctrl`, `Alt` or `Win` is required (`Shift` alone would stop you typing capital letters). Function keys also accept `Shift` alone, e.g. `Shift+F5`. If the combination is already taken by another program, the tray menu shows the hotkey as unavailable.

`[colors]`

| Key | Type and values | Default |
| --- | --- | --- |
| `saved` | list of `"#RRGGBB"` colors saved in the settings window's color picker, at most 16 (extra and invalid entries are ignored with a warning) | `[]` |

`[updates]`

| Key | Type and values | Default |
| --- | --- | --- |
| `check` | bool; check GitHub for a newer release about 30 s after start and then every 24 h. Nothing is downloaded or installed until you click **Update now** | `true` |

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

To rename a game, type a name in the settings window's **Game names** section, or add it to `[game_names]` in the settings file and choose **Reload settings**. Either way `games.csv` is regenerated with the new name, and new `sessions.csv` rows use it (existing rows keep the old name).

Anti-cheat: Lite Bar never injects into, hooks or reads the memory of a game. Its only contact with a game's process is `OpenProcess` with `PROCESS_QUERY_LIMITED_INFORMATION` to read the exe path, the start time and whether the process has exited, and the handle is closed straight away. Lite Bar queries a recorded game's process when its session starts and then about once a second only while the game has stopped presenting frames; while it presents, that alone shows it is running, and it is queried at most every 30 s. The window title comes from `GetWindowTextW`, once, when the session starts. Everything else reads files and the registry, with no network lookups. If a game's process refuses even that limited query, its live stats still show on the overlay, but it isn't recorded in the history (a warning goes to the log). Lite Bar asks such a process at most once each time it gets the focus.

### Known limitations

- Exclusive fullscreen games may hide the overlay. Use borderless or windowed fullscreen.
- FPS needs administrator rights in M1. The tray menu offers **Restart as administrator** when capture cannot start.
- The FPS value lags by roughly 0.5-1 s because of ETW buffering.
- A graceful `taskkill` (without `/F`) does not close the app. Use the tray icon's **Quit**.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this project by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
