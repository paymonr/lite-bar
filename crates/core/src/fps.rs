//! Per-process FPS from present timestamps. Pure logic, no Windows types.
//!
//! Two time bases are involved. `event_time` is the ETW event timestamp in seconds: accurate,
//! but events arrive in bursts because of ETW's buffer flushing. `received_at` / `now` is the
//! app's monotonic clock in seconds and is only used to detect processes that stopped presenting.

use std::collections::{HashMap, VecDeque};

/// Length of the sliding window of event times kept per pid and origin.
pub const WINDOW_SECS: f64 = 1.0;
/// A pid with no events received for longer than this shows no FPS and gets pruned.
pub const STALE_SECS: f64 = 2.0;

/// Where a present event came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresentOrigin {
    /// DXGI and D3D9 present calls.
    Api,
    /// DxgKrnl presents: used for Vulkan and OpenGL and as a fallback.
    Kernel,
}

/// Event times (seconds) inside the sliding window for one origin.
#[derive(Default)]
struct Series {
    times: VecDeque<f64>,
    /// Largest event time pushed. Events may arrive slightly out of order, so the back of the
    /// deque is not always the newest.
    newest: f64,
}

impl Series {
    fn push(&mut self, event_time: f64) {
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

    /// Frames per second over the window, or `None` with fewer than two entries or no time span.
    fn rate(&self) -> Option<f64> {
        let n = self.times.len();
        if n < 2 {
            return None;
        }
        let span = self.newest - *self.times.front()?;
        (span > 0.0).then(|| (n - 1) as f64 / span)
    }
}

#[derive(Default)]
struct PidState {
    api: Series,
    kernel: Series,
    last_received: f64,
}

/// Tracks present timestamps per process and derives a recent FPS value.
#[derive(Default)]
pub struct FpsTracker {
    pids: HashMap<u32, PidState>,
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
        let state = self.pids.entry(pid).or_default();
        if received_at > state.last_received {
            state.last_received = received_at;
        }
        match origin {
            PresentOrigin::Api => state.api.push(event_time),
            PresentOrigin::Kernel => state.kernel.push(event_time),
        }
    }

    /// FPS for `pid` at app-clock time `now`. Prefers `Api` events and falls back to `Kernel`
    /// ones; the two are never summed because a DXGI game produces both.
    pub fn fps(&self, pid: u32, now: f64) -> Option<f64> {
        let state = self.pids.get(&pid)?;
        if now - state.last_received > STALE_SECS {
            return None;
        }
        if state.api.times.len() >= 2 {
            state.api.rate()
        } else {
            state.kernel.rate()
        }
    }

    /// Forgets pids that have not been heard from for more than `STALE_SECS`.
    pub fn prune(&mut self, now: f64) {
        self.pids.retain(|_, s| now - s.last_received <= STALE_SECS);
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
}
