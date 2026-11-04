//! Per-game play history: the `games.toml` store and the `games.csv` / `sessions.csv` formats.
//! Pure formatting and parsing; the file IO lives in the app crate.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::frame_stats::{FrameHistogram, StatsSummary};
use crate::game_name::{display_name, game_key, is_generic_host};

/// Sessions with less counted play time than this are discarded.
pub const MIN_SESSION_SECS: f64 = 60.0;

/// UTF-8 byte order mark. Written once at the start of a newly created CSV file so Excel
/// shows non-ASCII names correctly.
pub const CSV_BOM: &str = "\u{feff}";

pub const SESSIONS_CSV_HEADER: &str =
    "start,end,game,play_time,frames,avg_fps,low_1pct_fps,high_1pct_fps,exe_path";
const GAMES_CSV_HEADER: &str =
    "game,sessions,play_time,first_played,last_played,avg_fps,low_1pct_fps,high_1pct_fps,exe_path";

/// `Default` is all zeros, only a filler (e.g. for rows built in tests).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct LocalTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

impl LocalTime {
    /// Parses `YYYY-MM-DD HH:MM:SS`.
    pub fn parse(text: &str) -> Option<LocalTime> {
        let (date, time) = text.split_once(' ')?;
        let mut date = date.split('-');
        let mut time = time.split(':');
        let year = date.next()?.parse().ok()?;
        let month = date.next()?.parse().ok()?;
        let day = date.next()?.parse().ok()?;
        let hour = time.next()?.parse().ok()?;
        let minute = time.next()?.parse().ok()?;
        let second = time.next()?.parse().ok()?;
        if date.next().is_some() || time.next().is_some() {
            return None;
        }
        let valid = (1..=12).contains(&month)
            && (1..=31).contains(&day)
            && hour < 24
            && minute < 60
            && second < 60;
        valid.then_some(LocalTime {
            year,
            month,
            day,
            hour,
            minute,
            second,
        })
    }
}

impl fmt::Display for LocalTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }
}

impl Serialize for LocalTime {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for LocalTime {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        LocalTime::parse(&text)
            .ok_or_else(|| serde::de::Error::custom(format!("invalid timestamp `{text}`")))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SessionRecord {
    pub start: LocalTime,
    pub end: LocalTime,
    pub key: String,
    pub name: String,
    pub exe_path: String,
    pub hist: FrameHistogram,
}

impl SessionRecord {
    /// Long enough and with enough frames for stats.
    pub fn is_kept(&self) -> bool {
        self.hist.total_secs() >= MIN_SESSION_SECS && self.hist.summary().is_some()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct GameRecord {
    pub name: String,
    pub exe_path: String,
    pub sessions: u32,
    pub first_played: LocalTime,
    pub last_played: LocalTime,
    pub hist: FrameHistogram,
    /// The newest session's numbers; `None` in entries written before they were kept.
    pub last_session: Option<LastSession>,
}

/// One session's numbers, kept per game for the Games page's "last session".
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LastSession {
    pub start: LocalTime,
    pub end: LocalTime,
    pub play_secs: f64,
    pub avg_fps: f64,
    pub low_1pct_fps: f64,
    pub high_1pct_fps: f64,
}

impl LastSession {
    /// `s`'s numbers, or `None` when it has too few frames for stats.
    fn of(s: &SessionRecord) -> Option<LastSession> {
        let sum = s.hist.summary()?;
        Some(LastSession {
            start: s.start,
            end: s.end,
            play_secs: sum.play_secs,
            avg_fps: sum.avg_fps,
            low_1pct_fps: sum.low_1pct_fps,
            high_1pct_fps: sum.high_1pct_fps,
        })
    }
}

/// On-disk shape of `last_session` in `games.toml`.
#[derive(Serialize, Deserialize)]
struct LastSessionFile {
    start: LocalTime,
    end: LocalTime,
    play_secs: f64,
    avg: f64,
    low_1pct: f64,
    high_1pct: f64,
}

impl From<LastSessionFile> for LastSession {
    fn from(f: LastSessionFile) -> Self {
        LastSession {
            start: f.start,
            end: f.end,
            play_secs: f.play_secs,
            avg_fps: f.avg,
            low_1pct_fps: f.low_1pct,
            high_1pct_fps: f.high_1pct,
        }
    }
}

impl From<&LastSession> for LastSessionFile {
    fn from(l: &LastSession) -> Self {
        LastSessionFile {
            start: l.start,
            end: l.end,
            play_secs: l.play_secs,
            avg: l.avg_fps,
            low_1pct: l.low_1pct_fps,
            high_1pct: l.high_1pct_fps,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GameStore {
    pub games: BTreeMap<String, GameRecord>,
    /// Raw values of `games.toml` entries that could not be read, kept so saving never drops them.
    pub unparsed: BTreeMap<String, toml::Value>,
}

/// A game entry, plus a warning when its optional `last_session` couldn't be read (the game
/// is kept without it: the field is only shown on the Games page).
fn parse_game(value: &toml::Value) -> Result<(GameRecord, Option<String>), String> {
    let g: GameIn = value
        .clone()
        .try_into()
        .map_err(|e: toml::de::Error| e.message().replace('\n', " "))?;
    if !g.total_secs.is_finite() || g.total_secs < 0.0 {
        return Err(format!(
            "total_secs {} is not a valid duration",
            g.total_secs
        ));
    }
    let (last_session, warning) = match g.last_session {
        None => (None, None),
        Some(raw) => match raw.try_into::<LastSessionFile>() {
            Ok(file) => (Some(LastSession::from(file)), None),
            Err(e) => (
                None,
                Some(format!(
                    "last_session: {}; ignored",
                    e.message().replace('\n', " ")
                )),
            ),
        },
    };
    let record = GameRecord {
        name: g.name,
        exe_path: g.exe_path,
        sessions: g.sessions,
        first_played: g.first_played,
        last_played: g.last_played,
        hist: FrameHistogram::from_sparse(&g.buckets, g.total_secs),
        last_session,
    };
    Ok((record, warning))
}

/// On-disk shape of one game in `games.toml`.
#[derive(Deserialize)]
struct GameIn {
    name: String,
    exe_path: String,
    sessions: u32,
    first_played: LocalTime,
    last_played: LocalTime,
    total_secs: f64,
    #[serde(default)]
    buckets: Vec<(u32, u64)>,
    /// Read leniently: a bad one is dropped with a warning, not the whole game.
    #[serde(default)]
    last_session: Option<toml::Value>,
}

#[derive(Serialize)]
struct GameOut<'a> {
    name: &'a str,
    exe_path: &'a str,
    sessions: u32,
    first_played: LocalTime,
    last_played: LocalTime,
    total_secs: f64,
    /// TOML integers are i64, so counts are clamped to `i64::MAX` (only reachable with
    /// corrupt data).
    buckets: Vec<(u32, i64)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_session: Option<LastSessionFile>,
}

#[derive(Serialize)]
struct FileOut {
    games: BTreeMap<String, toml::Value>,
}

impl GameStore {
    /// Errors on invalid TOML or a `games` value that isn't a table. A malformed game entry
    /// doesn't fail the file: it is reported in the returned warnings (naming the key and the
    /// reason) and its raw value is kept in `unparsed`, so saving never loses it. Unknown
    /// fields are ignored.
    pub fn parse(text: &str) -> Result<(GameStore, Vec<String>), String> {
        let root: toml::Table = text.parse().map_err(|e: toml::de::Error| e.to_string())?;
        let entries = match root.get("games") {
            None => return Ok((GameStore::default(), Vec::new())),
            Some(toml::Value::Table(entries)) => entries,
            Some(_) => return Err("`games` must be a table".to_string()),
        };
        let mut store = GameStore::default();
        let mut warnings = Vec::new();
        for (key, value) in entries {
            match parse_game(value) {
                Ok((record, warning)) => {
                    if let Some(w) = warning {
                        warnings.push(format!("games.{key}: {w}"));
                    }
                    store.games.insert(key.clone(), record);
                }
                Err(reason) => {
                    warnings.push(format!("games.{key}: {reason}"));
                    store.unparsed.insert(key.clone(), value.clone());
                }
            }
        }
        Ok((store, warnings))
    }

    pub fn to_toml(&self) -> String {
        // Unreadable entries first so a parsed entry would win on a (never expected) key clash.
        let mut games = self.unparsed.clone();
        for (key, g) in &self.games {
            let out = GameOut {
                name: &g.name,
                exe_path: &g.exe_path,
                sessions: g.sessions,
                first_played: g.first_played,
                last_played: g.last_played,
                total_secs: g.hist.total_secs(),
                buckets: toml_buckets(&g.hist),
                last_session: g.last_session.as_ref().map(LastSessionFile::from),
            };
            let value = toml::Value::try_from(out).expect("game entry converts to a TOML value");
            games.insert(key.clone(), value);
        }
        // Panicking beats returning an empty string that the caller would write over the history.
        toml::to_string(&FileOut { games }).expect("games.toml serialization cannot fail")
    }

    /// Adds a finished session. `name` and `exe_path` follow the newest session (by end time).
    /// If `unparsed` holds an entry for this key it moves to `<key>#unreadable` and the session
    /// starts a fresh record.
    pub fn add_session(&mut self, s: &SessionRecord) {
        // An unreadable entry under this key is set aside (never overwritten) so both survive.
        if let Some(raw) = self.unparsed.remove(&s.key) {
            let mut slot = format!("{}#unreadable", s.key);
            let mut n = 2;
            while self.unparsed.contains_key(&slot) {
                slot = format!("{}#unreadable{n}", s.key);
                n += 1;
            }
            self.unparsed.insert(slot, raw);
        }
        match self.games.get_mut(&s.key) {
            Some(g) => {
                g.sessions = g.sessions.saturating_add(1);
                g.hist.merge(&s.hist);
                g.first_played = g.first_played.min(s.start);
                if s.end >= g.last_played {
                    g.name = s.name.clone();
                    g.exe_path = s.exe_path.clone();
                    g.last_played = s.end;
                    g.last_session = LastSession::of(s);
                }
            }
            None => {
                self.games.insert(
                    s.key.clone(),
                    GameRecord {
                        name: s.name.clone(),
                        exe_path: s.exe_path.clone(),
                        sessions: 1,
                        first_played: s.start,
                        last_played: s.end,
                        hist: s.hist.clone(),
                        last_session: LastSession::of(s),
                    },
                );
            }
        }
    }
}

// ---- pending.toml ----

/// Sparse buckets for TOML, whose integers are i64 (counts clamp, as in `games.toml`).
fn toml_buckets(hist: &FrameHistogram) -> Vec<(u32, i64)> {
    hist.to_sparse()
        .into_iter()
        .map(|(i, n)| (i, i64::try_from(n).unwrap_or(i64::MAX)))
        .collect()
}

#[derive(Serialize)]
struct PendingOut<'a> {
    /// Before `sessions`: TOML needs plain values ahead of the `[[sessions]]` tables.
    rows: &'a [String],
    sessions: Vec<PendingSessionOut<'a>>,
}

#[derive(Serialize)]
struct PendingSessionOut<'a> {
    start: LocalTime,
    end: LocalTime,
    key: &'a str,
    name: &'a str,
    exe_path: &'a str,
    total_secs: f64,
    buckets: Vec<(u32, i64)>,
}

#[derive(Deserialize)]
struct PendingIn {
    #[serde(default)]
    rows: Vec<String>,
    #[serde(default)]
    sessions: Vec<PendingSessionIn>,
}

#[derive(Deserialize)]
struct PendingSessionIn {
    start: LocalTime,
    end: LocalTime,
    key: String,
    name: String,
    exe_path: String,
    total_secs: f64,
    #[serde(default)]
    buckets: Vec<(u32, u64)>,
}

/// `pending.toml`: finished sessions not yet merged into `games.toml` and `sessions.csv` rows
/// not yet appended, saved at quit so they are retried after a restart.
pub fn pending_toml(sessions: &[SessionRecord], rows: &[String]) -> String {
    let out = PendingOut {
        rows,
        sessions: sessions
            .iter()
            .map(|s| PendingSessionOut {
                start: s.start,
                end: s.end,
                key: &s.key,
                name: &s.name,
                exe_path: &s.exe_path,
                total_secs: s.hist.total_secs(),
                buckets: toml_buckets(&s.hist),
            })
            .collect(),
    };
    toml::to_string(&out).expect("pending.toml serialization cannot fail")
}

/// Reads `pending.toml` back as `(sessions, rows)`. Any unreadable part fails the whole file.
pub fn parse_pending(text: &str) -> Result<(Vec<SessionRecord>, Vec<String>), String> {
    let pending: PendingIn = toml::from_str(text).map_err(|e| e.message().replace('\n', " "))?;
    let sessions = pending
        .sessions
        .into_iter()
        .map(|s| {
            if !s.total_secs.is_finite() || s.total_secs < 0.0 {
                return Err(format!(
                    "total_secs {} is not a valid duration",
                    s.total_secs
                ));
            }
            Ok(SessionRecord {
                start: s.start,
                end: s.end,
                key: s.key,
                name: s.name,
                exe_path: s.exe_path,
                hist: FrameHistogram::from_sparse(&s.buckets, s.total_secs),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((sessions, pending.rows))
}

// ---- CSV ----

/// Quotes a field only when it contains `,`, `"`, CR or LF.
fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\r', '\n']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

/// `H:MM:SS`, seconds truncated, hours unbounded.
pub(crate) fn play_time(secs: f64) -> String {
    // Negative and NaN become 0 in the cast; infinity would saturate, so it is zeroed too.
    let total = if secs.is_finite() { secs as u64 } else { 0 };
    format!(
        "{}:{:02}:{:02}",
        total / 3600,
        total % 3600 / 60,
        total % 60
    )
}

/// `avg_fps,low_1pct_fps,high_1pct_fps`, each with one decimal; empty without a summary.
fn fps_columns(hist: &FrameHistogram) -> String {
    let finite = |s: &StatsSummary| {
        s.avg_fps.is_finite() && s.low_1pct_fps.is_finite() && s.high_1pct_fps.is_finite()
    };
    match hist.summary().filter(finite) {
        Some(s) => format!(
            "{:.1},{:.1},{:.1}",
            s.avg_fps, s.low_1pct_fps, s.high_1pct_fps
        ),
        None => ",,".to_string(),
    }
}

/// One `sessions.csv` row, `\r\n`-terminated. `display` is the game name to show.
pub fn session_csv_row(s: &SessionRecord, display: &str) -> String {
    format!(
        "{},{},{},{},{},{},{}\r\n",
        s.start,
        s.end,
        csv_field(display),
        play_time(s.hist.total_secs()),
        s.hist.frames(),
        fps_columns(&s.hist),
        csv_field(&s.exe_path),
    )
}

impl GameStore {
    /// The store without the games marked "Not a game", for `games.csv`. The store itself
    /// keeps them, so "It's a game" brings their history back.
    pub fn shown(&self, not_games: &BTreeMap<String, String>) -> GameStore {
        GameStore {
            games: self
                .games
                .iter()
                .filter(|(key, _)| !not_games.contains_key(*key))
                .map(|(key, g)| (key.clone(), g.clone()))
                .collect(),
            unparsed: BTreeMap::new(),
        }
    }
}

/// Splits the rows of `sessions.csv` (`shown`) and of the file kept aside for not-games
/// (`aside`) again for `not_games`: rows of a not-game go aside, the others are shown. Each
/// side is a whole file (BOM, header, rows by end time, the order `sessions.csv` is appended
/// in); an aside side without rows is empty, meaning no file. Lines that can't be read as a
/// session row stay shown.
pub fn split_sessions_csv(
    shown: &str,
    aside: &str,
    store: &GameStore,
    overrides: &BTreeMap<String, String>,
    not_games: &BTreeMap<String, String>,
) -> (String, String) {
    let (mut visible, mut hidden) = (Vec::new(), Vec::new());
    for line in data_lines(shown).chain(data_lines(aside)) {
        let key = csv_fields(line)
            .filter(|f| f.len() == 9)
            .and_then(|f| row_key(&f[2], &f[8], store, overrides));
        if key.is_some_and(|k| not_games.contains_key(&k)) {
            hidden.push(line);
        } else {
            visible.push(line);
        }
    }
    let end_time = |line: &&str| line.get(20..39).unwrap_or("").to_string();
    visible.sort_by_key(end_time);
    hidden.sort_by_key(end_time);
    let file = |rows: &[&str]| {
        let mut out = format!("{CSV_BOM}{SESSIONS_CSV_HEADER}\r\n");
        for row in rows {
            out.push_str(row);
            out.push_str("\r\n");
        }
        out
    };
    let aside = if hidden.is_empty() {
        String::new()
    } else {
        file(&hidden)
    };
    (file(&visible), aside)
}

/// The non-empty lines of a sessions CSV file after its BOM and header.
fn data_lines(text: &str) -> impl Iterator<Item = &str> {
    text.strip_prefix(CSV_BOM)
        .unwrap_or(text)
        .lines()
        .filter(|l| !l.trim().is_empty() && *l != SESSIONS_CSV_HEADER)
}

/// The fields of one CSV line, with `"…"` quoting and `""` for a quote; `None` when a quote
/// is left open.
fn csv_fields(line: &str) -> Option<Vec<String>> {
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut at_start = true;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c != '"' {
                field.push(c);
            } else if chars.peek() == Some(&'"') {
                chars.next();
                field.push('"');
            } else {
                quoted = false;
            }
        } else if c == ',' {
            fields.push(std::mem::take(&mut field));
            at_start = true;
            continue;
        } else if c == '"' && at_start {
            quoted = true;
        } else {
            field.push(c);
        }
        at_start = false;
    }
    (!quoted).then(|| {
        fields.push(field);
        fields
    })
}

/// The game key of a session row: from the exe path, or for a generic host (Java), the
/// stored game with that exe and the row's shown name.
fn row_key(
    game: &str,
    exe_path: &str,
    store: &GameStore,
    overrides: &BTreeMap<String, String>,
) -> Option<String> {
    if !is_generic_host(exe_path) {
        return Some(game_key(exe_path, None));
    }
    store
        .games
        .iter()
        .find(|(_, g)| {
            g.exe_path.eq_ignore_ascii_case(exe_path)
                && display_name(&g.name, &g.exe_path, overrides) == game
        })
        .map(|(key, _)| key.clone())
}

/// The whole `games.csv`: BOM, header, then games by `last_played` descending.
pub fn games_csv(store: &GameStore, overrides: &BTreeMap<String, String>) -> String {
    let mut games: Vec<&GameRecord> = store.games.values().collect();
    games.sort_by_key(|g| std::cmp::Reverse(g.last_played));
    let mut out = format!("{CSV_BOM}{GAMES_CSV_HEADER}\r\n");
    for g in games {
        let name = display_name(&g.name, &g.exe_path, overrides);
        out.push_str(&format!(
            "{},{},{},{},{},{},{}\r\n",
            csv_field(&name),
            g.sessions,
            play_time(g.hist.total_secs()),
            g.first_played,
            g.last_played,
            fps_columns(&g.hist),
            csv_field(&g.exe_path),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(day: u8, hour: u8, minute: u8) -> LocalTime {
        LocalTime {
            year: 2026,
            month: 10,
            day,
            hour,
            minute,
            second: 7,
        }
    }

    /// `n` frames of `secs` each. Dyadic frame times keep f64 sums exact.
    fn hist(n: usize, secs: f64) -> FrameHistogram {
        let mut h = FrameHistogram::new();
        for _ in 0..n {
            h.add(secs);
        }
        h
    }

    fn session(
        key: &str,
        name: &str,
        start: LocalTime,
        end: LocalTime,
        h: FrameHistogram,
    ) -> SessionRecord {
        SessionRecord {
            start,
            end,
            key: key.into(),
            name: name.into(),
            exe_path: format!(r"C:\Games\{name}.exe"),
            hist: h,
        }
    }

    fn game(name: &str, last: LocalTime, h: FrameHistogram) -> GameRecord {
        GameRecord {
            name: name.into(),
            exe_path: format!(r"C:\Games\{name}.exe"),
            sessions: 1,
            first_played: t(1, 0, 0),
            last_played: last,
            hist: h,
            last_session: None,
        }
    }

    // ---- LocalTime ----

    #[test]
    fn local_time_displays_zero_padded() {
        let time = LocalTime {
            year: 2026,
            month: 3,
            day: 4,
            hour: 5,
            minute: 6,
            second: 7,
        };
        assert_eq!(time.to_string(), "2026-03-04 05:06:07");
    }

    #[test]
    fn local_time_parse_roundtrips_and_rejects_garbage() {
        let time = t(26, 21, 5);
        assert_eq!(LocalTime::parse(&time.to_string()), Some(time));
        for bad in [
            "",
            "yesterday",
            "2026-10-26",
            "2026-13-01 00:00:00",
            "2026-00-01 00:00:00",
            "2026-10-32 00:00:00",
            "2026-10-26 24:00:00",
            "2026-10-26 00:60:00",
            "2026-10-26 00:00:60",
            "2026-10-26T00:00:00",
        ] {
            assert_eq!(LocalTime::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn local_time_orders_chronologically() {
        assert!(t(2, 0, 0) > t(1, 23, 59));
        assert!(t(1, 10, 30) > t(1, 10, 29));
    }

    // ---- is_kept ----

    #[test]
    fn short_session_is_not_kept() {
        let short = session("k", "G", t(1, 0, 0), t(1, 1, 0), hist(472, 0.125)); // 59.0 s
        assert_eq!(short.hist.total_secs(), 59.0);
        assert!(!short.is_kept());
    }

    #[test]
    fn minute_long_session_with_stats_is_kept() {
        let ok = session("k", "G", t(1, 0, 0), t(1, 1, 0), hist(480, 0.125)); // 60.0 s
        assert!(ok.is_kept());
    }

    #[test]
    fn long_session_without_enough_frames_is_not_kept() {
        let few = session("k", "G", t(1, 0, 0), t(1, 1, 0), hist(30, 2.0)); // 60 s, 30 frames
        assert!(!few.is_kept());
    }

    // ---- store ----

    #[test]
    fn add_session_creates_a_game() {
        let mut store = GameStore::default();
        let s = session("k", "Alpha", t(1, 9, 0), t(1, 10, 0), hist(480, 0.125));
        store.add_session(&s);
        let g = &store.games["k"];
        assert_eq!(g.sessions, 1);
        assert_eq!(g.name, "Alpha");
        assert_eq!(g.exe_path, r"C:\Games\Alpha.exe");
        assert_eq!((g.first_played, g.last_played), (t(1, 9, 0), t(1, 10, 0)));
        assert_eq!(g.hist, s.hist);
    }

    #[test]
    fn add_session_merges_two_sessions() {
        let mut store = GameStore::default();
        store.add_session(&session(
            "k",
            "Old Name",
            t(1, 9, 0),
            t(1, 10, 0),
            hist(480, 0.125),
        ));
        store.add_session(&session(
            "k",
            "New Name",
            t(2, 9, 0),
            t(2, 11, 0),
            hist(240, 0.25),
        ));
        assert_eq!(store.games.len(), 1);
        let g = &store.games["k"];
        assert_eq!(g.sessions, 2);
        assert_eq!(g.hist.frames(), 720);
        assert_eq!(g.hist.total_secs(), 120.0);
        assert_eq!(g.name, "New Name");
        assert_eq!(g.exe_path, r"C:\Games\New Name.exe");
        assert_eq!((g.first_played, g.last_played), (t(1, 9, 0), t(2, 11, 0)));
    }

    #[test]
    fn add_session_older_than_stored_keeps_newest_name_and_widens_range() {
        let mut store = GameStore::default();
        store.add_session(&session(
            "k",
            "Newer",
            t(5, 9, 0),
            t(5, 10, 0),
            hist(480, 0.125),
        ));
        store.add_session(&session(
            "k",
            "Older",
            t(2, 9, 0),
            t(2, 10, 0),
            hist(480, 0.125),
        ));
        let g = &store.games["k"];
        assert_eq!(g.name, "Newer");
        assert_eq!((g.first_played, g.last_played), (t(2, 9, 0), t(5, 10, 0)));
        assert_eq!(g.sessions, 2);
    }

    fn sample_store() -> GameStore {
        let mut store = GameStore::default();
        store.add_session(&session(
            r"c:\games\a.exe",
            "Alpha",
            t(1, 9, 0),
            t(1, 10, 0),
            hist(480, 0.125),
        ));
        store.add_session(&session(
            r"c:\program files\launcher\host.exe|my \o/ game, \x22hi\x22",
            "Tom \"Clancy\", Inc",
            t(2, 9, 0),
            t(2, 10, 0),
            hist(500, 1.0 / 60.0),
        ));
        store
    }

    #[test]
    fn store_roundtrips_through_toml() {
        let store = sample_store();
        let text = store.to_toml();
        let (back, warnings) = GameStore::parse(&text).unwrap_or_else(|e| panic!("{e}\n{text}"));
        assert_eq!(back, store, "{text}");
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn toml_has_games_tables_with_sparse_buckets() {
        let text = sample_store().to_toml();
        assert!(text.contains("buckets = [["), "{text}");
        assert!(text.contains("total_secs"), "{text}");
        assert!(
            text.contains("first_played = \"2026-10-01 09:00:07\""),
            "{text}"
        );
    }

    #[test]
    fn parse_empty_text_is_empty_store() {
        assert_eq!(
            GameStore::parse("").unwrap(),
            (GameStore::default(), Vec::new())
        );
    }

    #[test]
    fn parse_invalid_toml_is_err() {
        assert!(GameStore::parse("not toml").is_err());
    }

    #[test]
    fn parse_wrong_top_level_shape_is_err() {
        assert!(GameStore::parse("games = 5").is_err());
    }

    #[test]
    fn parse_skips_a_malformed_game_and_keeps_the_rest() {
        let good = sample_store().to_toml();
        let text = format!(
            "{good}\n[games.broken]\nname = \"Broken\"\nsessions = \"many\"\n\n[games.badtime]\nname = \"B\"\nexe_path = \"b.exe\"\nsessions = 1\nfirst_played = \"yesterday\"\nlast_played = \"2026-10-01 00:00:00\"\ntotal_secs = 1.0\n"
        );
        let (store, _) = GameStore::parse(&text).unwrap();
        assert_eq!(store.games, sample_store().games);
    }

    const BAD_SESSIONS: &str =
        "[games.broken]\nname = \"Broken\"\nsessions = \"many\"\nfuture = [1, 2]\n";
    const BAD_TIME: &str = "[games.badtime]\nname = \"B\"\nexe_path = \"b.exe\"\nsessions = 1\nfirst_played = \"yesterday\"\nlast_played = \"2026-10-01 00:00:00\"\ntotal_secs = 1.0\n";

    #[test]
    fn parse_reports_each_skipped_entry_with_key_and_reason() {
        let text = format!("{BAD_SESSIONS}\n{BAD_TIME}");
        let (store, warnings) = GameStore::parse(&text).unwrap();
        assert!(store.games.is_empty());
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings[0].contains("badtime"), "{warnings:?}");
        assert!(warnings[0].contains("yesterday"), "{warnings:?}");
        assert!(warnings[1].contains("broken"), "{warnings:?}");
        assert_eq!(
            store
                .unparsed
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["badtime", "broken"]
        );
    }

    #[test]
    fn skipped_entries_survive_to_toml_and_parse_unchanged() {
        let text = format!("{}\n{BAD_SESSIONS}\n{BAD_TIME}", sample_store().to_toml());
        let (store, _) = GameStore::parse(&text).unwrap();
        assert_eq!(store.unparsed.len(), 2);
        let (again, warnings) = GameStore::parse(&store.to_toml()).unwrap();
        assert_eq!(again, store);
        assert_eq!(warnings.len(), 2);
        assert_eq!(
            again.unparsed["broken"],
            text.parse::<toml::Table>().unwrap()["games"]["broken"]
        );
    }

    #[test]
    fn non_table_game_entry_is_kept_as_unparsed() {
        let (store, warnings) = GameStore::parse("[games]\nweird = 5\n").unwrap();
        assert_eq!(warnings.len(), 1);
        assert_eq!(store.unparsed["weird"], toml::Value::Integer(5));
        assert!(store.to_toml().contains("weird = 5"));
    }

    fn game_toml(total_secs: &str) -> String {
        format!("[games.k]\nname = \"G\"\nexe_path = \"g.exe\"\nsessions = 1\nfirst_played = \"2026-10-01 00:00:00\"\nlast_played = \"2026-10-02 00:00:00\"\ntotal_secs = {total_secs}\nbuckets = [[5, 200]]\n")
    }

    #[test]
    fn non_finite_or_negative_total_secs_makes_the_entry_unreadable_but_preserved() {
        for bad in ["nan", "inf", "-inf", "-1.0"] {
            let (store, warnings) = GameStore::parse(&game_toml(bad)).unwrap();
            assert!(store.games.is_empty(), "{bad}");
            assert!(store.unparsed.contains_key("k"), "{bad}");
            assert_eq!(warnings.len(), 1, "{bad}: {warnings:?}");
            assert!(
                warnings[0].contains('k') && warnings[0].contains("total_secs"),
                "{warnings:?}"
            );
            // Saved and reloaded it is still there with the same raw text.
            let saved = store.to_toml();
            assert!(
                saved.contains(&format!("total_secs = {bad}")),
                "{bad}: {saved}"
            );
            let (again, _) = GameStore::parse(&saved).unwrap();
            assert!(again.unparsed.contains_key("k"), "{bad}");
            assert_eq!(again.to_toml(), saved, "{bad}");
        }
    }

    #[test]
    fn add_session_on_an_unparsed_key_keeps_the_raw_entry_as_unreadable() {
        let (mut store, _) = GameStore::parse(BAD_SESSIONS).unwrap();
        let raw = store.unparsed["broken"].clone();
        store.add_session(&session(
            "broken",
            "Fresh",
            t(3, 9, 0),
            t(3, 10, 0),
            hist(480, 0.125),
        ));
        assert_eq!(store.games["broken"].name, "Fresh");
        assert_eq!(store.games["broken"].sessions, 1);
        assert!(!store.unparsed.contains_key("broken"));
        assert_eq!(store.unparsed["broken#unreadable"], raw);
        // A second unreadable entry for the same key must not overwrite the first.
        store
            .unparsed
            .insert("broken".into(), toml::Value::Integer(7));
        store.games.remove("broken");
        store.add_session(&session(
            "broken",
            "Fresh2",
            t(4, 9, 0),
            t(4, 10, 0),
            hist(480, 0.125),
        ));
        assert_eq!(store.unparsed["broken#unreadable"], raw);
        assert_eq!(store.unparsed.len(), 2);
        assert!(store
            .unparsed
            .values()
            .any(|v| *v == toml::Value::Integer(7)));
    }

    #[test]
    fn parse_ignores_unknown_fields() {
        let text = "[games.k]\nname = \"G\"\nexe_path = \"g.exe\"\nsessions = 3\nfirst_played = \"2026-10-01 00:00:00\"\nlast_played = \"2026-10-02 00:00:00\"\ntotal_secs = 90\nbuckets = [[5, 10]]\nfuture_field = true\n";
        let (store, warnings) = GameStore::parse(text).unwrap();
        assert!(warnings.is_empty());
        let g = &store.games["k"];
        assert_eq!(g.sessions, 3);
        assert_eq!(g.hist.total_secs(), 90.0);
        assert_eq!(g.hist.to_sparse(), vec![(5, 10)]);
    }

    #[test]
    fn counts_beyond_toml_integer_range_do_not_break_saving() {
        let mut store = GameStore::default();
        let mut g = game(
            "G",
            t(1, 0, 0),
            FrameHistogram::from_sparse(&[(5, u64::MAX)], 1.0),
        );
        g.sessions = u32::MAX;
        store.games.insert("k".into(), g);
        let (back, _) = GameStore::parse(&store.to_toml()).unwrap();
        assert_eq!(back.games["k"].hist.to_sparse(), vec![(5, i64::MAX as u64)]);
    }

    // ---- CSV ----

    #[test]
    fn sessions_header_has_no_bom() {
        assert!(!SESSIONS_CSV_HEADER.starts_with(CSV_BOM));
        assert_eq!(CSV_BOM, "\u{feff}");
    }

    #[test]
    fn session_row_formats_all_columns() {
        // 6000 frames of 1/64 s: 93.75 s of play, exactly 64.0 fps on average.
        let s = session(
            "k",
            "Alpha",
            t(1, 9, 0),
            t(1, 10, 0),
            hist(6000, 1.0 / 64.0),
        );
        let sum = s.hist.summary().unwrap();
        assert_eq!(
            session_csv_row(&s, "Alpha"),
            format!(
                "2026-10-01 09:00:07,2026-10-01 10:00:07,Alpha,0:01:33,6000,64.0,{:.1},{:.1},C:\\Games\\Alpha.exe\r\n",
                sum.low_1pct_fps, sum.high_1pct_fps
            )
        );
    }

    #[test]
    fn session_row_escapes_the_game_name() {
        let s = session("k", "x", t(1, 9, 0), t(1, 10, 0), hist(480, 0.125));
        let row = session_csv_row(&s, "Tom \"Clancy\", Inc");
        assert!(row.contains(",\"Tom \"\"Clancy\"\", Inc\","), "{row}");
        assert!(row.ends_with("\r\n"));
    }

    #[test]
    fn session_row_without_stats_leaves_fps_empty() {
        let s = session("k", "x", t(1, 9, 0), t(1, 10, 0), hist(30, 2.0));
        let row = session_csv_row(&s, "x");
        assert!(row.contains(",0:01:00,30,,,,"), "{row}");
    }

    #[test]
    fn play_time_truncates_to_h_mm_ss() {
        let mut store = GameStore::default();
        // 3725.4 s of play: 3725 frames of 1 s plus one 0.4 s frame.
        let mut h = hist(3725, 1.0);
        h.add(0.4);
        store.games.insert("k".into(), game("G", t(2, 0, 0), h));
        let csv = games_csv(&store, &BTreeMap::new());
        assert!(csv.contains(",1:02:05,"), "{csv}");
    }

    #[test]
    fn play_time_hours_are_unbounded() {
        let mut store = GameStore::default();
        store
            .games
            .insert("k".into(), game("G", t(2, 0, 0), hist(1, 400_000.0)));
        let csv = games_csv(&store, &BTreeMap::new());
        assert!(csv.contains(",111:06:40,"), "{csv}");
    }

    #[test]
    fn games_csv_starts_with_bom_and_header_and_uses_crlf() {
        let csv = games_csv(&sample_store(), &BTreeMap::new());
        assert!(csv.starts_with(&format!(
            "{CSV_BOM}game,sessions,play_time,first_played,last_played,avg_fps,low_1pct_fps,high_1pct_fps,exe_path\r\n"
        )));
        assert_eq!(csv.matches('\n').count(), csv.matches("\r\n").count());
        assert_eq!(csv.matches('\u{feff}').count(), 1);
    }

    #[test]
    fn games_csv_rows_sorted_by_last_played_descending() {
        let mut store = GameStore::default();
        store
            .games
            .insert("a".into(), game("Old", t(1, 0, 0), hist(480, 0.125)));
        store
            .games
            .insert("b".into(), game("Newest", t(9, 0, 0), hist(480, 0.125)));
        store
            .games
            .insert("c".into(), game("Middle", t(5, 0, 0), hist(480, 0.125)));
        let csv = games_csv(&store, &BTreeMap::new());
        let names: Vec<&str> = csv
            .lines()
            .skip(1)
            .map(|l| l.split(',').next().unwrap())
            .collect();
        assert_eq!(names, ["Newest", "Middle", "Old"]);
    }

    #[test]
    fn games_csv_row_has_fps_columns_and_escapes_fields() {
        let mut store = GameStore::default();
        store.games.insert(
            "k".into(),
            game("Tom \"Clancy\", Inc", t(2, 0, 0), hist(480, 0.125)),
        );
        let csv = games_csv(&store, &BTreeMap::new());
        assert_eq!(
            csv.lines().nth(1).unwrap(),
            "\"Tom \"\"Clancy\"\", Inc\",1,0:01:00,2026-10-01 00:00:07,2026-10-02 00:00:07,8.0,8.0,8.0,\"C:\\Games\\Tom \"\"Clancy\"\", Inc.exe\""
        );
    }

    #[test]
    fn csv_never_contains_non_finite_numbers() {
        let mut store = GameStore::default();
        // 100 frames in a vanishingly small total: avg fps would overflow to infinity.
        let tiny = FrameHistogram::from_sparse(&[(5, 100)], 1e-320);
        let nan = FrameHistogram::from_sparse(&[(5, 100)], f64::NAN);
        let inf = FrameHistogram::from_sparse(&[(5, 100)], f64::INFINITY);
        store
            .games
            .insert("a".into(), game("Tiny", t(1, 0, 0), tiny.clone()));
        store
            .games
            .insert("b".into(), game("NotNum", t(2, 0, 0), nan.clone()));
        store
            .games
            .insert("c".into(), game("Endless", t(3, 0, 0), inf.clone()));
        let mut all = games_csv(&store, &BTreeMap::new());
        for h in [tiny, nan, inf] {
            all.push_str(&session_csv_row(
                &session("k", "x", t(1, 0, 0), t(1, 1, 0), h),
                "x",
            ));
        }
        assert!(all.contains("\r\nEndless,1,0:00:00,"), "{all}");
        let lower = all.to_lowercase();
        assert!(!lower.contains("nan") && !lower.contains("inf"), "{all}");
    }

    #[test]
    fn games_csv_applies_name_overrides() {
        let mut store = GameStore::default();
        store
            .games
            .insert("k".into(), game("Raw Name", t(2, 0, 0), hist(480, 0.125)));
        let mut overrides = BTreeMap::new();
        overrides.insert("raw name.exe".to_string(), "Pretty Name".to_string());
        let csv = games_csv(&store, &overrides);
        assert!(csv.contains("\r\nPretty Name,1,"), "{csv}");
        assert!(!csv.contains("Raw Name,"), "{csv}");
    }

    // ---- pending.toml ----

    #[test]
    fn pending_roundtrips_sessions_and_rows() {
        let mut h = hist(500, 1.0 / 60.0);
        for _ in 0..7 {
            h.add(1.0 / 23.0);
        }
        let mut odd = session(
            "c:\\games\\a.exe",
            "A, \"the\" game",
            t(1, 0, 0),
            t(1, 1, 0),
            h,
        );
        odd.exe_path = r"C:\Games\A\a.exe".into();
        let sessions = vec![
            odd,
            session("b", "B", t(2, 0, 0), t(2, 3, 0), hist(480, 0.125)),
        ];
        let rows = vec![
            session_csv_row(&sessions[0], "A Shown"),
            session_csv_row(&sessions[1], "B"),
        ];
        let text = pending_toml(&sessions, &rows);
        assert_eq!(parse_pending(&text), Ok((sessions, rows)));
    }

    #[test]
    fn pending_with_only_rows_or_nothing_parses() {
        let rows = vec!["2026-10-01 00:00:07,x\r\n".to_string()];
        assert_eq!(
            parse_pending(&pending_toml(&[], &rows)),
            Ok((Vec::new(), rows))
        );
        assert_eq!(parse_pending(""), Ok((Vec::new(), Vec::new())));
    }

    #[test]
    fn unreadable_pending_is_an_error() {
        for bad in [
            "rows = [ not toml",
            "rows = 5",
            "[[sessions]]\nstart = \"yesterday\"\nend = \"2026-10-01 00:00:07\"\nkey = \"k\"\nname = \"n\"\nexe_path = \"e\"\ntotal_secs = 1.0\n",
            "[[sessions]]\nstart = \"2026-10-01 00:00:07\"\nend = \"2026-10-01 00:00:07\"\nkey = \"k\"\nname = \"n\"\nexe_path = \"e\"\ntotal_secs = nan\n",
        ] {
            assert!(parse_pending(bad).is_err(), "{bad}");
        }
    }

    // ---- Not games ----

    /// A session of `exe` named `name` (the display name in the row), at hour `h`.
    fn row_of(exe: &str, name: &str, h: u8) -> String {
        let s = SessionRecord {
            start: t(2, h, 0),
            end: t(2, h, 30),
            key: String::new(),
            name: name.into(),
            exe_path: exe.into(),
            hist: hist(4000, 1.0 / 64.0),
        };
        session_csv_row(&s, name)
    }

    fn csv_of(rows: &[String]) -> String {
        format!("{CSV_BOM}{SESSIONS_CSV_HEADER}\r\n{}", rows.concat())
    }

    fn not_games(keys: &[&str]) -> BTreeMap<String, String> {
        keys.iter()
            .map(|k| (k.to_string(), "x".to_string()))
            .collect()
    }

    const ZOOM: &str = r"C:\Apps\Zoom.exe";
    const DOOM: &str = r"C:\Games\Doom.exe";

    #[test]
    fn shown_store_leaves_out_not_games() {
        let mut store = GameStore::default();
        store.games.insert(
            r"c:\games\a.exe".into(),
            game("A", t(2, 0, 0), hist(10, 0.5)),
        );
        store.games.insert(
            r"c:\games\b.exe".into(),
            game("B", t(3, 0, 0), hist(10, 0.5)),
        );
        let shown = store.shown(&not_games(&[r"c:\games\b.exe"]));
        assert_eq!(
            shown.games.keys().collect::<Vec<_>>(),
            vec![r"c:\games\a.exe"]
        );
        assert_eq!(store.games.len(), 2, "the store itself keeps every game");
    }

    #[test]
    fn rows_of_not_games_move_aside_and_come_back_in_order() {
        let (z1, d2, z3) = (
            row_of(ZOOM, "Zoom", 1),
            row_of(DOOM, "Doom", 2),
            row_of(ZOOM, "Zoom", 3),
        );
        let all = csv_of(&[z1.clone(), d2.clone(), z3.clone()]);
        let store = GameStore::default();
        let none = BTreeMap::new();
        let hidden = not_games(&[r"c:\apps\zoom.exe"]);

        let (shown, aside) = split_sessions_csv(&all, "", &store, &none, &hidden);
        assert_eq!(shown, csv_of(std::slice::from_ref(&d2)));
        assert_eq!(aside, csv_of(&[z1.clone(), z3.clone()]));

        // "It's a game": the rows go back between the others, by start time.
        let (shown, aside) = split_sessions_csv(&shown, &aside, &store, &none, &BTreeMap::new());
        assert_eq!(shown, all);
        assert_eq!(aside, "", "nothing left aside: no file");
    }

    #[test]
    fn java_games_are_told_apart_by_their_shown_name() {
        let javaw = r"C:\Java\bin\javaw.exe";
        let mut store = GameStore::default();
        let mut mc = game("Minecraft", t(2, 0, 0), hist(10, 0.5));
        mc.exe_path = javaw.into();
        let mut other = game("Other", t(2, 0, 0), hist(10, 0.5));
        other.exe_path = javaw.into();
        store
            .games
            .insert(r"c:\java\bin\javaw.exe|minecraft".into(), mc);
        store
            .games
            .insert(r"c:\java\bin\javaw.exe|other".into(), other);
        let (m, o) = (row_of(javaw, "Minecraft", 1), row_of(javaw, "Other", 2));
        let all = csv_of(&[m.clone(), o.clone()]);
        let hidden = not_games(&[r"c:\java\bin\javaw.exe|other"]);
        let (shown, aside) = split_sessions_csv(&all, "", &store, &BTreeMap::new(), &hidden);
        assert_eq!(shown, csv_of(&[m]));
        assert_eq!(aside, csv_of(&[o]));
    }

    #[test]
    fn quoted_names_and_a_missing_last_newline_are_read() {
        let odd = row_of(ZOOM, "Zoom, \"the\" app", 1);
        let all = csv_of(std::slice::from_ref(&odd));
        let cut = all.trim_end_matches("\r\n");
        let hidden = not_games(&[r"c:\apps\zoom.exe"]);
        let (shown, aside) =
            split_sessions_csv(cut, "", &GameStore::default(), &BTreeMap::new(), &hidden);
        assert_eq!(shown, csv_of(&[]));
        assert_eq!(aside, csv_of(&[odd]));
    }

    #[test]
    fn unreadable_lines_stay_shown() {
        let all = csv_of(&["garbage without commas\r\n".to_string()]);
        let hidden = not_games(&[r"c:\apps\zoom.exe"]);
        let (shown, aside) =
            split_sessions_csv(&all, "", &GameStore::default(), &BTreeMap::new(), &hidden);
        assert_eq!(shown, all);
        assert_eq!(aside, "");
    }

    // ---- Last session ----

    #[test]
    fn add_session_keeps_the_newest_session() {
        let mut store = GameStore::default();
        let newer = session("k", "A", t(2, 9, 30), t(2, 10, 0), hist(6000, 1.0 / 64.0));
        let older = session("k", "A", t(2, 8, 0), t(2, 9, 0), hist(6000, 1.0 / 32.0));
        store.add_session(&newer);
        store.add_session(&older);
        let want = newer.hist.summary().unwrap();
        let last = store.games["k"].last_session.expect("a last session");
        assert_eq!((last.start, last.end), (newer.start, newer.end));
        assert_eq!(last.play_secs, want.play_secs);
        assert_eq!(last.avg_fps, want.avg_fps);
        assert_eq!(last.low_1pct_fps, want.low_1pct_fps);
        assert_eq!(last.high_1pct_fps, want.high_1pct_fps);
    }

    #[test]
    fn last_session_round_trips_and_old_entries_have_none() {
        let mut store = GameStore::default();
        store.add_session(&session(
            "k",
            "A",
            t(2, 9, 30),
            t(2, 10, 0),
            hist(6000, 1.0 / 64.0),
        ));
        let (back, warnings) = GameStore::parse(&store.to_toml()).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(back.games["k"].last_session, store.games["k"].last_session);

        let old = "[games.k]\nname = \"A\"\nexe_path = 'C:\\A.exe'\nsessions = 1\n\
                   first_played = \"2026-10-02 09:30:07\"\nlast_played = \"2026-10-02 10:00:07\"\n\
                   total_secs = 60.0\nbuckets = []\n";
        let (old, warnings) = GameStore::parse(old).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(old.games["k"].last_session, None);
    }

    #[test]
    fn a_damaged_last_session_leaves_the_game_readable() {
        let text = "[games.k]\nname = \"A\"\nexe_path = 'C:\\A.exe'\nsessions = 3\n\
                    first_played = \"2026-10-02 09:30:07\"\nlast_played = \"2026-10-02 10:00:07\"\n\
                    total_secs = 60.0\nbuckets = []\nlast_session = \"garbage\"\n";
        let (store, warnings) = GameStore::parse(text).unwrap();
        let game = store.games.get("k").expect("the game is still read");
        assert_eq!(game.sessions, 3);
        assert_eq!(game.last_session, None);
        assert!(store.unparsed.is_empty());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("games.k") && warnings[0].contains("last_session"));
    }
}
