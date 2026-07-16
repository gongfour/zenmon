//! Bounded termination for streaming/watch loops.
//!
//! Each command writes its own `tokio::select!` loop (its item source and
//! Ctrl+C branch differ), but shares the termination bookkeeping here:
//! [`Budget`] tracks the count/deadline and [`sleep_until_opt`] is the deadline
//! branch. The deadline uses `tokio::time::Instant`, so the loop shape is
//! deterministically testable with paused time.

use std::time::Duration;
use tokio::time::Instant;

/// Termination bounds for a watch/stream loop. `None` on a field means that
/// dimension is unbounded.
#[derive(Debug, Clone, Copy, Default)]
pub struct Bounds {
    pub max_count: Option<u64>,
    pub duration: Option<Duration>,
}

impl Bounds {
    pub fn new(max_count: Option<u64>, duration: Option<Duration>) -> Self {
        Self {
            max_count,
            duration,
        }
    }
}

/// Tracks how much of a watch loop's count/duration budget remains.
///
/// Start it once the subscriber/watch is ready (the duration clock begins at
/// [`Budget::start`]). Read [`Budget::deadline`] for the deadline branch and
/// call [`Budget::record`] after each emitted item; it returns `true` when the
/// count budget is exhausted and the loop should stop.
pub struct Budget {
    remaining: Option<u64>,
    deadline: Option<Instant>,
}

impl Budget {
    pub fn start(bounds: Bounds) -> Self {
        Self {
            remaining: bounds.max_count,
            deadline: bounds.duration.map(|d| Instant::now() + d),
        }
    }

    /// The deadline instant (Copy), or `None` if the loop is time-unbounded.
    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// Record one emitted item; returns `true` if the count budget is now
    /// exhausted. Unbounded count never exhausts.
    pub fn record(&mut self) -> bool {
        match &mut self.remaining {
            Some(r) => {
                *r = r.saturating_sub(1);
                *r == 0
            }
            None => false,
        }
    }
}

/// Sleep until `deadline`, or never (for a time-unbounded loop). A free
/// function so it borrows nothing from the [`Budget`]; callers pass
/// `budget.deadline()` as a Copy value, leaving the budget free to `record()`.
pub async fn sleep_until_opt(deadline: Option<Instant>) {
    match deadline {
        Some(dl) => tokio::time::sleep_until(dl).await,
        None => std::future::pending::<()>().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::UnboundedReceiver;

    #[test]
    fn record_counts_down_to_exhaustion() {
        let mut b = Budget::start(Bounds::new(Some(2), None));
        assert!(!b.record(), "2 -> 1, not exhausted");
        assert!(b.record(), "1 -> 0, exhausted");
    }

    #[test]
    fn record_unbounded_never_exhausts() {
        let mut b = Budget::start(Bounds::new(None, None));
        for _ in 0..1000 {
            assert!(!b.record());
        }
    }

    /// Representative bounded loop mirroring the command loops (minus Ctrl+C,
    /// which is just another break branch). Drains `rx` under `bounds`.
    async fn drive(bounds: Bounds, rx: &mut UnboundedReceiver<u64>) -> Vec<u64> {
        let mut budget = Budget::start(bounds);
        let mut out = Vec::new();
        loop {
            let deadline = budget.deadline();
            tokio::select! {
                biased;
                _ = sleep_until_opt(deadline) => break,
                item = rx.recv() => match item {
                    Some(x) => {
                        out.push(x);
                        if budget.record() {
                            break;
                        }
                    }
                    None => break,
                }
            }
        }
        out
    }

    #[tokio::test(start_paused = true)]
    async fn count_termination_stops_at_max() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<u64>();
        for i in 1..=5 {
            tx.send(i).unwrap();
        }
        let out = drive(Bounds::new(Some(3), None), &mut rx).await;
        assert_eq!(out, vec![1, 2, 3]);
    }

    #[tokio::test(start_paused = true)]
    async fn duration_termination_with_zero_items() {
        // tx stays alive so recv() pends forever; only the deadline can end it.
        let (_tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<u64>();
        let out = drive(Bounds::new(None, Some(Duration::from_secs(5))), &mut rx).await;
        assert!(out.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn both_set_count_wins() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<u64>();
        for i in 1..=10 {
            tx.send(i).unwrap();
        }
        let out = drive(Bounds::new(Some(2), Some(Duration::from_secs(10))), &mut rx).await;
        assert_eq!(out, vec![1, 2]);
    }

    #[tokio::test(start_paused = true)]
    async fn both_set_duration_wins() {
        let (_tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<u64>();
        let out = drive(Bounds::new(Some(100), Some(Duration::from_secs(1))), &mut rx).await;
        assert!(out.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn stream_close_stops_loop() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<u64>();
        tx.send(1).unwrap();
        tx.send(2).unwrap();
        drop(tx);
        let out = drive(Bounds::new(None, None), &mut rx).await;
        assert_eq!(out, vec![1, 2]);
    }
}
