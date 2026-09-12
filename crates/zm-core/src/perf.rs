use std::{collections::VecDeque, time::Duration};

const MAX_TIMING_SAMPLES: usize = 1_200;

/// A bounded window of elapsed times, with a separate lifetime sample count.
///
/// Recording does not sort or scan existing samples. Percentiles and averages
/// describe only the latest 1,200 samples and are calculated on request.
#[derive(Clone, Debug, Default)]
pub struct TimingSamples {
    samples: VecDeque<Duration>,
    total_count: u64,
}

impl TimingSamples {
    pub fn record(&mut self, elapsed: Duration) {
        if self.samples.len() == MAX_TIMING_SAMPLES {
            self.samples.pop_front();
        }
        self.samples.push_back(elapsed);
        self.total_count = self.total_count.saturating_add(1);
    }

    /// Summarizes the recent window using nearest-rank percentiles.
    pub fn summary(&self, label: &str) -> String {
        let mut sorted: Vec<_> = self.samples.iter().copied().collect();
        sorted.sort_unstable();
        let sample_count = sorted.len();
        let average_ms = if sample_count == 0 {
            0.0
        } else {
            sorted.iter().map(Duration::as_secs_f64).sum::<f64>() * 1_000.0 / sample_count as f64
        };
        let millis = |duration: Duration| duration.as_secs_f64() * 1_000.0;
        let peak = sorted.last().copied().unwrap_or_default();
        format!(
            "{label}: sample_count={sample_count} total_count={} p50_ms={:.3} p95_ms={:.3} p99_ms={:.3} avg_ms={average_ms:.3} peak_ms={:.3}\n",
            self.total_count,
            millis(percentile(&sorted, 50)),
            millis(percentile(&sorted, 95)),
            millis(percentile(&sorted, 99)),
            millis(peak),
        )
    }
}

fn percentile(sorted: &[Duration], percent: usize) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let rank = (sorted.len() * percent).div_ceil(100);
    sorted[rank.saturating_sub(1)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_window_has_finite_zero_statistics() {
        assert_eq!(
            TimingSamples::default().summary("empty"),
            "empty: sample_count=0 total_count=0 p50_ms=0.000 p95_ms=0.000 p99_ms=0.000 avg_ms=0.000 peak_ms=0.000\n"
        );
    }

    #[test]
    fn percentiles_use_nearest_rank_independent_of_record_order() {
        let mut samples = TimingSamples::default();
        for millis in (1..=100).rev() {
            samples.record(Duration::from_millis(millis));
        }
        assert_eq!(
            samples.summary("timing"),
            "timing: sample_count=100 total_count=100 p50_ms=50.000 p95_ms=95.000 p99_ms=99.000 avg_ms=50.500 peak_ms=100.000\n"
        );
        assert_eq!(samples.samples.front(), Some(&Duration::from_millis(100)));
    }

    #[test]
    fn window_evicts_old_samples_but_preserves_total_count() {
        let mut samples = TimingSamples::default();
        samples.record(Duration::from_secs(999));
        for _ in 0..MAX_TIMING_SAMPLES {
            samples.record(Duration::from_micros(500));
        }
        assert_eq!(samples.samples.len(), MAX_TIMING_SAMPLES);
        assert_eq!(
            samples.summary("recent"),
            "recent: sample_count=1200 total_count=1201 p50_ms=0.500 p95_ms=0.500 p99_ms=0.500 avg_ms=0.500 peak_ms=0.500\n"
        );
    }

    #[test]
    fn samples_can_be_shared_by_callers_with_their_own_synchronization() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<TimingSamples>();
    }
}
