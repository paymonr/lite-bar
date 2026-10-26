//! Log-bucketed frame-time histogram with avg FPS and 1% low/high.

pub const MIN_STATS_FRAMES: u64 = 100;

#[derive(Clone, Debug, PartialEq)]
pub struct FrameHistogram {
    buckets: Vec<u64>,
    frames: u64,
    total_secs: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StatsSummary {
    pub frames: u64,
    pub play_secs: f64,
    pub avg_fps: f64,
    pub low_1pct_fps: f64,
    pub high_1pct_fps: f64,
}

impl Default for FrameHistogram {
    fn default() -> Self {
        Self::new()
    }
}

/// Ratio between adjacent bucket edges.
const RATIO: f64 = 1.01;
/// Lower edge of bucket 0, in seconds.
const MIN_SECS: f64 = 1e-4;
/// Bucket 1157 starts at ~9.9997 s, so it holds everything from 10 s up.
const BUCKET_COUNT: usize = 1158;

fn bucket_index(frame_secs: f64) -> usize {
    let raw = ((frame_secs / MIN_SECS).ln() / RATIO.ln()).floor();
    if raw <= 0.0 {
        0
    } else {
        (raw as usize).min(BUCKET_COUNT - 1)
    }
}

/// Geometric midpoint of bucket `index`.
fn bucket_value(index: usize) -> f64 {
    MIN_SECS * RATIO.powf(index as f64 + 0.5)
}

impl FrameHistogram {
    pub fn new() -> Self {
        Self {
            buckets: vec![0; BUCKET_COUNT],
            frames: 0,
            total_secs: 0.0,
        }
    }

    /// Records one frame time. Allocation-free. Non-finite or non-positive values are ignored.
    pub fn add(&mut self, frame_secs: f64) {
        if !frame_secs.is_finite() || frame_secs <= 0.0 {
            return;
        }
        self.buckets[bucket_index(frame_secs)] += 1;
        self.frames += 1;
        self.total_secs += frame_secs;
    }

    pub fn merge(&mut self, other: &FrameHistogram) {
        for (mine, theirs) in self.buckets.iter_mut().zip(&other.buckets) {
            *mine += theirs;
        }
        self.frames += other.frames;
        self.total_secs += other.total_secs;
    }

    pub fn frames(&self) -> u64 {
        self.frames
    }

    pub fn total_secs(&self) -> f64 {
        self.total_secs
    }

    /// Nearest-rank percentile of frame time, `p` clamped to [0, 1]. `None` when empty.
    pub fn percentile_frame_secs(&self, p: f64) -> Option<f64> {
        if self.frames == 0 {
            return None;
        }
        let p = if p.is_nan() { 0.0 } else { p.clamp(0.0, 1.0) };
        let rank = ((p * self.frames as f64).ceil() as u64).clamp(1, self.frames);
        let mut cumulative = 0;
        for (i, &count) in self.buckets.iter().enumerate() {
            cumulative += count;
            if cumulative >= rank {
                return Some(bucket_value(i));
            }
        }
        None
    }

    /// `None` below [`MIN_STATS_FRAMES`]. Low = slowest 1% (99th percentile frame time),
    /// high = fastest 1%.
    pub fn summary(&self) -> Option<StatsSummary> {
        if self.frames < MIN_STATS_FRAMES || self.total_secs <= 0.0 {
            return None;
        }
        let slow = self.percentile_frame_secs(0.99)?;
        let fast = self.percentile_frame_secs(0.01)?;
        Some(StatsSummary {
            frames: self.frames,
            play_secs: self.total_secs,
            avg_fps: self.frames as f64 / self.total_secs,
            low_1pct_fps: 1.0 / slow,
            high_1pct_fps: 1.0 / fast,
        })
    }

    /// Non-empty buckets as `(index, count)`, ascending by index.
    pub fn to_sparse(&self) -> Vec<(u32, u64)> {
        self.buckets
            .iter()
            .enumerate()
            .filter(|(_, &count)| count > 0)
            .map(|(i, &count)| (i as u32, count))
            .collect()
    }

    /// Rebuilds a histogram; out-of-range indexes are skipped and `frames` is recomputed
    /// from the kept buckets.
    pub fn from_sparse(buckets: &[(u32, u64)], total_secs: f64) -> FrameHistogram {
        let mut h = FrameHistogram::new();
        for &(index, count) in buckets {
            if let Some(slot) = h.buckets.get_mut(index as usize) {
                *slot += count;
                h.frames += count;
            }
        }
        h.total_secs = total_secs;
        h
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled(n: usize, secs: f64) -> FrameHistogram {
        let mut h = FrameHistogram::new();
        for _ in 0..n {
            h.add(secs);
        }
        h
    }

    fn within_pct(actual: f64, expected: f64, pct: f64) -> bool {
        (actual - expected).abs() <= expected * pct / 100.0
    }

    #[test]
    fn steady_60_fps() {
        let s = filled(6000, 1.0 / 60.0).summary().unwrap();
        assert!((s.avg_fps - 60.0).abs() < 0.01, "avg {}", s.avg_fps);
        assert!(
            within_pct(s.low_1pct_fps, 60.0, 1.0),
            "low {}",
            s.low_1pct_fps
        );
        assert!(
            within_pct(s.high_1pct_fps, 60.0, 1.0),
            "high {}",
            s.high_1pct_fps
        );
    }

    #[test]
    fn stutter_shows_in_1pct_low() {
        let mut h = filled(980, 1.0 / 144.0);
        for _ in 0..20 {
            h.add(1.0 / 30.0);
        }
        let s = h.summary().unwrap();
        assert!(
            within_pct(s.low_1pct_fps, 30.0, 1.0),
            "low {}",
            s.low_1pct_fps
        );
        assert!(
            within_pct(s.high_1pct_fps, 144.0, 1.0),
            "high {}",
            s.high_1pct_fps
        );
        let expected_avg = 1000.0 / (980.0 / 144.0 + 20.0 / 30.0);
        assert!((s.avg_fps - expected_avg).abs() < 0.1, "avg {}", s.avg_fps);
    }

    #[test]
    fn below_100_frames_has_no_summary() {
        assert!(filled(99, 0.01).summary().is_none());
        assert!(filled(100, 0.01).summary().is_some());
    }

    #[test]
    fn ignores_invalid_frame_times() {
        let mut h = FrameHistogram::new();
        for v in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            h.add(v);
        }
        assert_eq!(h.frames(), 0);
        assert_eq!(h.total_secs(), 0.0);
    }

    #[test]
    fn huge_frame_time_lands_in_top_bucket_but_counts_real_time() {
        let mut h = FrameHistogram::new();
        h.add(30.0);
        assert_eq!(h.frames(), 1);
        assert_eq!(h.total_secs(), 30.0);
        let sparse = h.to_sparse();
        assert_eq!(sparse.len(), 1);
        let mut other = FrameHistogram::new();
        other.add(1000.0);
        assert_eq!(
            other.to_sparse(),
            sparse,
            "all huge times share the top bucket"
        );
    }

    #[test]
    fn merge_equals_adding_both() {
        // Dyadic frame times (exact in binary) so f64 sums are order-independent and `==` is exact.
        let mut a = filled(300, 0.25);
        let b = filled(200, 0.125);
        let mut both = a.clone();
        for _ in 0..200 {
            both.add(0.125);
        }
        a.merge(&b);
        assert_eq!(a, both);
    }

    #[test]
    fn sparse_roundtrip_is_lossless() {
        let mut h = filled(500, 1.0 / 60.0);
        for _ in 0..50 {
            h.add(1.0 / 20.0);
        }
        let sparse = h.to_sparse();
        assert_eq!(sparse.len(), 2);
        assert_eq!(FrameHistogram::from_sparse(&sparse, h.total_secs()), h);
    }

    #[test]
    fn from_sparse_skips_out_of_range_indexes() {
        let h = FrameHistogram::from_sparse(&[(5, 10), (u32::MAX, 99), (100_000, 7)], 1.0);
        assert_eq!(h.frames(), 10);
        assert_eq!(h.to_sparse(), vec![(5, 10)]);
    }

    #[test]
    fn percentile_edge_cases() {
        assert_eq!(FrameHistogram::new().percentile_frame_secs(0.5), None);
        let h = filled(10, 0.01);
        let lo = h.percentile_frame_secs(-3.0).unwrap();
        let hi = h.percentile_frame_secs(7.0).unwrap();
        assert_eq!(lo, h.percentile_frame_secs(0.0).unwrap());
        assert_eq!(hi, h.percentile_frame_secs(1.0).unwrap());
        assert!(within_pct(lo, 0.01, 1.0));
    }
}
