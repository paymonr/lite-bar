//! Per-process FPS from present timestamps. Pure logic, no Windows types.
//!
//! Two time bases are involved. `event_time` is the ETW event timestamp in seconds: accurate,
//! but events arrive in bursts because of ETW's buffer flushing. `received_at` / `now` is the
//! app's monotonic clock in seconds and is only used to detect processes that stopped presenting.

use std::collections::{HashMap, VecDeque};

use crate::frame_stats::FrameHistogram;

/// Length of the sliding window of event times kept per pid and origin.
pub const WINDOW_SECS: f64 = 1.0;
/// A pid with no events received for longer than this shows no FPS and gets pruned.
pub const STALE_SECS: f64 = 2.0;
/// A gap between two presents longer than this (sleep, a modal dialog, a game that stopped
/// presenting) is a break in play, not a frame time. Same as the live counter's stale limit.
pub const MAX_FRAME_GAP_SECS: f64 = STALE_SECS;
/// A present this much older than the previous one means the clock stepped backwards; smaller
/// out-of-order skips are ordinary ETW reordering.
const CLOCK_STEP_BACK_SECS: f64 = 1.0;

/// Where a present event came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresentOrigin {
    /// DXGI and D3D9 present calls.
    Api,
    /// DxgKrnl presents: used for Vulkan and OpenGL and as a fallback.
    Kernel,
}

/// Event times (seconds) inside the sliding window for one origin.
struct Series {
    times: VecDeque<f64>,
    /// Largest event time pushed. Events may arrive slightly out of order, so the back of the
    /// deque is not always the newest.
    newest: f64,
    /// Largest `received_at` seen for this origin. Negative infinity until the first event.
    last_received: f64,
}

impl Default for Series {
    fn default() -> Self {
        Self {
            times: VecDeque::new(),
            newest: 0.0,
            last_received: f64::NEG_INFINITY,
        }
    }
}

impl Series {
    fn push(&mut self, event_time: f64, received_at: f64) {
        self.last_received = self.last_received.max(received_at);
        if self.times.is_empty() || event_time > self.newest {
            self.newest = event_time;
        }
        self.times.push_back(event_time);
        // Drop only entries strictly older than the window; the boundary entry stays.
        let cutoff = self.newest - WINDOW_SECS;
        while self.times.front().is_some_and(|&t| t < cutoff) {
            self.times.pop_front();
        }
    }

    fn is_fresh(&self, now: f64) -> bool {
        now - self.last_received <= STALE_SECS
    }

    /// Frames per second over the window, or `None` when stale, with fewer than two entries, or
    /// with no time span.
    fn rate(&self, now: f64) -> Option<f64> {
        if !self.is_fresh(now) {
            return None;
        }
        let n = self.times.len();
        if n < 2 {
            return None;
        }
        let span = self.newest - *self.times.front()?;
        (span > 0.0).then(|| (n - 1) as f64 / span)
    }
}

/// Focus-gated frame-time accounting for one pid.
struct StatsState {
    /// Which origin feeds the histogram. Starts as `Kernel` and switches to `Api` for good on
    /// the first `Api` present, because a DXGI game produces both and would be counted twice.
    origin: PresentOrigin,
    /// Time of the previous counted present of `origin`.
    prev: Option<f64>,
    hist: Option<FrameHistogram>,
}

impl Default for StatsState {
    fn default() -> Self {
        Self {
            origin: PresentOrigin::Kernel,
            prev: None,
            hist: None,
        }
    }
}

impl StatsState {
    fn observe(&mut self, origin: PresentOrigin, event_time: f64, focused: bool) {
        if origin == PresentOrigin::Api && self.origin == PresentOrigin::Kernel {
            self.origin = PresentOrigin::Api;
            self.prev = None;
            self.hist = None;
        }
        if origin != self.origin || !focused {
            return;
        }
        match self.prev {
            // A long pause, or the clock stepped backwards: a break, so the chain restarts.
            Some(prev)
                if event_time - prev > MAX_FRAME_GAP_SECS
                    || event_time < prev - CLOCK_STEP_BACK_SECS =>
            {
                self.prev = Some(event_time);
            }
            Some(prev) if event_time > prev => {
                self.hist
                    .get_or_insert_with(FrameHistogram::new)
                    .add(event_time - prev);
                self.prev = Some(event_time);
            }
            // Slightly out of order: skipped, and the chain does not move.
            Some(_) => {}
            None => self.prev = Some(event_time),
        }
    }
}

#[derive(Default)]
struct PidState {
    api: Series,
    kernel: Series,
    stats: StatsState,
}

/// Tracks present timestamps per process and derives a recent FPS value.
#[derive(Default)]
pub struct FpsTracker {
    pids: HashMap<u32, PidState>,
    focus: Option<u32>,
}

impl FpsTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records one present. Allocation-free once the pid's deques have grown to window size.
    pub fn record_present(
        &mut self,
        pid: u32,
        origin: PresentOrigin,
        event_time: f64,
        received_at: f64,
    ) {
        let focused = self.focus == Some(pid);
        let state = self.pids.entry(pid).or_default();
        match origin {
            PresentOrigin::Api => state.api.push(event_time, received_at),
            PresentOrigin::Kernel => state.kernel.push(event_time, received_at),
        }
        state.stats.observe(origin, event_time, focused);
    }

    /// FPS for `pid` at app-clock time `now`. Prefers a fresh `Api` series with at least two
    /// entries and otherwise falls back to a fresh `Kernel` one. The two are never summed
    /// because a DXGI game produces both. Freshness is judged per series, so an `Api` series
    /// that went quiet cannot mask live `Kernel` data.
    pub fn fps(&self, pid: u32, now: f64) -> Option<f64> {
        let state = self.pids.get(&pid)?;
        state.api.rate(now).or_else(|| state.kernel.rate(now))
    }

    /// Sets the pid whose presents feed frame stats. A different value forgets every pid's
    /// previous-present timestamp so a focus gap or capture restart is never a frame time;
    /// histograms are kept. The same value changes nothing.
    pub fn set_focus(&mut self, pid: Option<u32>) {
        if self.focus == pid {
            return;
        }
        self.focus = pid;
        for state in self.pids.values_mut() {
            state.stats.prev = None;
        }
    }

    /// Frame-time histogram accumulated for `pid` while it was focused.
    pub fn stats(&self, pid: u32) -> Option<&FrameHistogram> {
        self.pids.get(&pid)?.stats.hist.as_ref()
    }

    /// Removes and returns the histogram for `pid`.
    pub fn take_stats(&mut self, pid: u32) -> Option<FrameHistogram> {
        self.pids.get_mut(&pid)?.stats.hist.take()
    }

    /// Forgets pids whose series have all gone stale (nothing received for more than
    /// `STALE_SECS`), unless they still hold stats.
    pub fn prune(&mut self, now: f64) {
        self.pids
            .retain(|_, s| s.api.is_fresh(now) || s.kernel.is_fresh(now) || s.stats.hist.is_some());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(t: &mut FpsTracker, pid: u32, o: PresentOrigin, fps: f64, secs: f64, recv: f64) {
        let n = (fps * secs) as usize;
        for i in 0..=n {
            t.record_present(pid, o, 100.0 + i as f64 / fps, recv);
        }
    }

    #[test]
    fn steady_144() {
        let mut t = FpsTracker::new();
        feed(&mut t, 42, PresentOrigin::Api, 144.0, 2.0, 10.0);
        assert!((t.fps(42, 10.5).unwrap() - 144.0).abs() < 0.5);
    }

    #[test]
    fn fps_is_none_for_other_pid() {
        let mut t = FpsTracker::new();
        feed(&mut t, 42, PresentOrigin::Api, 60.0, 1.0, 10.0);
        assert_eq!(t.fps(7, 10.5), None);
    }

    #[test]
    fn fps_is_none_when_stale() {
        let mut t = FpsTracker::new();
        feed(&mut t, 42, PresentOrigin::Api, 60.0, 1.0, 10.0);
        assert!(t.fps(42, 11.9).is_some());
        assert_eq!(t.fps(42, 12.1), None);
    }

    #[test]
    fn single_present_is_none() {
        let mut t = FpsTracker::new();
        t.record_present(1, PresentOrigin::Api, 5.0, 5.0);
        assert_eq!(t.fps(1, 5.0), None);
    }

    #[test]
    fn api_preferred_over_kernel_no_double_count() {
        let mut t = FpsTracker::new();
        feed(&mut t, 9, PresentOrigin::Api, 60.0, 1.0, 10.0);
        feed(&mut t, 9, PresentOrigin::Kernel, 60.0, 1.0, 10.0);
        assert!((t.fps(9, 10.0).unwrap() - 60.0).abs() < 0.5);
    }

    #[test]
    fn kernel_used_when_no_api_events() {
        let mut t = FpsTracker::new();
        feed(&mut t, 3, PresentOrigin::Kernel, 90.0, 1.0, 10.0);
        assert!((t.fps(3, 10.0).unwrap() - 90.0).abs() < 0.5);
    }

    #[test]
    fn window_tracks_recent_rate_after_drop() {
        let mut t = FpsTracker::new();
        feed(&mut t, 5, PresentOrigin::Api, 144.0, 2.0, 10.0); // event times 100..102
        for i in 1..=30 {
            t.record_present(5, PresentOrigin::Api, 102.0 + i as f64 / 30.0, 11.0);
        } // 30 fps for 1 s
        assert!((t.fps(5, 11.0).unwrap() - 30.0).abs() < 1.0);
    }

    #[test]
    fn prune_drops_exited_processes() {
        let mut t = FpsTracker::new();
        feed(&mut t, 5, PresentOrigin::Api, 60.0, 1.0, 10.0);
        t.prune(20.0);
        assert_eq!(t.fps(5, 10.0), None);
    }

    // Additional tests covering the controller rulings.

    #[test]
    fn prune_keeps_fresh_processes() {
        let mut t = FpsTracker::new();
        feed(&mut t, 5, PresentOrigin::Api, 60.0, 1.0, 10.0);
        t.prune(11.0);
        assert!(t.fps(5, 11.0).is_some());
    }

    #[test]
    fn entry_exactly_at_window_boundary_is_kept() {
        let mut t = FpsTracker::new();
        t.record_present(1, PresentOrigin::Api, 100.0, 10.0);
        t.record_present(1, PresentOrigin::Api, 100.5, 10.0);
        t.record_present(1, PresentOrigin::Api, 101.0, 10.0);
        // 100.0 == newest - WINDOW_SECS, so it stays: 2 intervals over 1.0 s.
        assert_eq!(t.fps(1, 10.0), Some(2.0));
        t.record_present(1, PresentOrigin::Api, 101.25, 10.0);
        // 100.0 is now strictly older than 100.25 and is dropped; 100.5 stays.
        assert_eq!(t.fps(1, 10.0), Some(2.0 / 0.75));
    }

    #[test]
    fn trimming_is_per_origin() {
        let mut t = FpsTracker::new();
        t.record_present(1, PresentOrigin::Kernel, 100.0, 10.0);
        t.record_present(1, PresentOrigin::Kernel, 100.5, 10.0);
        // Api events far in the future must not trim the Kernel deque.
        t.record_present(1, PresentOrigin::Api, 200.0, 10.0);
        // Only one Api entry, so Kernel is used and still holds both entries.
        assert_eq!(t.fps(1, 10.0), Some(1.0 / 0.5));
    }

    #[test]
    fn out_of_order_event_is_still_counted() {
        let mut t = FpsTracker::new();
        t.record_present(1, PresentOrigin::Api, 100.0, 10.0);
        t.record_present(1, PresentOrigin::Api, 100.5, 10.0);
        t.record_present(1, PresentOrigin::Api, 100.25, 10.0); // late, older than the back
        t.record_present(1, PresentOrigin::Api, 101.0, 10.0);
        // 4 entries over 100.0..101.0.
        assert_eq!(t.fps(1, 10.0), Some(3.0));
    }

    #[test]
    fn last_received_is_the_maximum_seen() {
        let mut t = FpsTracker::new();
        t.record_present(1, PresentOrigin::Api, 100.0, 10.0);
        t.record_present(1, PresentOrigin::Api, 100.5, 10.0);
        t.record_present(1, PresentOrigin::Api, 100.6, 5.0); // smaller received_at must not rewind
        assert!(t.fps(1, 11.9).is_some());
        assert_eq!(t.fps(1, 12.1), None);
    }

    #[test]
    fn identical_timestamps_do_not_divide_by_zero() {
        let mut t = FpsTracker::new();
        t.record_present(1, PresentOrigin::Api, 100.0, 10.0);
        t.record_present(1, PresentOrigin::Api, 100.0, 10.0);
        assert_eq!(t.fps(1, 10.0), None);
    }

    #[test]
    fn default_matches_new() {
        let mut t = FpsTracker::default();
        assert_eq!(t.fps(1, 0.0), None);
        t.record_present(1, PresentOrigin::Api, 1.0, 1.0);
        t.record_present(1, PresentOrigin::Api, 1.5, 1.0);
        assert_eq!(t.fps(1, 1.0), Some(2.0));
    }

    #[test]
    fn late_out_of_order_event_does_not_move_the_newest_endpoint() {
        let mut t = FpsTracker::new();
        t.record_present(1, PresentOrigin::Api, 100.0, 10.0);
        t.record_present(1, PresentOrigin::Api, 101.0, 10.0);
        t.record_present(1, PresentOrigin::Api, 100.9, 10.0); // late event arrives last
                                                              // The span is 100.0..101.0 (newest, not the back at 100.9): 2 intervals / 1.0 s.
        assert_eq!(t.fps(1, 10.0), Some(2.0));
    }

    #[test]
    fn stale_api_falls_back_to_live_kernel() {
        let mut t = FpsTracker::new();
        feed(&mut t, 8, PresentOrigin::Api, 60.0, 1.0, 10.0); // event times 100..101
        for i in 0..=90 {
            t.record_present(8, PresentOrigin::Kernel, 111.0 + i as f64 / 90.0, 12.5);
        }
        assert!((t.fps(8, 12.6).unwrap() - 90.0).abs() < 0.5);
    }

    #[test]
    fn prune_keeps_pid_with_only_one_fresh_series() {
        let mut t = FpsTracker::new();
        feed(&mut t, 8, PresentOrigin::Api, 60.0, 1.0, 10.0);
        feed(&mut t, 8, PresentOrigin::Kernel, 60.0, 1.0, 12.5);
        t.prune(12.6);
        assert!(t.fps(8, 12.6).is_some());
        t.prune(20.0);
        assert_eq!(t.fps(8, 12.6), None);
    }

    // Focus-gated per-process frame stats.

    fn present_run(
        t: &mut FpsTracker,
        pid: u32,
        o: PresentOrigin,
        start: f64,
        step: f64,
        n: usize,
    ) {
        for i in 0..n {
            t.record_present(pid, o, start + i as f64 * step, 10.0);
        }
    }

    #[test]
    fn focused_pid_accumulates_stats() {
        let mut t = FpsTracker::new();
        t.set_focus(Some(42));
        present_run(&mut t, 42, PresentOrigin::Kernel, 100.0, 1.0 / 60.0, 600);
        assert_eq!(t.stats(42).unwrap().frames(), 599);
    }

    #[test]
    fn unfocused_pid_has_no_stats() {
        let mut t = FpsTracker::new();
        t.set_focus(Some(1));
        present_run(&mut t, 42, PresentOrigin::Kernel, 100.0, 1.0 / 60.0, 600);
        assert!(t.stats(42).is_none());
        assert!(t.stats(1).is_none());
    }

    #[test]
    fn refocus_does_not_count_the_gap() {
        let mut t = FpsTracker::new();
        t.set_focus(Some(1));
        present_run(&mut t, 1, PresentOrigin::Kernel, 100.0, 0.01, 200);
        t.set_focus(Some(2));
        t.set_focus(Some(1));
        present_run(&mut t, 1, PresentOrigin::Kernel, 400.0, 0.01, 200);
        let h = t.stats(1).unwrap();
        assert_eq!(h.frames(), 398);
        assert!((h.total_secs() - 3.98).abs() < 0.01, "{}", h.total_secs());
    }

    #[test]
    fn api_presents_replace_kernel_for_stats() {
        let mut t = FpsTracker::new();
        t.set_focus(Some(9));
        present_run(&mut t, 9, PresentOrigin::Kernel, 100.0, 1.0 / 60.0, 30);
        assert_eq!(t.stats(9).unwrap().frames(), 29);
        // A DXGI game: every frame yields an Api present and a slightly later Kernel present.
        for i in 0..=120 {
            let at = 101.0 + i as f64 / 60.0;
            t.record_present(9, PresentOrigin::Api, at, 10.0);
            t.record_present(9, PresentOrigin::Kernel, at + 0.001, 10.0);
        }
        let h = t.stats(9).unwrap();
        // 121 Api presents; the first switches the origin and restarts the chain.
        assert_eq!(h.frames(), 120);
        assert!((h.frames() as f64 / h.total_secs() - 60.0).abs() < 0.5);
    }

    #[test]
    fn unfocused_api_present_still_switches_origin_for_good() {
        let mut t = FpsTracker::new();
        // The game presents through the API while unfocused, then gets focus.
        t.record_present(9, PresentOrigin::Api, 100.0, 10.0);
        t.set_focus(Some(9));
        // Kernel presents are not counted: the origin already switched to Api.
        t.record_present(9, PresentOrigin::Kernel, 100.2, 10.0);
        t.record_present(9, PresentOrigin::Kernel, 100.4, 10.0);
        assert!(t.stats(9).is_none());
        for i in 0..=60 {
            let at = 101.0 + i as f64 / 60.0;
            t.record_present(9, PresentOrigin::Api, at, 10.0);
            if i % 2 == 0 {
                // Kernel runs at half the Api rate here, so mixing origins would show.
                t.record_present(9, PresentOrigin::Kernel, at + 0.001, 10.0);
            }
        }
        assert_eq!(t.stats(9).unwrap().frames(), 60);
    }

    #[test]
    fn out_of_order_present_is_skipped() {
        let mut t = FpsTracker::new();
        t.set_focus(Some(1));
        for at in [1.0, 1.1, 1.05, 1.2] {
            t.record_present(1, PresentOrigin::Kernel, at, 10.0);
        }
        let h = t.stats(1).unwrap();
        // 1.05 is not newer than 1.1: skipped and does not move the chain, so 1.1 -> 1.2 counts.
        assert_eq!(h.frames(), 2);
        assert!((h.total_secs() - 0.2).abs() < 1e-9, "{}", h.total_secs());
    }

    #[test]
    fn long_pause_is_not_a_frame_time() {
        let mut t = FpsTracker::new();
        t.set_focus(Some(1));
        present_run(&mut t, 1, PresentOrigin::Kernel, 100.0, 1.0 / 60.0, 600);
        // Sleep, a modal dialog or a game that stops presenting: 8 h with no presents.
        present_run(
            &mut t,
            1,
            PresentOrigin::Kernel,
            100.0 + 8.0 * 3600.0,
            1.0 / 60.0,
            600,
        );
        let h = t.stats(1).unwrap();
        assert_eq!(h.frames(), 2 * 599);
        let runs = 2.0 * 599.0 / 60.0;
        assert!((h.total_secs() - runs).abs() < 0.01, "{}", h.total_secs());
    }

    #[test]
    fn clock_step_back_resets_the_chain() {
        let mut t = FpsTracker::new();
        t.set_focus(Some(1));
        present_run(&mut t, 1, PresentOrigin::Kernel, 100.0, 0.01, 11);
        // The system clock stepped back by 50 s: the chain restarts at the new time.
        present_run(&mut t, 1, PresentOrigin::Kernel, 50.0, 0.01, 11);
        let h = t.stats(1).unwrap();
        assert_eq!(h.frames(), 20);
        assert!((h.total_secs() - 0.2).abs() < 1e-6, "{}", h.total_secs());
    }

    #[test]
    fn gap_just_under_limit_is_counted() {
        let mut t = FpsTracker::new();
        t.set_focus(Some(1));
        t.record_present(1, PresentOrigin::Kernel, 100.0, 10.0);
        t.record_present(
            1,
            PresentOrigin::Kernel,
            100.0 + MAX_FRAME_GAP_SECS - 0.01,
            10.0,
        );
        let h = t.stats(1).unwrap();
        assert_eq!(h.frames(), 1);
        assert!((h.total_secs() - 1.99).abs() < 1e-9, "{}", h.total_secs());
    }

    #[test]
    fn same_focus_keeps_interval_chain() {
        let mut t = FpsTracker::new();
        t.set_focus(Some(1));
        t.record_present(1, PresentOrigin::Kernel, 1.0, 10.0);
        t.set_focus(Some(1));
        t.record_present(1, PresentOrigin::Kernel, 1.1, 10.0);
        assert_eq!(t.stats(1).unwrap().frames(), 1);
    }

    #[test]
    fn take_stats_removes_them() {
        let mut t = FpsTracker::new();
        t.set_focus(Some(1));
        present_run(&mut t, 1, PresentOrigin::Kernel, 100.0, 0.01, 50);
        let h = t.take_stats(1).unwrap();
        assert_eq!(h.frames(), 49);
        assert!(t.stats(1).is_none());
        assert!(t.take_stats(1).is_none());
    }

    #[test]
    fn prune_keeps_pids_with_stats() {
        let mut t = FpsTracker::new();
        t.set_focus(Some(1));
        present_run(&mut t, 1, PresentOrigin::Kernel, 100.0, 0.01, 50);
        present_run(&mut t, 2, PresentOrigin::Kernel, 100.0, 0.01, 50);
        t.prune(1000.0);
        assert_eq!(t.stats(1).unwrap().frames(), 49);
        // Pid 1's sliding window is stale but its stats survive; pid 2 has neither and is gone.
        assert_eq!(t.pids.len(), 1);
        assert!(t.pids.contains_key(&1));
    }
}
