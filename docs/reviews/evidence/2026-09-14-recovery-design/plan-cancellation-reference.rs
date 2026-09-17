use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

const PENDING: u8 = 0;
const CANCELLED: u8 = 1;
const RETIRING: u8 = 2;
const FINISHED: u8 = 3;

#[derive(Default)]
struct HandoffProgress {
    state: AtomicU8,
    successor_durable: AtomicBool,
}

impl HandoffProgress {
    fn cancel(&self) -> bool {
        self.state.compare_exchange(PENDING, CANCELLED,
            Ordering::AcqRel, Ordering::Acquire).is_ok()
    }
    fn authorize_retirement_after_sync(&self) -> bool {
        self.successor_durable.store(true, Ordering::Release);
        self.state.compare_exchange(PENDING, RETIRING,
            Ordering::AcqRel, Ordering::Acquire).is_ok()
    }
    fn is_protected(&self) -> bool {
        self.successor_durable.load(Ordering::Acquire)
    }
    fn finish(&self) { self.state.store(FINISHED, Ordering::Release); }
}
#[test] fn cancel_wins_before_retirement() {
 let p=HandoffProgress::default();
 assert!(p.cancel()); assert!(!p.authorize_retirement_after_sync());
 assert!(p.is_protected()); p.finish(); assert!(!p.cancel());
}
#[test] fn retirement_wins_after_sync() {
 let p=HandoffProgress::default();
 assert!(!p.is_protected()); assert!(p.authorize_retirement_after_sync());
 assert!(!p.cancel()); assert!(p.is_protected()); p.finish();
}
