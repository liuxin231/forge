use std::time::{Duration, Instant};

/// Tracks restart state for a service
pub struct RestartTracker {
    pub restart_count: u32,
    pub max_restarts: u32,
    pub restart_delay: Duration,
    pub autorestart: bool,
    pub last_start: Option<Instant>,
    pub min_uptime: Duration,
    /// After running for this long, reset restart_count
    pub stable_reset_threshold: Duration,
}

impl RestartTracker {
    pub fn new(autorestart: bool, max_restarts: u32, restart_delay_secs: u64) -> Self {
        let min_uptime = Duration::from_secs(5);
        Self {
            restart_count: 0,
            max_restarts,
            restart_delay: Duration::from_secs(restart_delay_secs),
            autorestart,
            last_start: None,
            min_uptime,
            // Reset restart count after running stably for 10x min_uptime
            stable_reset_threshold: min_uptime * 10,
        }
    }

    /// Record that the service was started
    pub fn record_start(&mut self) {
        self.last_start = Some(Instant::now());
    }

    /// Determine if we should restart after a crash
    pub fn should_restart(&mut self) -> bool {
        if !self.autorestart {
            return false;
        }

        if self.restart_count >= self.max_restarts {
            return false;
        }

        // Check if the service was running stably — if so, reset counter
        if let Some(last) = self.last_start {
            let uptime = last.elapsed();
            if uptime >= self.stable_reset_threshold {
                tracing::info!(
                    "Service ran stably for {:?}, resetting restart counter",
                    uptime
                );
                self.restart_count = 0;
            } else if uptime < self.min_uptime {
                tracing::warn!("Service crashed within min_uptime, counting as startup failure");
            }
        }

        self.restart_count += 1;
        true
    }

    /// Get the delay before next restart
    pub fn delay(&self) -> Duration {
        self.restart_delay
    }

    /// Check if the service has been marked as errored (exceeded max restarts)
    pub fn is_errored(&self) -> bool {
        self.restart_count >= self.max_restarts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_autorestart_disabled() {
        let mut tracker = RestartTracker::new(false, 10, 3);
        assert!(!tracker.should_restart());
    }

    #[test]
    fn test_basic_restart() {
        let mut tracker = RestartTracker::new(true, 3, 1);
        tracker.record_start();
        assert!(tracker.should_restart());
        assert_eq!(tracker.restart_count, 1);
    }

    #[test]
    fn test_max_restarts_reached() {
        let mut tracker = RestartTracker::new(true, 2, 1);
        tracker.record_start();
        assert!(tracker.should_restart()); // count = 1
        tracker.record_start();
        assert!(tracker.should_restart()); // count = 2
        tracker.record_start();
        assert!(!tracker.should_restart()); // count = 2 >= max 2
        assert!(tracker.is_errored());
    }

    #[test]
    fn test_max_restarts_zero_means_no_restart() {
        let mut tracker = RestartTracker::new(true, 0, 1);
        tracker.record_start();
        // max_restarts=0: restart_count(0) >= max_restarts(0), so no restart
        assert!(!tracker.should_restart());
    }

    #[test]
    fn test_delay() {
        let tracker = RestartTracker::new(true, 10, 5);
        assert_eq!(tracker.delay(), Duration::from_secs(5));
    }

    #[test]
    fn test_is_errored_initial() {
        let tracker = RestartTracker::new(true, 10, 1);
        assert!(!tracker.is_errored());
    }

    #[test]
    fn test_stable_reset_threshold() {
        let mut tracker = RestartTracker::new(true, 3, 1);
        tracker.restart_count = 2;
        // Simulate running for a long time
        tracker.last_start = Some(Instant::now() - Duration::from_secs(120));
        // Should restart and reset counter because uptime > threshold
        assert!(tracker.should_restart());
        // Counter was reset to 0, then incremented to 1
        assert_eq!(tracker.restart_count, 1);
    }

    #[test]
    fn test_rapid_crash_does_not_reset() {
        let mut tracker = RestartTracker::new(true, 3, 1);
        tracker.restart_count = 1;
        // Crash immediately
        tracker.last_start = Some(Instant::now());
        assert!(tracker.should_restart());
        // Counter was NOT reset — went from 1 to 2
        assert_eq!(tracker.restart_count, 2);
    }

    #[test]
    fn test_no_start_recorded() {
        let mut tracker = RestartTracker::new(true, 10, 1);
        // No record_start called
        assert!(tracker.should_restart());
        assert_eq!(tracker.restart_count, 1);
    }
}
