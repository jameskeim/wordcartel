//! Top-level kernel law (spec §11.2): a random sequence of edits, each committed
//! with selection mapping, fully reverses under repeated undo back to the original
//! text — and every intermediate selection stays within document bounds.
use proptest::prelude::*;
use wordcartel_core::buffer::TextBuffer;
use wordcartel_core::change::ChangeSet;
use wordcartel_core::history::{History, Transaction};
use wordcartel_core::selection::Selection;

fn snap(s: &str, mut i: usize) -> usize {
    while i < s.len() && !s.is_char_boundary(i) {
        i -= 1;
    }
    i.min(s.len())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn undo_all_restores_original(
        start in ".{0,20}",
        ops in proptest::collection::vec((0usize..30, ".{0,4}", any::<bool>()), 0..12),
    ) {
        let original = start.clone();
        let mut buf = TextBuffer::from_str(&start);
        let mut hist = History::default();
        let mut sel = Selection::single(0);

        for (pos, ins, is_insert) in ops {
            let len = buf.len();
            let at = snap(&buf.to_string(), pos.min(len));
            let cs = if is_insert || at >= len {
                ChangeSet::insert(at, &ins, len)
            } else {
                let end = snap(&buf.to_string(), (at + 1).min(len));
                ChangeSet::delete(at..end, len)
            };
            sel = hist.commit(Transaction::new(cs), &mut buf, sel.clone());
            // selection stays in bounds after every commit
            prop_assert!(sel.primary().head <= buf.len());
        }

        // undo everything
        while hist.undo(&mut buf).is_some() {}
        prop_assert_eq!(buf.to_string(), original);
    }
}
