//! Failure retry floor, separate from the ordinary edit-driven checkpoint cadence.
use crate::editor::Editor;

const RETRY_DELAY_MS: u64 = 30_000;

#[derive(Debug, Clone, Default)]
pub(crate) enum RetryState {
    #[default]
    Ready,
    NeedsClock,
    After(u64),
}
impl RetryState {
    pub(crate) fn failed(&mut self) { *self = Self::NeedsClock; }
    pub(crate) fn succeeded(&mut self) { *self = Self::Ready; }
    pub(crate) fn ready(&self, now: u64) -> bool {
        match self { Self::Ready => true, Self::NeedsClock => false, Self::After(at) => now >= *at }
    }
    pub(crate) fn constrain(&self, due: u64) -> u64 {
        match self { Self::After(at) => due.max(*at), Self::Ready | Self::NeedsClock => due }
    }
}

/// Merge closures have no clock. Arm once at the next foreground boundary, so even
/// a worker that failed after a long delay receives the full retry interval.
pub(crate) fn arm_retries(editor: &mut Editor, now: u64) {
    for buffer in &mut editor.buffers {
        if matches!(buffer.recovery_retry, RetryState::NeedsClock) {
            buffer.recovery_retry = RetryState::After(now.saturating_add(RETRY_DELAY_MS));
        }
    }
}
