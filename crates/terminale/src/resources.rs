//! System resource sampling (CPU + memory) for the bottom resource-indicator
//! strip.
//!
//! CPU and memory come from `sysinfo` and are reliable on Windows/macOS/Linux.
//! True GPU-utilisation percent is not available cross-platform, so the GPU
//! label is read straight from the wgpu adapter by the renderer.

use std::time::{Duration, Instant};
use sysinfo::System;

/// Refresh cadence. CPU% needs a non-trivial gap between refreshes to be
/// meaningful; 1s keeps the sampling cost negligible.
const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);

/// A point-in-time snapshot of system resource usage.
#[derive(Debug, Clone, Copy)]
pub struct ResourceSample {
    /// Global CPU utilisation, percent in `[0, 100]`.
    pub cpu_pct: f32,
    /// Used memory as a percent of total, `[0, 100]`.
    pub mem_pct: f32,
}

impl ResourceSample {
    const ZERO: Self = Self {
        cpu_pct: 0.0,
        mem_pct: 0.0,
    };
}

/// Periodically samples CPU and memory usage on a cheap cadence.
pub struct ResourceSampler {
    sys: System,
    last: Option<Instant>,
    sample: ResourceSample,
}

impl ResourceSampler {
    #[must_use]
    pub fn new() -> Self {
        Self {
            sys: System::new(),
            last: None,
            sample: ResourceSample::ZERO,
        }
    }

    #[must_use]
    pub fn sample(&self) -> ResourceSample {
        self.sample
    }

    /// Refresh the sample if the cadence has elapsed. Returns `true` when a new
    /// sample was taken (so the caller can request a redraw). The first tick
    /// reports 0% CPU (no delta yet); subsequent ticks are accurate.
    pub fn tick(&mut self, now: Instant) -> bool {
        if let Some(last) = self.last {
            if now.duration_since(last) < SAMPLE_INTERVAL {
                return false;
            }
        }
        self.last = Some(now);
        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();
        let total = self.sys.total_memory();
        let used = self.sys.used_memory();
        self.sample = ResourceSample {
            cpu_pct: self.sys.global_cpu_usage().clamp(0.0, 100.0),
            mem_pct: if total > 0 {
                (used as f32 / total as f32 * 100.0).clamp(0.0, 100.0)
            } else {
                0.0
            },
        };
        true
    }
}

impl Default for ResourceSampler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_respects_cadence() {
        let mut s = ResourceSampler::new();
        let t0 = Instant::now();
        assert!(s.tick(t0), "first tick always samples");
        assert!(
            !s.tick(t0),
            "same instant is within the cadence → no resample"
        );
        assert!(
            s.tick(t0 + SAMPLE_INTERVAL),
            "after the interval a new sample is taken"
        );
    }

    #[test]
    fn sample_percentages_are_bounded() {
        let mut s = ResourceSampler::new();
        s.tick(Instant::now());
        let sample = s.sample();
        assert!((0.0..=100.0).contains(&sample.cpu_pct));
        assert!((0.0..=100.0).contains(&sample.mem_pct));
    }
}
