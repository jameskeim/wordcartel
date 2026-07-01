//! The untrusted/library/worker panic boundary (M4). Reused by the sync transform, the
//! ad-hoc worker threads, the job Executor, and (later) plugin call-sites.

use std::cell::Cell;

thread_local! {
    /// Set while a `catch` is active on THIS thread. The panic hook consults it
    /// (via `caught_guard_active`) to suppress its teardown for a panic that
    /// `catch` will itself handle.
    static CAUGHT_GUARD: Cell<bool> = const { Cell::new(false) };
}

/// True while a `catch` is in progress on the current thread.
pub(crate) fn caught_guard_active() -> bool {
    CAUGHT_GUARD.with(|g| g.get())
}

/// RAII: sets the guard true, restores the PREVIOUS value on drop (re-entrant safe).
struct GuardReset(bool);
impl Drop for GuardReset {
    fn drop(&mut self) {
        CAUGHT_GUARD.with(|g| g.set(self.0));
    }
}

/// Run `f`, catching a panic and returning a best-effort message instead of unwinding.
pub(crate) fn catch<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    // Establish the guard BEFORE catch_unwind so it is still live when the panic
    // hook runs at the panic site (the hook runs before unwinding reaches here).
    let _reset = CAUGHT_GUARD.with(|g| GuardReset(g.replace(true)));
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).map_err(panic_message)
}

/// Extract a human-readable string from a panic payload (best-effort).
pub(crate) fn panic_message(p: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catch_returns_ok_for_non_panicking() {
        assert_eq!(catch(|| 1 + 1).unwrap(), 2);
    }

    #[test]
    fn catch_maps_str_panic_to_message() {
        let e = catch(|| panic!("boom")).unwrap_err();
        assert_eq!(e, "boom");
    }

    #[test]
    fn catch_maps_string_panic_to_message() {
        let e = catch(|| panic!("{}", String::from("dynamic"))).unwrap_err();
        assert_eq!(e, "dynamic");
    }

    #[test]
    fn catch_maps_other_payload_to_default() {
        let e = catch(|| std::panic::panic_any(42u32)).unwrap_err();
        assert_eq!(e, "panic");
    }

    #[test]
    fn guard_is_inactive_outside_catch_and_active_inside() {
        assert!(!caught_guard_active());
        let inside = catch(caught_guard_active).unwrap();
        assert!(inside, "guard must be active inside catch");
        assert!(!caught_guard_active(), "guard restored after catch");
    }

    #[test]
    fn guard_restores_previous_value_on_nesting() {
        let inner_seen = catch(|| {
            assert!(caught_guard_active());
            let deeper = catch(caught_guard_active).unwrap();
            // after the inner catch returns, the outer guard is still active
            (deeper, caught_guard_active())
        })
        .unwrap();
        assert_eq!(inner_seen, (true, true));
        assert!(!caught_guard_active());
    }

    #[test]
    fn guard_is_restored_even_when_the_closure_panics() {
        let _ = catch(|| panic!("boom"));
        assert!(!caught_guard_active(), "guard must reset after a caught panic");
    }
}
