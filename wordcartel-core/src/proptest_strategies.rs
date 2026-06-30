//! Shared proptest strategies for the M7 property tests. cfg(test)-only (uses proptest, a dev-dep).
use proptest::prelude::*;
use crate::test_support::UNICODE_PALETTE;

/// A unicode-biased string: a sequence of palette pieces (ASCII + multibyte + combining + ZWJ + emoji).
pub fn prop_unicode_string() -> impl Strategy<Value = String> {
    proptest::collection::vec(proptest::sample::select(UNICODE_PALETTE), 0..40)
        .prop_map(|parts| parts.concat())
}
