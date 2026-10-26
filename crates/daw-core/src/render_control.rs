//! Cooperative cancellation and progress shared by offline render callers.
use std::{
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
    time::{Duration, Instant},
};
pub struct RenderControl {
    pub cancelled: AtomicBool,
    pub progress: AtomicU32,
    deadline: Instant,
}
impl RenderControl {
    pub fn new(timeout: Duration) -> Self {
        Self {
            cancelled: AtomicBool::new(false),
            progress: AtomicU32::new(0),
            deadline: Instant::now() + timeout,
        }
    }
    pub fn check(&self) -> anyhow::Result<()> {
        anyhow::ensure!(!self.cancelled.load(Ordering::Relaxed), "render cancelled");
        anyhow::ensure!(Instant::now() < self.deadline, "render timed out");
        Ok(())
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}
impl Default for RenderControl {
    fn default() -> Self {
        Self::new(Duration::from_secs(300))
    }
}
