//! Writes `games.toml`, `games.csv` and `sessions.csv` into the stats directory.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use lite_bar_core::game_name::display_name;
use lite_bar_core::history::{
    games_csv, session_csv_row, GameStore, LocalTime, SessionRecord, CSV_BOM, SESSIONS_CSV_HEADER,
};
use windows::Win32::System::SystemInformation::GetLocalTime;

use crate::log;

const GAMES_TOML: &str = "games.toml";
const GAMES_CSV: &str = "games.csv";
const SESSIONS_CSV: &str = "sessions.csv";
const BAD_SUFFIX: &str = ".bad";

/// Current local date and time.
pub fn local_now() -> LocalTime {
    // SAFETY: no arguments; returns a plain struct.
    let t = unsafe { GetLocalTime() };
    LocalTime {
        year: t.wYear,
        month: t.wMonth as u8,
        day: t.wDay as u8,
        hour: t.wHour as u8,
        minute: t.wMinute as u8,
        second: t.wSecond as u8,
    }
}

/// Writes the three stats files. Work that fails (locked file, full disk) stays queued in
/// memory and is retried by the next `record` or `flush`.
pub struct StatsWriter {
    dir: PathBuf,
    /// Sessions not yet merged into `games.toml`.
    sessions: Vec<SessionRecord>,
    /// `sessions.csv` rows not yet appended.
    rows: Vec<String>,
    /// `games.csv` needs regenerating from `games.toml`.
    games_csv_dirty: bool,
}

impl StatsWriter {
    pub fn new(stats_dir: PathBuf) -> StatsWriter {
        StatsWriter {
            dir: stats_dir,
            sessions: Vec::new(),
            rows: Vec::new(),
            games_csv_dirty: false,
        }
    }

    pub fn stats_dir(&self) -> &Path {
        &self.dir
    }

    /// Stores a finished session in all three files (or queues whatever can't be written now).
    pub fn record(&mut self, s: &SessionRecord, overrides: &BTreeMap<String, String>) {
        let display = display_name(&s.name, &s.exe_path, overrides);
        self.rows.push(session_csv_row(s, &display));
        self.sessions.push(s.clone());
        self.flush(overrides);
    }

    /// Retries queued work.
    pub fn flush(&mut self, overrides: &BTreeMap<String, String>) {
        if self.sessions.is_empty() && self.rows.is_empty() && !self.games_csv_dirty {
            return;
        }
        if let Err(e) = std::fs::create_dir_all(&self.dir) {
            log::error(&format!("stats: cannot create {}: {e}", self.dir.display()));
        }
        let mut store = None;
        if !self.sessions.is_empty() {
            store = self.merge_sessions();
        }
        if self.sessions.is_empty() && (self.games_csv_dirty || store.is_some()) {
            self.write_games_csv(store, overrides);
        }
        if !self.rows.is_empty() {
            self.append_rows();
        }
    }

    /// Merges queued sessions into `games.toml`. On success the queue is emptied, `games.csv`
    /// is marked dirty and the new store is returned.
    fn merge_sessions(&mut self) -> Option<GameStore> {
        let mut store = self.load_store()?;
        for s in &self.sessions {
            store.add_session(s);
        }
        match write_atomic(&self.dir.join(GAMES_TOML), store.to_toml().as_bytes()) {
            Ok(()) => {
                self.sessions.clear();
                self.games_csv_dirty = true;
                Some(store)
            }
            Err(e) => {
                log::error(&format!("stats: cannot write {GAMES_TOML}: {e}"));
                None
            }
        }
    }

    fn write_games_csv(&mut self, store: Option<GameStore>, overrides: &BTreeMap<String, String>) {
        let Some(store) = store.or_else(|| self.load_store()) else {
            return;
        };
        let csv = games_csv(&store, overrides);
        match write_atomic(&self.dir.join(GAMES_CSV), csv.as_bytes()) {
            Ok(()) => self.games_csv_dirty = false,
            Err(e) => log::error(&format!("stats: cannot write {GAMES_CSV}: {e}")),
        }
    }

    fn append_rows(&mut self) {
        let path = self.dir.join(SESSIONS_CSV);
        let result = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut file| {
                let mut text = String::new();
                if file.metadata()?.len() == 0 {
                    text.push_str(CSV_BOM);
                    text.push_str(SESSIONS_CSV_HEADER);
                    text.push_str("\r\n");
                }
                text.extend(self.rows.iter().map(String::as_str));
                file.write_all(text.as_bytes())
            });
        match result {
            Ok(()) => self.rows.clear(),
            Err(e) => log::error(&format!("stats: cannot append to {SESSIONS_CSV}: {e}")),
        }
    }

    /// Loads `games.toml`: missing means empty. A file that can't be parsed at all is moved to
    /// `games.toml.bad` and the store starts empty. `None` when the file can't be safely read or
    /// set aside (it must not be overwritten then).
    fn load_store(&self) -> Option<GameStore> {
        let path = self.dir.join(GAMES_TOML);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Some(GameStore::default()),
            Err(e) => {
                log::error(&format!("stats: cannot read {GAMES_TOML}: {e}"));
                return None;
            }
        };
        let parsed = String::from_utf8(bytes)
            .map_err(|e| e.to_string())
            .and_then(|text| GameStore::parse(text.strip_prefix(CSV_BOM).unwrap_or(&text)));
        match parsed {
            Ok((store, warnings)) => {
                for warning in warnings {
                    log::warn(&format!(
                        "stats: {GAMES_TOML}: {warning} (entry kept as is)"
                    ));
                }
                Some(store)
            }
            Err(reason) => {
                let bad = with_suffix(&path, BAD_SUFFIX);
                match std::fs::rename(&path, &bad) {
                    Ok(()) => {
                        log::warn(&format!(
                            "stats: {GAMES_TOML} is unreadable ({reason}); moved to {}",
                            bad.display()
                        ));
                        Some(GameStore::default())
                    }
                    Err(e) => {
                        log::error(&format!(
                            "stats: {GAMES_TOML} is unreadable ({reason}) and cannot be moved aside: {e}"
                        ));
                        None
                    }
                }
            }
        }
    }
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = OsString::from(path.as_os_str());
    name.push(suffix);
    PathBuf::from(name)
}

/// Writes `<path>.tmp` then renames it over `path` (which replaces an existing file).
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = with_suffix(path, ".tmp");
    let result = std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;
    use lite_bar_core::frame_stats::FrameHistogram;
    use lite_bar_core::history::{CSV_BOM, SESSIONS_CSV_HEADER};
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;

    fn time(h: u8, m: u8) -> LocalTime {
        LocalTime {
            year: 2026,
            month: 10,
            day: 26,
            hour: h,
            minute: m,
            second: 0,
        }
    }

    fn session(key: &str, name: &str, start: LocalTime, end: LocalTime) -> SessionRecord {
        let mut hist = FrameHistogram::new();
        for _ in 0..600 {
            hist.add(1.0 / 100.0);
        }
        SessionRecord {
            start,
            end,
            key: key.to_string(),
            name: name.to_string(),
            exe_path: format!(r"C:\Games\{name}.exe"),
            hist,
        }
    }

    fn no_overrides() -> BTreeMap<String, String> {
        BTreeMap::new()
    }

    fn read(dir: &Path, file: &str) -> String {
        std::fs::read_to_string(dir.join(file)).unwrap()
    }

    #[test]
    fn local_now_is_a_plausible_date() {
        let now = local_now();
        assert!(now.year >= 2024, "{now:?}");
        assert!((1..=12).contains(&now.month) && (1..=31).contains(&now.day));
    }

    #[test]
    fn record_creates_all_three_files() {
        let tmp = TempDir::new("stats-create");
        let dir = tmp.path().join("nested").join("stats");
        let mut writer = StatsWriter::new(dir.clone());
        assert_eq!(writer.stats_dir(), dir.as_path());
        writer.record(
            &session("alpha", "Alpha", time(10, 0), time(10, 10)),
            &no_overrides(),
        );

        let toml = read(&dir, "games.toml");
        assert!(toml.contains("alpha") && toml.contains("Alpha"), "{toml}");
        let games = read(&dir, "games.csv");
        assert!(
            games.starts_with(CSV_BOM) && games.contains("Alpha"),
            "{games}"
        );
        let sessions = read(&dir, "sessions.csv");
        assert!(
            sessions.starts_with(&format!("{CSV_BOM}{SESSIONS_CSV_HEADER}")),
            "{sessions}"
        );
        assert_eq!(sessions.matches(",Alpha,").count(), 1);
        assert!(!dir.join("games.toml.tmp").exists());
        assert!(!dir.join("games.csv.tmp").exists());
    }

    #[test]
    fn second_session_merges_into_games_toml() {
        let tmp = TempDir::new("stats-merge");
        let dir = tmp.path().to_path_buf();
        let mut writer = StatsWriter::new(dir.clone());
        writer.record(
            &session("alpha", "Alpha", time(10, 0), time(10, 10)),
            &no_overrides(),
        );
        // A new writer (as after an app restart) must continue from the file on disk.
        let mut writer = StatsWriter::new(dir.clone());
        writer.record(
            &session("alpha", "Alpha", time(11, 0), time(11, 10)),
            &no_overrides(),
        );

        let (store, warnings) =
            lite_bar_core::history::GameStore::parse(&read(&dir, "games.toml")).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(store.games.len(), 1);
        assert_eq!(store.games["alpha"].sessions, 2);
        let sessions = read(&dir, "sessions.csv");
        assert_eq!(sessions.matches(SESSIONS_CSV_HEADER).count(), 1);
        assert_eq!(sessions.lines().count(), 3);
        assert_eq!(read(&dir, "games.csv").lines().count(), 2);
    }

    #[test]
    fn overrides_rename_the_display_name_in_csv_files() {
        let tmp = TempDir::new("stats-override");
        let dir = tmp.path().to_path_buf();
        let mut writer = StatsWriter::new(dir.clone());
        let mut overrides = BTreeMap::new();
        overrides.insert("Alpha.exe".to_string(), "Renamed".to_string());
        writer.record(
            &session("alpha", "Alpha", time(10, 0), time(10, 10)),
            &overrides,
        );
        assert!(read(&dir, "games.csv").contains("Renamed"));
        assert!(read(&dir, "sessions.csv").contains("Renamed"));
        assert!(read(&dir, "games.toml").contains("Alpha"));
    }

    #[test]
    fn locked_sessions_csv_keeps_row_pending() {
        let tmp = TempDir::new("stats-locked");
        let dir = tmp.path().to_path_buf();
        let csv = dir.join("sessions.csv");
        let mut writer = StatsWriter::new(dir.clone());

        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .share_mode(0)
            .open(&csv)
            .unwrap();
        writer.record(
            &session("alpha", "Alpha", time(10, 0), time(10, 10)),
            &no_overrides(),
        );
        writer.record(
            &session("beta", "Beta", time(11, 0), time(11, 10)),
            &no_overrides(),
        );
        // games.toml and games.csv are independent of the locked file.
        assert!(read(&dir, "games.toml").contains("beta"));
        drop(lock);
        assert_eq!(std::fs::metadata(&csv).unwrap().len(), 0);

        writer.flush(&no_overrides());
        let sessions = read(&dir, "sessions.csv");
        assert!(sessions.starts_with(&format!("{CSV_BOM}{SESSIONS_CSV_HEADER}")));
        assert_eq!(sessions.matches(SESSIONS_CSV_HEADER).count(), 1);
        assert_eq!(sessions.matches(",Alpha,").count(), 1);
        assert_eq!(sessions.matches(",Beta,").count(), 1);

        writer.flush(&no_overrides());
        assert_eq!(
            read(&dir, "sessions.csv"),
            sessions,
            "a second flush adds nothing"
        );
    }

    #[test]
    fn locked_games_csv_is_regenerated_on_flush() {
        let tmp = TempDir::new("stats-locked-games");
        let dir = tmp.path().to_path_buf();
        let mut writer = StatsWriter::new(dir.clone());
        writer.record(
            &session("alpha", "Alpha", time(10, 0), time(10, 10)),
            &no_overrides(),
        );

        // Replacing a file another process holds open without delete sharing fails.
        let lock = OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(dir.join("games.csv"))
            .unwrap();
        writer.record(
            &session("beta", "Beta", time(11, 0), time(11, 10)),
            &no_overrides(),
        );
        assert!(!read(&dir, "games.csv").contains("Beta"));
        drop(lock);

        writer.flush(&no_overrides());
        let games = read(&dir, "games.csv");
        assert!(games.contains("Alpha") && games.contains("Beta"), "{games}");
        assert_eq!(read(&dir, "sessions.csv").matches(",Beta,").count(), 1);
    }

    #[test]
    fn corrupt_store_is_moved_aside() {
        let tmp = TempDir::new("stats-corrupt");
        let dir = tmp.path().to_path_buf();
        std::fs::write(dir.join("games.toml"), "games = [ not toml").unwrap();
        std::fs::write(dir.join("games.toml.bad"), "older backup").unwrap();
        let mut writer = StatsWriter::new(dir.clone());
        writer.record(
            &session("alpha", "Alpha", time(10, 0), time(10, 10)),
            &no_overrides(),
        );

        assert_eq!(read(&dir, "games.toml.bad"), "games = [ not toml");
        let (store, _) =
            lite_bar_core::history::GameStore::parse(&read(&dir, "games.toml")).unwrap();
        assert_eq!(store.games.len(), 1);
    }

    #[test]
    fn unreadable_entries_are_kept_without_a_backup() {
        let tmp = TempDir::new("stats-unreadable");
        let dir = tmp.path().to_path_buf();
        std::fs::write(dir.join("games.toml"), "[games.odd]\nname = 5\n").unwrap();
        let mut writer = StatsWriter::new(dir.clone());
        writer.record(
            &session("alpha", "Alpha", time(10, 0), time(10, 10)),
            &no_overrides(),
        );

        assert!(!dir.join("games.toml.bad").exists());
        let toml = read(&dir, "games.toml");
        assert!(toml.contains("odd") && toml.contains("alpha"), "{toml}");
    }
}
