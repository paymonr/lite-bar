# Lite Bar

A small Windows tray app that shows your FPS in a corner of the screen, with average, 1% low and 1% high, and keeps a history of every game you play.

## Install

1. Download `lite-bar.exe` from the [latest release](https://github.com/paymonr/lite-bar/releases/latest) and run it.
2. The welcome popup offers to install Lite Bar to `C:\Program Files\Lite Bar`. Click **Install to Program Files** (one admin prompt). Lite Bar then shows up in the Start menu and in Installed apps.

If you chose **No thanks**, you can install later from **Settings → General → Install to Program Files**.

Lite Bar needs administrator rights to read FPS. If it can't, the tray menu offers **Restart as administrator**. Turn on **Start with Windows** in Settings to have it start elevated at sign-in without a prompt.

## Using it

Lite Bar lives in the system tray. Double-click the tray icon to open **Settings**, or right-click it for the menu. `Ctrl+Alt+Shift+L` shows or hides the overlay.

**Settings** changes take effect right away, with a live preview:

- **Overlay**: where the counter sits and whether it shows the extra stats.
- **Appearance**: colors, background opacity and sizes.
- **General**: Start with Windows, and Install to Program Files.
- **Hotkeys**: click the field and press a new combination.
- **Updates**: check for new versions and install them with **Update now**.
- **Restore defaults** (click twice) resets the overlay and hotkey.

Everything is also in `%APPDATA%\LiteBar\settings.toml`, with a comment explaining every setting. After editing it by hand, choose **Reload settings** from the tray menu.

## Games

The **Games** page in Settings shows:

- the game you're playing, with this session's average FPS, 1% low and play time next to its all-time numbers (or your last session when you're not playing);
- a table of every game with sessions, play time, last played, average FPS and 1% low.

Click a game's name to rename it. If something that isn't a game got recorded (a terminal, a video call), click **Not a game** to move it to **Other apps**. **It's a game** puts it back; nothing is deleted.

A game is recorded once you've played it for at least a minute. Browsers, launchers, Discord, OBS and other common apps are never recorded.

Your stats are saved in `%APPDATA%\LiteBar\stats\`. `games.csv` has one row per game and `sessions.csv` one row per play session; both open in Excel. **Open game stats** in the tray menu opens `games.csv`.

## Updates

Lite Bar checks GitHub for a new version once a day; this is its only network traffic. When one is out, Settings shows **Update now** and the tray menu shows **Update to v…**. Nothing is downloaded until you click it. The download is checked against its SHA-256 checksum before it replaces the running copy. You can turn off update checks in **Settings → Updates**.

## Uninstall

Uninstall Lite Bar from Windows **Settings → Apps → Installed apps**. It asks first, and only deletes your settings and stats if you tick **Also delete my settings and stats**.

If you're using the exe without installing it, run `lite-bar.exe --uninstall` to remove its startup entry (and, if you choose, your data), then delete the exe.

## Anti-cheat

Lite Bar never injects into, hooks or reads the memory of a game. It reads frame timings from Windows' own event tracing (ETW). It only touches a game's process to read its exe path, start time and whether it is still running.

## Known limitations

- Exclusive fullscreen games may hide the overlay. Use borderless or windowed fullscreen.
- The FPS number lags by about half a second to a second.

## Development

Lite Bar is written in Rust. It is developed in WSL2 and cross-compiled to Windows with `cargo-xwin`.

Setup:

- rustup with stable Rust, `rustup target add x86_64-pc-windows-msvc` and `rustup component add llvm-tools`
- `cargo install --locked cargo-xwin`
- `~/.local/bin/lld-link` symlinked to rust-lld

`scripts/xwin.sh` wraps `cargo xwin` with the environment this setup needs. Use pure-Rust dependencies only: crates with C build scripts won't build with this toolchain.

```bash
cargo test -p lite-bar-core                     # core logic, native Linux
scripts/xwin.sh test -p lite-bar                # Windows app, runs through WSL interop
scripts/xwin.sh build --release -p lite-bar     # target/x86_64-pc-windows-msvc/release/lite-bar.exe
```

To change the icon, edit and run `scripts/make-icon.py` (needs Pillow). It writes `crates/app/assets/lite-bar.ico`, which `crates/app/build.rs` builds into the exe.

CI checks pull requests on Windows with `cargo fmt`, `cargo clippy -D warnings` and `cargo test --workspace`.

**Releasing:** bump `version` in the root `Cargo.toml` and merge to `master`. The release workflow publishes `v<version>` with `lite-bar.exe` and `lite-bar.exe.sha256`, which the updater downloads. A merge without a version bump publishes nothing.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this project by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
