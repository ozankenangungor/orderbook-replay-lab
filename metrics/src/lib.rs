use std::time::{Duration, Instant};

use hdrhistogram::Histogram;

#[derive(Debug, Clone)]
pub struct LatencyStats {
    histogram: Histogram<u64>,
}

impl LatencyStats {
    pub fn new() -> Self {
        let mut histogram =
            Histogram::<u64>::new(3).expect("histogram construction should succeed");
        histogram.auto(true);
        Self { histogram }
    }

    pub fn record(&mut self, ns: u64) {
        self.histogram
            .record(ns)
            .expect("histogram should auto-resize to fit values");
    }

    pub fn summary_string(&self) -> String {
        if self.histogram.is_empty() {
            return "count=0 p50=0 p95=0 p99=0 max=0".to_string();
        }

        let p50 = self.histogram.value_at_quantile(0.50);
        let p95 = self.histogram.value_at_quantile(0.95);
        let p99 = self.histogram.value_at_quantile(0.99);
        let max = self.histogram.max();
        let count = self.histogram.len();

        format!(
            "count={} p50={} p95={} p99={} max={}",
            count, p50, p95, p99, max
        )
    }

    pub fn count(&self) -> u64 {
        self.histogram.len()
    }
}

impl Default for LatencyStats {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct ThroughputTracker {
    window: Duration,
    window_start: Instant,
    count: u64,
}

impl ThroughputTracker {
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            window_start: Instant::now(),
            count: 0,
        }
    }

    pub fn record(&mut self, events: u64) {
        self.count = self.count.saturating_add(events);
    }

    pub fn events_per_sec(&mut self) -> Option<f64> {
        let elapsed = self.window_start.elapsed();
        if elapsed < self.window {
            return None;
        }

        let rate = self.count as f64 / elapsed.as_secs_f64();
        self.window_start = Instant::now();
        self.count = 0;
        Some(rate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_formatting_with_data() {
        let mut stats = LatencyStats::new();
        stats.record(100);
        stats.record(100);
        stats.record(100);

        assert_eq!(
            stats.summary_string(),
            "count=3 p50=100 p95=100 p99=100 max=100"
        );
    }

    #[test]
    fn basic_recording_increments_count() {
        let mut stats = LatencyStats::new();
        stats.record(10);
        stats.record(20);
        assert_eq!(stats.count(), 2);
    }
}
