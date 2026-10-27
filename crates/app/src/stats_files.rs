//! Writes `games.toml`, `games.csv` and `sessions.csv` into the stats directory.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use lite_bar_core::game_name::display_name;
use lite_bar_core::history::{
    games_csv, parse_pending, pending_toml, session_csv_row, GameStore, LocalTime, SessionRecord,
    CSV_BOM, SESSIONS_CSV_HEADER,
};
use windows::Win32::System::SystemInformation::GetLocalTime;

use crate::log;

const GAMES_TOML: &str = "games.toml";
const GAMES_CSV: &str = "games.csv";
const SESSIONS_CSV: &str = "sessions.csv";
const PENDING_TOML: &str = "pending.toml";

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

/// What `pending.toml` holds, as far as this writer knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PendingFile {
    /// Nothing of ours: no file, or it was deleted once its work was written.
    None,
    /// Our queue, as of the last flush; rewritten or deleted after each flush.
    Ours,
    /// A file that could be neither read nor moved aside: never overwritten or deleted.
    Blocked,
}

/// Writes the three stats files. Work that fails (locked file, full disk) stays queued in
/// memory and is retried by the next `record` or `flush`. Work still queued at quit is saved
/// to `pending.toml` and queued again by the next `new`.
pub struct StatsWriter {
    dir: PathBuf,
    /// Sessions not yet merged into `games.toml`.
    sessions: Vec<SessionRecord>,
    /// `sessions.csv` rows not yet appended.
    rows: Vec<String>,
    /// `games.csv` needs regenerating from `games.toml`.
    games_csv_dirty: bool,
    pending: PendingFile,
}

impl StatsWriter {
    /// Queues the work saved in `pending.toml` by an earlier run, if any; the next `flush`
    /// writes it.
    pub fn new(stats_dir: PathBuf) -> StatsWriter {
        let mut writer = StatsWriter {
            dir: stats_dir,
            sessions: Vec::new(),
            rows: Vec::new(),
            games_csv_dirty: false,
            pending: PendingFile::None,
        };
        writer.load_pending();
        writer
    }

    fn load_pending(&mut self) {
        let path = self.dir.join(PENDING_TOML);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(e) => {
                log::error(&format!("stats: cannot read {PENDING_TOML}: {e}"));
                self.pending = PendingFile::Blocked;
                return;
            }
        };
        let parsed = String::from_utf8(bytes)
            .map_err(|e| e.to_string())
            .and_then(|text| parse_pending(&text));
        match parsed {
            Ok((sessions, rows)) => {
                log::info(&format!(
                    "stats: {PENDING_TOML}: retrying {} sessions and {} rows from an earlier run",
                    sessions.len(),
                    rows.len()
                ));
                self.sessions.extend(sessions);
                self.rows.extend(rows);
                self.pending = PendingFile::Ours;
            }
            Err(reason) => {
                if !move_aside(&path, PENDING_TOML, &reason) {
                    self.pending = PendingFile::Blocked;
                }
            }
        }
    }

    /// Makes the next `flush` regenerate `games.csv` (e.g. after the name overrides changed).
    pub fn mark_games_csv_dirty(&mut self) {
        self.games_csv_dirty = true;
    }

    pub fn stats_dir(&self) -> &Path {
        &self.dir
    }

    /// What "Open game stats" opens: `games.csv`, or the stats folder (created if missing)
    /// while there is no CSV yet.
    pub fn open_target(&self) -> std::io::Result<PathBuf> {
        let csv = self.dir.join(GAMES_CSV);
        if csv.is_file() {
            return Ok(csv);
        }
        std::fs::create_dir_all(&self.dir)?;
        Ok(self.dir.clone())
    }

    /// Stores a finished session in all three files (or queues whatever can't be written now).
    pub fn record(&mut self, s: &SessionRecord, overrides: &BTreeMap<String, String>) {
        let display = display_name(&s.name, &s.exe_path, overrides);
        self.rows.push(session_csv_row(s, &display));
        self.sessions.push(s.clone());
        self.flush(overrides);
    }

    /// The last flush before quit or sign-out: whatever is still queued is saved to
    /// `pending.toml` so it is retried after a restart.
    pub fn shutdown_flush(&mut self, overrides: &BTreeMap<String, String>) {
        self.flush(overrides);
        if self.sessions.is_empty() && self.rows.is_empty() {
            return;
        }
        if self.pending == PendingFile::Blocked {
            log::error(&format!(
                "stats: {} sessions and {} rows are lost: {PENDING_TOML} can't be replaced",
                self.sessions.len(),
                self.rows.len()
            ));
            return;
        }
        if self.write_pending() {
            log::warn(&format!(
                "stats: {} sessions and {} rows saved to {PENDING_TOML} for the next start",
                self.sessions.len(),
                self.rows.len()
            ));
        }
    }

    /// Retries queued work.
    pub fn flush(&mut self, overrides: &BTreeMap<String, String>) {
        self.write_queued(overrides);
        self.sync_pending();
    }

    fn write_queued(&mut self, overrides: &BTreeMap<String, String>) {
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

    /// Keeps our `pending.toml` equal to the queue, so work written since it was loaded is
    /// never queued again by a later start; deletes it once the queue is empty.
    fn sync_pending(&mut self) {
        if self.pending != PendingFile::Ours {
            return;
        }
        if !self.sessions.is_empty() || !self.rows.is_empty() {
            self.write_pending();
            return;
        }
        match std::fs::remove_file(self.dir.join(PENDING_TOML)) {
            Ok(()) => self.pending = PendingFile::None,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => self.pending = PendingFile::None,
            Err(e) => log::error(&format!("stats: cannot delete {PENDING_TOML}: {e}")),
        }
    }

    /// Saves the queue to `pending.toml`. Returns whether it worked.
    fn write_pending(&mut self) -> bool {
        let text = pending_toml(&self.sessions, &self.rows);
        match write_atomic(&self.dir.join(PENDING_TOML), text.as_bytes()) {
            Ok(()) => {
                self.pending = PendingFile::Ours;
                true
            }
            Err(e) => {
                log::error(&format!("stats: cannot write {PENDING_TOML}: {e}"));
                false
            }
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
                file.write_all(text.as_bytes())?;
                // On disk before `sync_pending` rewrites or deletes pending.toml.
                file.sync_all()
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
            Err(reason) => move_aside(&path, GAMES_TOML, &reason).then(GameStore::default),
        }
    }
}

/// Moves an unreadable file to the first free `.bad`, `.bad2`, ... name. Returns whether it
/// worked; the file must not be overwritten otherwise.
fn move_aside(path: &Path, name: &str, reason: &str) -> bool {
    let bad = free_backup_path(path);
    match std::fs::rename(path, &bad) {
        Ok(()) => {
            log::warn(&format!(
                "stats: {name} is unreadable ({reason}); moved to {}",
                bad.display()
            ));
            true
        }
        Err(e) => {
            log::error(&format!(
                "stats: {name} is unreadable ({reason}) and cannot be moved aside: {e}"
            ));
            false
        }
    }
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = OsString::from(path.as_os_str());
    name.push(suffix);
    PathBuf::from(name)
}

/// First of `<path>.bad`, `<path>.bad2`, `<path>.bad3`, ... that doesn't exist yet.
fn free_backup_path(path: &Path) -> PathBuf {
    let first = with_suffix(path, ".bad");
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|n| with_suffix(path, &format!(".bad{n}")))
        .find(|candidate| !candidate.exists())
        .expect("an unused backup name exists")
}

/// Writes `<path>.tmp`, flushes it to disk, then renames it over `path` (which replaces an
/// existing file), so a crash or power loss leaves either the old or the complete new file.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = with_suffix(path, ".tmp");
    let write = || -> std::io::Result<()> {
        let mut file = File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()
    };
    let result = write().and_then(|()| std::fs::rename(&tmp, path));
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
    fn open_target_is_the_created_folder_until_games_csv_exists() {
        let tmp = TempDir::new("stats-open-target");
        let dir = tmp.path().join("stats");
        let mut writer = StatsWriter::new(dir.clone());
        assert_eq!(writer.open_target().unwrap(), dir);
        assert!(dir.is_dir(), "the folder is created so it can be opened");

        writer.record(
            &session("alpha", "Alpha", time(10, 0), time(10, 10)),
            &no_overrides(),
        );
        assert_eq!(writer.open_target().unwrap(), dir.join("games.csv"));
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
    fn corrupt_store_is_moved_aside_without_replacing_older_backups() {
        let tmp = TempDir::new("stats-corrupt");
        let dir = tmp.path().to_path_buf();
        std::fs::write(dir.join("games.toml"), "games = [ not toml").unwrap();
        std::fs::write(dir.join("games.toml.bad"), "older backup").unwrap();
        let mut writer = StatsWriter::new(dir.clone());
        writer.record(
            &session("alpha", "Alpha", time(10, 0), time(10, 10)),
            &no_overrides(),
        );

        assert_eq!(read(&dir, "games.toml.bad"), "older backup");
        assert_eq!(read(&dir, "games.toml.bad2"), "games = [ not toml");
        let (store, _) =
            lite_bar_core::history::GameStore::parse(&read(&dir, "games.toml")).unwrap();
        assert_eq!(store.games.len(), 1);

        // Corrupted again: the next free name is used and every backup survives.
        std::fs::write(dir.join("games.toml"), "again = = broken").unwrap();
        writer.record(
            &session("beta", "Beta", time(11, 0), time(11, 10)),
            &no_overrides(),
        );
        assert_eq!(read(&dir, "games.toml.bad"), "older backup");
        assert_eq!(read(&dir, "games.toml.bad2"), "games = [ not toml");
        assert_eq!(read(&dir, "games.toml.bad3"), "again = = broken");
    }

    #[test]
    fn locked_games_toml_is_never_overwritten_and_sessions_stay_pending() {
        let tmp = TempDir::new("stats-locked-toml");
        let dir = tmp.path().to_path_buf();
        let toml = dir.join("games.toml");
        let mut writer = StatsWriter::new(dir.clone());
        writer.record(
            &session("alpha", "Alpha", time(10, 0), time(10, 10)),
            &no_overrides(),
        );
        let before = std::fs::read(&toml).unwrap();

        let lock = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&toml)
            .unwrap();
        writer.record(
            &session("beta", "Beta", time(11, 0), time(11, 10)),
            &no_overrides(),
        );
        drop(lock);
        assert_eq!(
            std::fs::read(&toml).unwrap(),
            before,
            "must not be replaced"
        );
        assert!(!dir.join("games.toml.bad").exists());

        writer.flush(&no_overrides());
        let (store, warnings) =
            lite_bar_core::history::GameStore::parse(&read(&dir, "games.toml")).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(store.games.len(), 2, "old and new sessions are both stored");
        let games = read(&dir, "games.csv");
        assert!(games.contains("Alpha") && games.contains("Beta"), "{games}");
        let sessions = read(&dir, "sessions.csv");
        assert_eq!(sessions.matches(",Alpha,").count(), 1);
        assert_eq!(sessions.matches(",Beta,").count(), 1);
    }

    #[test]
    fn marking_games_csv_dirty_regenerates_it_on_flush() {
        let tmp = TempDir::new("stats-dirty");
        let dir = tmp.path().to_path_buf();
        let mut writer = StatsWriter::new(dir.clone());
        writer.record(
            &session("alpha", "Alpha", time(10, 0), time(10, 10)),
            &no_overrides(),
        );
        assert!(!read(&dir, "games.csv").contains("Renamed"));
        let sessions_before = read(&dir, "sessions.csv");

        let mut overrides = BTreeMap::new();
        overrides.insert("Alpha.exe".to_string(), "Renamed".to_string());
        writer.flush(&overrides);
        assert!(
            !read(&dir, "games.csv").contains("Renamed"),
            "clean flush is a no-op"
        );

        writer.mark_games_csv_dirty();
        writer.flush(&overrides);
        assert!(read(&dir, "games.csv").contains("Renamed"));
        assert_eq!(read(&dir, "sessions.csv"), sessions_before);
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

    fn lock_exclusive(path: &Path) -> File {
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .share_mode(0)
            .open(path)
            .unwrap()
    }

    fn stored(dir: &Path) -> lite_bar_core::history::GameStore {
        lite_bar_core::history::GameStore::parse(&read(dir, "games.toml"))
            .unwrap()
            .0
    }

    #[test]
    fn queued_work_survives_restart() {
        let tmp = TempDir::new("stats-pending-rows");
        let dir = tmp.path().to_path_buf();
        let lock = lock_exclusive(&dir.join("sessions.csv"));
        let mut writer = StatsWriter::new(dir.clone());
        writer.record(
            &session("alpha", "Alpha", time(10, 0), time(10, 10)),
            &no_overrides(),
        );
        writer.shutdown_flush(&no_overrides());
        assert!(dir.join("pending.toml").is_file(), "queued row is saved");
        drop(writer);
        drop(lock);

        let mut writer = StatsWriter::new(dir.clone());
        writer.flush(&no_overrides());
        assert_eq!(read(&dir, "sessions.csv").matches(",Alpha,").count(), 1);
        assert_eq!(stored(&dir).games["alpha"].sessions, 1);
        assert!(!dir.join("pending.toml").exists(), "deleted once written");
    }

    #[test]
    fn queued_session_survives_restart() {
        let tmp = TempDir::new("stats-pending-session");
        let dir = tmp.path().to_path_buf();
        std::fs::write(dir.join("games.toml"), "").unwrap();
        let toml_lock = lock_exclusive(&dir.join("games.toml"));
        let csv_lock = lock_exclusive(&dir.join("sessions.csv"));
        let mut writer = StatsWriter::new(dir.clone());
        writer.record(
            &session("beta", "Beta", time(11, 0), time(11, 10)),
            &no_overrides(),
        );
        writer.shutdown_flush(&no_overrides());
        drop(writer);
        drop(toml_lock);
        drop(csv_lock);
        assert_eq!(read(&dir, "games.toml"), "", "nothing was written yet");

        let mut writer = StatsWriter::new(dir.clone());
        writer.flush(&no_overrides());
        let store = stored(&dir);
        assert_eq!(store.games.len(), 1);
        assert_eq!(store.games["beta"].sessions, 1);
        assert_eq!(store.games["beta"].hist.frames(), 600);
        assert!(read(&dir, "games.csv").contains("Beta"));
        assert_eq!(read(&dir, "sessions.csv").matches(",Beta,").count(), 1);
        assert!(!dir.join("pending.toml").exists());
    }

    #[test]
    fn partly_written_pending_keeps_only_the_rest() {
        let tmp = TempDir::new("stats-pending-partial");
        let dir = tmp.path().to_path_buf();
        std::fs::write(dir.join("games.toml"), "").unwrap();
        let toml_lock = lock_exclusive(&dir.join("games.toml"));
        let csv_lock = lock_exclusive(&dir.join("sessions.csv"));
        let mut writer = StatsWriter::new(dir.clone());
        writer.record(
            &session("beta", "Beta", time(11, 0), time(11, 10)),
            &no_overrides(),
        );
        writer.shutdown_flush(&no_overrides());
        drop(writer);
        drop(toml_lock);

        // The store can be written now but sessions.csv is still locked.
        let mut writer = StatsWriter::new(dir.clone());
        writer.flush(&no_overrides());
        assert_eq!(stored(&dir).games["beta"].sessions, 1);
        drop(writer);
        drop(csv_lock);

        // The merged session must not be merged a second time.
        let mut writer = StatsWriter::new(dir.clone());
        writer.flush(&no_overrides());
        assert_eq!(stored(&dir).games["beta"].sessions, 1);
        assert_eq!(read(&dir, "sessions.csv").matches(",Beta,").count(), 1);
        assert!(!dir.join("pending.toml").exists());
    }

    #[test]
    fn clean_shutdown_writes_no_pending_file() {
        let tmp = TempDir::new("stats-pending-none");
        let dir = tmp.path().to_path_buf();
        let mut writer = StatsWriter::new(dir.clone());
        writer.record(
            &session("alpha", "Alpha", time(10, 0), time(10, 10)),
            &no_overrides(),
        );
        writer.shutdown_flush(&no_overrides());
        assert!(!dir.join("pending.toml").exists());
    }

    #[test]
    fn unparsable_pending_is_moved_aside() {
        let tmp = TempDir::new("stats-pending-bad");
        let dir = tmp.path().to_path_buf();
        std::fs::write(dir.join("pending.toml"), "rows = [ broken").unwrap();
        std::fs::write(dir.join("pending.toml.bad"), "older backup").unwrap();
        let mut writer = StatsWriter::new(dir.clone());
        assert!(!dir.join("pending.toml").exists());
        assert_eq!(read(&dir, "pending.toml.bad"), "older backup");
        assert_eq!(read(&dir, "pending.toml.bad2"), "rows = [ broken");

        writer.record(
            &session("alpha", "Alpha", time(10, 0), time(10, 10)),
            &no_overrides(),
        );
        assert_eq!(stored(&dir).games["alpha"].sessions, 1);
        assert_eq!(read(&dir, "pending.toml.bad2"), "rows = [ broken");
    }
}
