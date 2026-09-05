use std::time::Duration;

/// Loading may exceed the timeout in total; only inactivity is a failure.
#[derive(Default)]
pub struct StartupWatchdog {
    last_progress: Duration,
}
impl StartupWatchdog {
    pub fn progress(&mut self, elapsed: Duration) {
        self.last_progress = self.last_progress.max(elapsed);
    }
    pub fn stalled(&self, elapsed: Duration, limit: Duration) -> bool {
        elapsed.saturating_sub(self.last_progress) >= limit
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn long_active_startup_is_not_a_timeout() {
        let mut watchdog = StartupWatchdog::default();
        let limit = Duration::from_secs(90);
        for seconds in [40, 80, 120, 160] {
            watchdog.progress(Duration::from_secs(seconds));
            assert!(!watchdog.stalled(Duration::from_secs(seconds + 1), limit));
        }
        assert!(watchdog.stalled(Duration::from_secs(250), limit));
    }
    #[test]
    fn progress_received_on_deadline_prevents_false_timeout() {
        let mut watchdog = StartupWatchdog::default();
        let now = Duration::from_secs(110);
        watchdog.progress(now);
        assert!(!watchdog.stalled(now, Duration::from_secs(90)));
    }
}
