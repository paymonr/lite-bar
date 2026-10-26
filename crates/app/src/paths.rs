//! App data directory and the settings file.

use lite_bar_core::settings::{default_settings_toml, parse_settings, Settings};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

const SETTINGS_FILE: &str = "settings.toml";

/// `<base>\LiteBar`, created if missing. Never fails: if creation fails the
/// path is returned anyway and the settings loader reports the problem.
pub fn app_dir_in(base: &Path) -> PathBuf {
    let dir = base.join("LiteBar");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// `%APPDATA%\LiteBar` (falling back to the temp dir if APPDATA is unset).
pub fn app_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    app_dir_in(&base)
}

/// Loads `settings.toml` from `dir`. If the file is missing, writes the
/// commented defaults and returns them. An existing file is never written to.
pub fn load_or_create_settings(dir: &Path) -> (Settings, Vec<String>) {
    let path = dir.join(SETTINGS_FILE);
    match std::fs::read_to_string(&path) {
        Ok(text) => parse_settings(&text),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            let mut warnings = Vec::new();
            // create_new: never clobber a file that appeared in the meantime.
            let written = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .and_then(|mut f| f.write_all(default_settings_toml().as_bytes()));
            if let Err(e) = written {
                warnings.push(format!(
                    "could not write defaults to {}: {e}",
                    path.display()
                ));
            }
            (Settings::default(), warnings)
        }
        Err(e) => (
            Settings::default(),
            vec![format!("could not read {}: {e}", path.display())],
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lite_bar_core::settings::Settings;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Unique temp directory, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let p = std::env::temp_dir().join(format!("litebar-test-{}-{n}", std::process::id()));
            std::fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
    }

    impl std::ops::Deref for TempDir {
        type Target = Path;
        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn creates_defaults_when_missing() {
        let d = TempDir::new();
        let (s, w) = load_or_create_settings(&d);
        assert_eq!(s, Settings::default());
        assert!(w.is_empty());
        assert!(d.join("settings.toml").exists());
    }

    #[test]
    fn never_overwrites_existing_file() {
        let d = TempDir::new();
        let p = d.join("settings.toml");
        std::fs::write(&p, "garbage [[[").unwrap();
        let (_, w) = load_or_create_settings(&d);
        assert_eq!(w.len(), 1);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "garbage [[[");
    }

    #[test]
    fn warns_with_path_when_defaults_cannot_be_written() {
        let d = TempDir::new();
        let missing = d.join("no-such-dir");
        let (s, w) = load_or_create_settings(&missing);
        assert_eq!(s, Settings::default());
        assert_eq!(w.len(), 1);
        assert!(w[0].contains("settings.toml"), "warning was: {}", w[0]);
    }

    #[test]
    fn app_dir_in_creates_litebar_subdir() {
        let d = TempDir::new();
        let dir = app_dir_in(&d);
        assert_eq!(dir, d.join("LiteBar"));
        assert!(dir.is_dir());
    }

    #[test]
    fn app_dir_in_returns_path_even_when_creation_fails() {
        let d = TempDir::new();
        let file = d.join("blocker");
        std::fs::write(&file, "x").unwrap();
        // A file where a directory is needed: create_dir_all fails.
        assert_eq!(app_dir_in(&file), file.join("LiteBar"));
    }
}
