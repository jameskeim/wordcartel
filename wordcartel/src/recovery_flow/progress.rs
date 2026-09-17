use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
#[derive(Debug, Default)]
pub(super) struct Progress { state: AtomicU8, durable: AtomicBool, ack: std::sync::OnceLock<crate::recovery_store::CheckpointAck> }
impl Progress {
    pub fn cancel(&self) -> bool { self.state.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire).is_ok() }
    pub fn authorize(&self) -> bool {
        self.durable.store(true, Ordering::Release);
        self.state.compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire).is_ok()
    }
    pub fn record_ack(&self, ack: crate::recovery_store::CheckpointAck) { let _ = self.ack.set(ack); }
    pub fn ack(&self) -> Option<&crate::recovery_store::CheckpointAck> { self.ack.get() }
    pub fn protected(&self) -> bool { self.durable.load(Ordering::Acquire) }
    pub fn finish(&self) { self.state.store(3, Ordering::Release); }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovery_handoff_cancel_wins_before_authorization_without_losing_durable_progress() {
        let progress = Progress::default();
        assert!(progress.cancel());
        assert!(!progress.authorize());
        assert!(progress.protected());
        progress.finish();
        assert!(!progress.cancel());
    }
    #[test]
    fn recovery_handoff_authorization_wins_and_cannot_be_revoked_after_sync() {
        let progress = Progress::default();
        assert!(progress.authorize());
        assert!(progress.protected());
        assert!(!progress.cancel());
        progress.finish();
        assert!(progress.protected());
    }
}
