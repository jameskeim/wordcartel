//! The untrusted/library/worker panic boundary (M4). Reused by the sync transform, the
//! ad-hoc worker threads, the job Executor, and (later) plugin call-sites.

/// Run `f`, catching a panic and returning a best-effort message instead of unwinding.
pub(crate) fn catch<T>(f: impl FnOnce() -> T) -> Result<T, String> {
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
}
