#![no_main]
use libfuzzer_sys::fuzz_target;
use wordcartel_core::test_support::snap;
use wordcartel_core::block_tree::incremental_equals_full;

fuzz_target!(|input: (String, usize, usize, String)| {
    let (old, s, e, repl) = input;
    let start = snap(&old, s % (old.len() + 1));
    let end = snap(&old, (start + (e % (old.len() - start + 1))).min(old.len())); // snap BOTH endpoints
    assert!(incremental_equals_full(&old, start..end, &repl),
            "incremental block-tree update diverged from a full reparse");
});
