//! When filesystems will be full at their recent rate of growth.
//!
//! Used space is sampled every [`SAMPLE_EVERY`] and kept for [`WINDOW`];
//! the slope of a least-squares line through the samples gives the rate.
//! Measured by the source (an agent, for remote hosts), so that alerts stay
//! a function of one snapshot and work wherever snapshots go.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use crate::snapshot::FilesystemUsage;

/// Time between two samples of a filesystem.
const SAMPLE_EVERY: Duration = Duration::from_secs(30);

/// How far back the rate looks.
const WINDOW: Duration = Duration::from_secs(30 * 60);

/// Shortest history a rate is computed from.
const MIN_SPAN: Duration = Duration::from_secs(10 * 60);

/// Used space of each filesystem over the last [`WINDOW`].
#[derive(Debug, Default)]
pub struct FillTracker {
    /// By filesystem key: (when, used bytes), oldest first.
    samples: HashMap<String, VecDeque<(Instant, u64)>>,
    last_sample: Option<Instant>,
}

impl FillTracker {
    /// Creates a tracker without history.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets [`FilesystemUsage::full_in_secs`] of each filesystem, sampling
    /// them first when it is time.
    pub fn annotate(&mut self, filesystems: &mut [FilesystemUsage], now: Instant) {
        let due = self
            .last_sample
            .is_none_or(|at| now.saturating_duration_since(at) >= SAMPLE_EVERY);
        if due {
            self.last_sample = Some(now);
            // Unmounted filesystems are forgotten.
            self.samples
                .retain(|key, _| filesystems.iter().any(|fs| fs_key(fs) == *key));
            for fs in filesystems.iter() {
                let samples = self.samples.entry(fs_key(fs)).or_default();
                samples.push_back((now, used_bytes(fs)));
                while samples
                    .front()
                    .is_some_and(|(at, _)| now.saturating_duration_since(*at) > WINDOW)
                {
                    samples.pop_front();
                }
            }
        }
        for fs in filesystems.iter_mut() {
            fs.full_in_secs = self
                .samples
                .get(&fs_key(fs))
                .and_then(|samples| full_in_secs(samples, fs.available_bytes));
        }
    }
}

/// A filesystem across samples: its first mount point, else its device.
fn fs_key(fs: &FilesystemUsage) -> String {
    fs.mount_points
        .first()
        .cloned()
        .unwrap_or_else(|| fs.device.clone())
}

fn used_bytes(fs: &FilesystemUsage) -> u64 {
    fs.total_bytes.saturating_sub(fs.available_bytes)
}

/// Seconds until `available` bytes are used up at the rate of `samples`:
/// `None` without enough history, when usage does not grow, or when the
/// filesystem is full already.
fn full_in_secs(samples: &VecDeque<(Instant, u64)>, available: u64) -> Option<u64> {
    let (first, _) = *samples.front()?;
    let (last, _) = *samples.back()?;
    if last.saturating_duration_since(first) < MIN_SPAN || available == 0 {
        return None;
    }
    // Least squares on (seconds since the first sample, used bytes).
    let n = samples.len() as f64;
    let points = samples.iter().map(|(at, used)| {
        (
            at.saturating_duration_since(first).as_secs_f64(),
            *used as f64,
        )
    });
    let (sum_x, sum_y) = points
        .clone()
        .fold((0.0, 0.0), |(sx, sy), (x, y)| (sx + x, sy + y));
    let (mean_x, mean_y) = (sum_x / n, sum_y / n);
    let (cov, var) = points.fold((0.0, 0.0), |(c, v), (x, y)| {
        (
            c + (x - mean_x) * (y - mean_y),
            v + (x - mean_x) * (x - mean_x),
        )
    });
    let bytes_per_sec = cov / var;
    (bytes_per_sec > 0.0).then(|| (available as f64 / bytes_per_sec).round() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1 << 30;

    fn fs(available: u64) -> FilesystemUsage {
        FilesystemUsage {
            device: "/dev/md0".to_owned(),
            mount_points: vec!["/srv".to_owned()],
            total_bytes: 100 * GIB,
            available_bytes: available,
            ..Default::default()
        }
    }

    #[test]
    fn predicts_from_steady_growth_only() {
        let start = Instant::now();
        let mut tracker = FillTracker::new();
        // 1 GiB a minute with 40 GiB left; samples every 30 s for 10 min.
        let mut list = Vec::new();
        for step in 0..=20u64 {
            list = vec![fs(60 * GIB - step * GIB / 2)];
            tracker.annotate(&mut list, start + SAMPLE_EVERY * step as u32);
            if step < 20 {
                assert_eq!(list[0].full_in_secs, None, "too short at step {step}");
            }
        }
        // 50 GiB left at 1 GiB a minute: 50 minutes.
        assert_eq!(list[0].full_in_secs, Some(50 * 60));

        // Between samples the last estimate stays, with the current space.
        let mut later = vec![fs(50 * GIB)];
        tracker.annotate(
            &mut later,
            start + SAMPLE_EVERY * 20 + Duration::from_secs(1),
        );
        assert_eq!(later[0].full_in_secs, Some(50 * 60));

        // Shrinking usage: no prediction.
        let mut tracker = FillTracker::new();
        for step in 0..=40u64 {
            list = vec![fs(10 * GIB + step * GIB / 4)];
            tracker.annotate(&mut list, start + SAMPLE_EVERY * step as u32);
        }
        assert_eq!(list[0].full_in_secs, None);
    }

    #[test]
    fn forgets_old_samples_and_unmounted_filesystems() {
        let start = Instant::now();
        let mut tracker = FillTracker::new();
        // Fast growth an hour ago, flat since: the old burst is forgotten.
        for step in 0..=120u64 {
            let used = if step < 20 { step } else { 20 };
            let mut list = vec![fs(80 * GIB - used * GIB)];
            tracker.annotate(&mut list, start + SAMPLE_EVERY * step as u32);
            if step == 120 {
                assert_eq!(list[0].full_in_secs, None);
            }
        }
        assert!(tracker.samples["/srv"].len() <= 61);
        tracker.annotate(&mut [], start + SAMPLE_EVERY * 121);
        assert!(tracker.samples.is_empty());
    }
}
