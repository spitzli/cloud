//! Bounded exponential delay for readiness/deletion polls: fast first checks, less API load on long waits.
use std::time::Duration;

const START: Duration = Duration::from_millis(250);
const CAP: Duration = Duration::from_secs(2);

pub(crate) struct Backoff {
    next: Duration,
}

impl Backoff {
    pub(crate) fn new() -> Self {
        Self { next: START }
    }

    /// Sleeps for the current delay, never past `deadline`, then grows it by 1.5x up to the cap.
    pub(crate) async fn wait_until(&mut self, deadline: Option<tokio::time::Instant>) {
        let mut delay = self.advance();
        if let Some(deadline) = deadline {
            delay = delay.min(deadline.saturating_duration_since(tokio::time::Instant::now()));
        }
        tokio::time::sleep(delay).await;
    }

    fn advance(&mut self) -> Duration {
        let current = self.next;
        self.next = (current * 3 / 2).min(CAP);
        current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grows_to_cap() {
        let mut b = Backoff::new();
        let seq: Vec<_> = (0..8).map(|_| b.advance().as_millis()).collect();
        assert_eq!(seq, [250, 375, 562, 843, 1265, 1898, 2000, 2000]);
    }
}
