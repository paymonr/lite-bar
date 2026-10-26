//! Per-game play history: the `games.toml` store and the `games.csv` / `sessions.csv` formats.
//! Pure formatting and parsing; the file IO lives in the app crate.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::frame_stats::{FrameHistogram, StatsSummary};
use crate::game_name::display_name;

/// Sessions with less counted play time than this are discarded.
pub const MIN_SESSION_SECS: f64 = 60.0;

/// UTF-8 byte order mark. Written once at the start of a newly created CSV file so Excel
/// shows non-ASCII names correctly.
pub const CSV_BOM: &str = "\u{feff}";

pub const SESSIONS_CSV_HEADER: &str =
    "start,end,game,play_time,frames,avg_fps,low_1pct_fps,high_1pct_fps,exe_path";
const GAMES_CSV_HEADER: &str =
    "game,sessions,play_time,first_played,last_played,avg_fps,low_1pct_fps,high_1pct_fps,exe_path";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
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
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GameStore {
    pub games: BTreeMap<String, GameRecord>,
    /// Raw values of `games.toml` entries that could not be read, kept so saving never drops them.
    pub unparsed: BTreeMap<String, toml::Value>,
}

fn parse_game(value: &toml::Value) -> Result<GameRecord, String> {
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
    Ok(GameRecord {
        name: g.name,
        exe_path: g.exe_path,
        sessions: g.sessions,
        first_played: g.first_played,
        last_played: g.last_played,
        hist: FrameHistogram::from_sparse(&g.buckets, g.total_secs),
    })
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
                Ok(record) => {
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
                buckets: g
                    .hist
                    .to_sparse()
                    .into_iter()
                    .map(|(i, n)| (i, i64::try_from(n).unwrap_or(i64::MAX)))
                    .collect(),
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
                    },
                );
            }
        }
    }
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
}
