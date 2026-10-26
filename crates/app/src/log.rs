//! Minimal append-only log file. Never panics and never shows anything to the user.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

const LOG_FILE: &str = "lite-bar.log";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Error,
}

impl Level {
    fn as_str(self) -> &'static str {
        match self {
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
        }
    }
}

static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Directs later `info`/`warn`/`error` calls to `<dir>\lite-bar.log`. Call once at startup.
pub fn init(dir: &Path) {
    let _ = LOG_PATH.set(dir.join(LOG_FILE));
}

pub fn info(msg: &str) {
    write(Level::Info, msg);
}

pub fn warn(msg: &str) {
    write(Level::Warn, msg);
}

pub fn error(msg: &str) {
    write(Level::Error, msg);
}

fn write(level: Level, msg: &str) {
    if let Some(path) = LOG_PATH.get() {
        append_to(path, level, msg);
    }
}

/// One log line: Unix-epoch seconds, level, message (newlines flattened), trailing newline.
fn format_line(epoch_secs: u64, level: Level, msg: &str) -> String {
    format!(
        "{epoch_secs} {} {}\n",
        level.as_str(),
        msg.replace(['\r', '\n'], " ")
    )
}

/// Appends a line to `path`. Failures are ignored: logging must never break the app.
fn append_to(path: &Path, level: Level, msg: &str) {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let line = format_line(secs, level, msg);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = f.write_all(line.as_bytes());
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_line_has_timestamp_level_and_message() {
        assert_eq!(
            format_line(1_700_000_000, Level::Warn, "hello"),
            "1700000000 WARN hello\n"
        );
    }

    #[test]
    fn newlines_in_message_are_flattened() {
        assert_eq!(format_line(1, Level::Info, "a\nb"), "1 INFO a b\n");
    }

    #[test]
    fn append_adds_lines_and_survives_unwritable_path() {
        let dir = std::env::temp_dir().join(format!("litebar-log-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.log");
        append_to(&path, Level::Info, "one");
        append_to(&path, Level::Error, "two");
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 2);
        assert!(text.lines().next().unwrap().ends_with(" INFO one"));
        assert!(text.lines().nth(1).unwrap().ends_with(" ERROR two"));
        // A directory where the file should be: must not panic.
        append_to(&dir, Level::Info, "ignored");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
