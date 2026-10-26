//! Session lifecycle: which processes have an open play session, when each one ends, and the
//! record it becomes. Pure; the app supplies process liveness, the clock and the name lookup.

use crate::frame_stats::FrameHistogram;
use crate::game_name::{clean_window_title, game_key};
use crate::history::{play_time, LocalTime, SessionRecord};

/// One process's run, from its first counted frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenSession {
    pub pid: u32,
    /// Process creation time (FILETIME ticks); with `pid` it identifies the run.
    pub created: u64,
    pub exe_path: String,
    /// Foreground window title when the session started.
    pub window_title: Option<String>,
    pub start: LocalTime,
    /// Last time the process was seen running.
    pub last_alive: LocalTime,
}

impl OpenSession {
    /// The stored record for this session ending at `end`, or `None` when it is too short to
    /// keep. `resolve_name(exe_path, window_title)` is only called for kept sessions.
    pub fn finish(
        self,
        end: LocalTime,
        hist: FrameHistogram,
        resolve_name: impl FnOnce(&str, Option<&str>) -> String,
    ) -> Option<SessionRecord> {
        let record = SessionRecord {
            start: self.start,
            end,
            key: game_key(
                &self.exe_path,
                self.window_title
                    .as_deref()
                    .and_then(clean_window_title)
                    .as_deref(),
            ),
            name: String::new(),
            exe_path: self.exe_path,
            hist,
        };
        if !record.is_kept() {
            return None;
        }
        let name = resolve_name(&record.exe_path, self.window_title.as_deref());
        Some(SessionRecord { name, ..record })
    }
}

/// The open sessions, at most one per pid.
#[derive(Debug, Default)]
pub struct SessionBook {
    sessions: Vec<OpenSession>,
}

impl SessionBook {
    pub fn new() -> SessionBook {
        SessionBook::default()
    }

    /// Starts tracking `s`. No-op if its pid is tracked with the same creation time; a reused
    /// pid (new creation time) replaces the old entry.
    pub fn track(&mut self, s: OpenSession) {
        match self.sessions.iter_mut().find(|o| o.pid == s.pid) {
            Some(existing) if existing.created == s.created => {}
            Some(existing) => *existing = s,
            None => self.sessions.push(s),
        }
    }

    pub fn contains(&self, pid: u32) -> bool {
        self.sessions.iter().any(|s| s.pid == pid)
    }

    pub fn mark_alive(&mut self, pid: u32, now: LocalTime) {
        if let Some(s) = self.sessions.iter_mut().find(|s| s.pid == pid) {
            s.last_alive = now;
        }
    }

    pub fn remove(&mut self, pid: u32) -> Option<OpenSession> {
        let index = self.sessions.iter().position(|s| s.pid == pid)?;
        Some(self.sessions.swap_remove(index))
    }

    pub fn pids(&self) -> Vec<u32> {
        self.sessions.iter().map(|s| s.pid).collect()
    }

    /// The periodic liveness check: sessions whose process still runs are marked alive at
    /// `now`; the others are removed and returned with their end time (last seen alive).
    pub fn sweep(
        &mut self,
        now: LocalTime,
        mut is_running: impl FnMut(&OpenSession) -> bool,
    ) -> Vec<(OpenSession, LocalTime)> {
        let mut finished = Vec::new();
        for s in std::mem::take(&mut self.sessions) {
            if is_running(&s) {
                self.sessions.push(OpenSession {
                    last_alive: now,
                    ..s
                });
            } else {
                let end = end_time(&s, false, now);
                finished.push((s, end));
            }
        }
        finished
    }

    /// Quit or sign-out: removes every session and returns it with its end time (`now` for a
    /// process that still runs, otherwise last seen alive).
    pub fn drain(
        &mut self,
        now: LocalTime,
        mut is_running: impl FnMut(&OpenSession) -> bool,
    ) -> Vec<(OpenSession, LocalTime)> {
        std::mem::take(&mut self.sessions)
            .into_iter()
            .map(|s| {
                let end = end_time(&s, is_running(&s), now);
                (s, end)
            })
            .collect()
    }
}

/// When a session ends: `now` if its process still runs, else when it was last seen alive.
pub fn end_time(s: &OpenSession, still_running: bool, now: LocalTime) -> LocalTime {
    if still_running {
        now
    } else {
        s.last_alive
    }
}

/// One log line for a recorded session: name, play time and average FPS.
pub fn session_log_line(s: &SessionRecord, display_name: &str) -> String {
    let avg = s.hist.summary().map_or(0.0, |summary| summary.avg_fps);
    format!(
        "session recorded: {display_name}, play time {}, avg {avg:.1} FPS",
        play_time(s.hist.total_secs())
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(hour: u8, minute: u8) -> LocalTime {
        LocalTime {
            year: 2026,
            month: 10,
            day: 26,
            hour,
            minute,
            second: 0,
        }
    }

    fn open(pid: u32, created: u64) -> OpenSession {
        OpenSession {
            pid,
            created,
            exe_path: r"C:\Games\Alpha\alpha.exe".into(),
            window_title: Some("Alpha v1.2 - Main Menu".into()),
            start: t(10, 0),
            last_alive: t(10, 0),
        }
    }

    /// `n` frames of `secs` each (dyadic, so sums are exact).
    fn hist(n: usize, secs: f64) -> FrameHistogram {
        let mut h = FrameHistogram::new();
        for _ in 0..n {
            h.add(secs);
        }
        h
    }

    #[test]
    fn track_is_idempotent_for_same_process() {
        let mut book = SessionBook::new();
        book.track(open(7, 100));
        book.mark_alive(7, t(10, 5));
        book.track(open(7, 100));
        assert_eq!(book.pids(), vec![7]);
        let s = book.remove(7).unwrap();
        assert_eq!(s.last_alive, t(10, 5), "the existing session is kept");
        assert!(!book.contains(7));
    }

    #[test]
    fn reused_pid_with_new_creation_time_replaces() {
        let mut book = SessionBook::new();
        book.track(open(7, 100));
        book.track(open(7, 200));
        assert_eq!(book.pids(), vec![7]);
        assert_eq!(book.remove(7).unwrap().created, 200);
    }

    #[test]
    fn exited_while_hidden_ends_at_last_seen_alive() {
        let mut s = open(7, 100);
        s.last_alive = t(10, 30);
        assert_eq!(end_time(&s, false, t(12, 0)), s.last_alive);
    }

    #[test]
    fn running_session_ends_now() {
        let mut s = open(7, 100);
        s.last_alive = t(10, 30);
        assert_eq!(end_time(&s, true, t(12, 0)), t(12, 0));
    }

    #[test]
    fn contains_and_remove_unknown_pid() {
        let mut book = SessionBook::new();
        book.track(open(7, 100));
        assert!(book.contains(7));
        assert!(!book.contains(8));
        assert_eq!(book.remove(8), None);
        book.mark_alive(8, t(11, 0)); // unknown pid: no effect
        assert_eq!(book.pids(), vec![7]);
    }

    #[test]
    fn sweep_marks_running_alive_and_returns_exited_at_last_alive() {
        let mut book = SessionBook::new();
        book.track(open(1, 10));
        book.track(open(2, 20));
        book.mark_alive(2, t(10, 20));
        let finished = book.sweep(t(10, 40), |s| s.pid == 1);
        assert_eq!(finished.len(), 1);
        assert_eq!(finished[0].0.pid, 2);
        assert_eq!(finished[0].1, t(10, 20));
        assert_eq!(book.pids(), vec![1]);
        assert_eq!(book.remove(1).unwrap().last_alive, t(10, 40));
    }

    #[test]
    fn drain_ends_running_now_and_exited_at_last_alive() {
        let mut book = SessionBook::new();
        book.track(open(1, 10));
        book.track(open(2, 20));
        book.mark_alive(2, t(10, 20));
        let mut finished = book.drain(t(11, 0), |s| s.pid == 1);
        finished.sort_by_key(|(s, _)| s.pid);
        let ends: Vec<(u32, LocalTime)> = finished.iter().map(|(s, e)| (s.pid, *e)).collect();
        assert_eq!(ends, vec![(1, t(11, 0)), (2, t(10, 20))]);
        assert!(book.pids().is_empty());
    }

    #[test]
    fn finish_builds_the_record_with_resolved_name_and_path_key() {
        let s = open(7, 100);
        let record = s
            .finish(t(10, 10), hist(8192, 1.0 / 128.0), |exe, title| {
                assert_eq!(exe, r"C:\Games\Alpha\alpha.exe");
                assert_eq!(title, Some("Alpha v1.2 - Main Menu"));
                "Alpha".to_string()
            })
            .expect("64 s with stats is kept");
        assert_eq!(record.start, t(10, 0));
        assert_eq!(record.end, t(10, 10));
        assert_eq!(record.key, r"c:\games\alpha\alpha.exe");
        assert_eq!(record.name, "Alpha");
        assert_eq!(record.exe_path, r"C:\Games\Alpha\alpha.exe");
        assert_eq!(record.hist.frames(), 8192);
    }

    #[test]
    fn finish_keys_a_generic_host_by_its_cleaned_title() {
        let mut s = open(7, 100);
        s.exe_path = r"C:\Java\bin\javaw.exe".into();
        s.window_title = Some("Minecraft 1.21.4 - Singleplayer".into());
        let record = s
            .finish(t(10, 10), hist(8192, 1.0 / 128.0), |_, _| {
                "Minecraft".into()
            })
            .unwrap();
        assert_eq!(record.key, r"c:\java\bin\javaw.exe|minecraft");
    }

    #[test]
    fn finish_drops_a_short_session_without_resolving_its_name() {
        let s = open(7, 100);
        let record = s.finish(t(10, 1), hist(1024, 1.0 / 128.0), |_, _| {
            panic!("name lookup must not run for a dropped session")
        });
        assert_eq!(record, None);
    }

    #[test]
    fn log_line_has_name_play_time_and_avg() {
        let record = open(7, 100)
            .finish(t(10, 10), hist(8192, 1.0 / 128.0), |_, _| "Alpha".into())
            .unwrap();
        assert_eq!(
            session_log_line(&record, "Alpha Renamed"),
            "session recorded: Alpha Renamed, play time 0:01:04, avg 128.0 FPS"
        );
    }
}
