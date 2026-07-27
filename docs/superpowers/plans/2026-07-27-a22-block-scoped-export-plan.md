# A22 — Block-scoped export: implementation plan

**Branch:** `effort-a22-block-scoped-export`. **Spec:** `docs/superpowers/specs/2026-07-27-a22-block-scoped-export-design.md` at `c0bbcae` (gate-clean; binding). **Decisions:** `scratchpad/a22/decisions.md` D1–D5 (binding).

Seven tasks, each TDD (failing test → implementation → green) and each independently
committable: **`cargo test --workspace` is green at every task boundary** — the compiler-forced
reshape (spec §8) lands whole in Task 1 so no task hands a broken build to the next (the E10 /
effort-④ lesson: no designed-in intermediate red).

## Ground rules (every task)

- **Hand-formatted repo. `cargo fmt` is FORBIDDEN.** Match the neighbouring code's indentation,
  wrapping, and import grouping by hand. Em-dashes in prose comments/strings are either a
  literal `—` or `\u{2014}` — copy whichever the neighbouring lines use.
- **Verification per task:** `cargo test --workspace` green; `cargo build` and
  `cargo test --no-run` warning-free for touched crates; `cargo clippy --workspace
  --all-targets` clean. All new `#[allow(clippy::…)]` are item-local with a one-line reason.
- **Anchor by symbol name, never line number.** Locate with grep/`workspaceSymbol`; the line
  anchors in this plan are as-observed hints only.
- For compile/usage questions on code you are editing, trust `cargo` + `grep`, never an
  editor's stale "unused"/"undefined" diagnostic.
- **Every test names the broken implementation it rules out** (in its comment or assertion
  message). A test that cannot name one does not go in.
- Commit at the end of your task with the project trailers (see `CLAUDE.md`). Do not push.

## Command-surface contract

Per spec §9/§9.1 (decision D5): **N/A — argued.** No command added/removed/renamed; no
user-settable option (`ExportScope` is flow context, not a setting — no `SettingsSnapshot`
field, no config key); palette/menu/hints untouched. Law 10 is satisfied TODAY by the existing
registered `block_write` command — a plugin reaches block-scoped export exactly as it reaches
whole-document export: dispatch a command, a picker opens. Parameterized picker-free export
stays future for both scopes. No task in this plan may register, rename, or re-categorize a
command; if an implementer believes their task needs to, STOP and escalate.

## §11 test-case → task ownership

| Case | Owner | Case | Owner |
|---|---|---|---|
| T1 (4 cases) | Task 1 | T8 (3 fixtures) | Task 4 |
| T2/T3 purpose-shape | Task 1 (extended in 3) | T9 | Task 3 |
| T2/T3 wording | Task 3 | T10 | Task 5 |
| T4 (2 fixtures) | Task 2 | T11 | Task 1 |
| T5 + T13a | Task 1 | T12a/b/c | Task 6 |
| T6 | Task 1 | T13b/c | Task 5 |
| T7 | Task 1 | cancel-tidy test | Task 7 |

---

## Task 1 — the reshape: `ExportScope`/`origin` plumbed end-to-end, dispatch verifies

**The one big task, by design (spec §8):** the `DestinationPurpose` reshape does not compile
piecemeal, so every compiler-forced site lands here together with the dispatch semantics
(`resolve_export_input` + refusals + `bool` return). Chrome wording, the probe gate, the
WriteBlock origin-verify, and `apply_export_done` are NOT here — they are later tasks and
nothing in this task's tests asserts them.

**Files:** `wordcartel/src/{file_browser.rs, export.rs, editor.rs, blocks_marked.rs,
file_browser_commit.rs, prompts.rs, render_overlays.rs, app.rs, e2e.rs}`.

### 1a. Failing tests first (write all, watch them fail to COMPILE, then implement)

In `export.rs` `#[cfg(test)] mod tests` (T1, T5+T13a, T11 — plus T7 as a migration of the
existing seed test):

```rust
    // T1 — the pure seam. Each case flips exactly ONE conjunct of
    // `scope carried ∧ origin unchanged ∧ mark present` (spec §11 E11 rule).
    #[test]
    fn resolve_export_input_whole_document_reads_everything() {
        let e = crate::editor::Editor::new_from_text("AAA\nBBB\nCCC\n", None, (80, 24));
        let id = e.active().id;
        assert_eq!(resolve_export_input(&e, ExportScope::WholeDocument, id),
            Ok("AAA\nBBB\nCCC\n".to_owned()));
    }

    #[test]
    fn resolve_export_input_marked_block_reads_only_the_block() {
        let mut e = crate::editor::Editor::new_from_text("AAA\nBBB\nCCC\n", None, (80, 24));
        let id = e.active().id;
        // hidden: true on purpose — pins the "hidden is display-only, ignored by content
        // reads" call (spec §2): an implementation consulting `hidden` skips the slice.
        e.active_mut().marked_block =
            Some(crate::editor::MarkedBlock { start: 4, end: 8, hidden: true });
        assert_eq!(resolve_export_input(&e, ExportScope::MarkedBlock, id),
            Ok("BBB\n".to_owned()),
            "the scope-dropped implementation returns the whole text here");
    }

    #[test]
    fn resolve_export_input_refuses_when_the_mark_is_gone() {
        let e = crate::editor::Editor::new_from_text("AAA\nBBB\n", None, (80, 24));
        let id = e.active().id;
        assert_eq!(resolve_export_input(&e, ExportScope::MarkedBlock, id),
            Err(ExportRefusal::NoMarkedBlock),
            "a whole-document-fallback implementation returns Ok(whole) — D2 forbids it");
    }

    #[test]
    fn resolve_export_input_refuses_when_the_buffer_changed() {
        let mut e = crate::editor::Editor::new_from_text("AAA\nBBB\n", None, (80, 24));
        let a = e.active().id;
        e.active_mut().marked_block =
            Some(crate::editor::MarkedBlock { start: 0, end: 4, hidden: false });
        crate::workspace::new_empty_buffer(&mut e); // appends AND switches active
        assert_ne!(e.active().id, a, "fixture: the active buffer really changed");
        assert_eq!(resolve_export_input(&e, ExportScope::MarkedBlock, a),
            Err(ExportRefusal::BufferChanged),
            "an origin-ignoring implementation reads the NEW active buffer and refuses \
             with NoMarkedBlock (or slices the wrong buffer) instead");
    }

    // T5 — dispatch refusals. The bool return is the scheduling-free discriminator: a
    // wrongly-dispatching implementation returns true and fails HERE, before any race.
    #[test]
    fn do_export_refuses_block_scope_with_no_mark_and_dispatches_nothing() {
        let mut e = crate::editor::Editor::new_from_text("# hi\n", None, (80, 24));
        let id = e.active().id;
        let (tx, rx) = std::sync::mpsc::channel();
        let d = crate::test_support::scratch_dir("a22-t5-refuse");
        let target = d.join("out.html");
        let dispatched = do_export(&mut e, "html", &target, &tx, false,
            ExportScope::MarkedBlock, id, crate::test_support::test_fs());
        assert!(!dispatched, "a wrongly-dispatching implementation returns true");
        assert_eq!(e.status_text(), "no marked block — export cancelled");
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Warning);
        assert_eq!(e.status().unwrap().lifetime(), crate::status::StatusLifetime::Sticky);
        // Deterministic absence, no timeout: drop OUR sender; a spawned worker would hold
        // a clone and its guaranteed single send would arrive as Ok — a wrong
        // implementation fails this recv deterministically once its worker finishes,
        // and a correct one errs Disconnected immediately.
        drop(tx);
        assert!(rx.recv().is_err(), "no worker may exist to ever send ExportDone");
    }

    #[test]
    fn do_export_refuses_when_the_buffer_changed() {
        let mut e = crate::editor::Editor::new_from_text("AAA\nBBB\n", None, (80, 24));
        let a = e.active().id;
        e.active_mut().marked_block =
            Some(crate::editor::MarkedBlock { start: 0, end: 4, hidden: false });
        crate::workspace::new_empty_buffer(&mut e);
        let (tx, rx) = std::sync::mpsc::channel();
        let d = crate::test_support::scratch_dir("a22-t5-switched");
        let dispatched = do_export(&mut e, "html", &d.join("out.html"), &tx, false,
            ExportScope::MarkedBlock, a, crate::test_support::test_fs());
        assert!(!dispatched, "an origin-re-deriving implementation dispatches from B");
        assert_eq!(e.status_text(), "buffer changed — export cancelled");
        drop(tx);
        assert!(rx.recv().is_err());
    }

    // T5 positive control + T13a — proves `false`/no-message is a discriminating reading,
    // and pins the do_export → Msg::ExportDone scope hand-off.
    #[test]
    fn do_export_block_scope_dispatches_and_the_message_carries_the_scope() {
        let mut e = crate::editor::Editor::new_from_text("AAA\nBBB\n", None, (80, 24));
        let id = e.active().id;
        e.active_mut().marked_block =
            Some(crate::editor::MarkedBlock { start: 0, end: 4, hidden: false });
        let (tx, rx) = std::sync::mpsc::channel();
        let d = crate::test_support::scratch_dir("a22-t5-control");
        let dispatched = do_export(&mut e, "html", &d.join("out.html"), &tx, false,
            ExportScope::MarkedBlock, id, crate::test_support::test_fs());
        assert!(dispatched, "mark present + origin intact must dispatch");
        // BOUNDED receive (the file_browser_commit/e2e precedent): guarded_export
        // guarantees a spawned worker sends exactly one ExportDone, pandoc or no pandoc.
        let msg = rx.recv_timeout(std::time::Duration::from_secs(10))
            .expect("the worker always sends exactly one ExportDone");
        match msg {
            crate::app::Msg::ExportDone { scope, .. } => assert_eq!(scope,
                ExportScope::MarkedBlock,
                "T13a: a default-substituting message construction reads WholeDocument"),
            other => panic!("expected ExportDone, got {other:?}"),
        }
    }

    // T11 — the reason scope is a FLAG: the read follows the funnel's remap.
    #[test]
    fn resolve_export_input_reads_the_remapped_mark_not_stored_offsets() {
        struct TestClock(u64);
        impl wordcartel_core::history::Clock for TestClock { fn now_ms(&self) -> u64 { self.0 } }
        let mut e = crate::editor::Editor::new_from_text("hello world\n", None, (40, 10));
        let id = e.active().id;
        e.active_mut().marked_block =
            Some(crate::editor::MarkedBlock { start: 6, end: 11, hidden: false }); // "world"
        let doc_len = e.active().document.buffer.len();
        let (cs, edit) = crate::commands::build_multi_replace(&[(0, 0, "pre ".into())], doc_len);
        let txn = wordcartel_core::history::Transaction::new(cs)
            .with_selection(wordcartel_core::selection::Selection::single(0));
        let _ = e.apply(txn, edit, wordcartel_core::history::EditKind::Other, &TestClock(0));
        assert_eq!(resolve_export_input(&e, ExportScope::MarkedBlock, id),
            Ok("world".to_owned()),
            "a stored-offsets design still slices [6,11) and returns 'orld\\n'-shifted bytes");
    }
```

In `prompts.rs` tests (T6):

```rust
    // T6 — confirm-boundary scope carriage, seeded side. The STATUS is the load-bearing
    // synchronous assertion (spec §11): a confirm boundary that degrades scope to
    // WholeDocument dispatches happily and leaves this status unset.
    #[test]
    fn overwrite_export_confirm_refuses_when_block_scope_finds_no_mark() {
        let mut e = crate::editor::Editor::new_from_text("# hi\n", None, (80, 24));
        let origin = e.active().id;
        let (tx, _rx) = std::sync::mpsc::channel();
        let d = crate::test_support::scratch_dir("a22-t6");
        let target = d.join("out.html");
        std::fs::write(&target, b"OLD").expect("existing target");
        e.pending_export = Some(crate::export::PendingExport {
            ext: "html".into(), target,
            scope: crate::export::ExportScope::MarkedBlock, origin });
        // The mark is deliberately ABSENT — the conjunct under test.
        let ex = crate::jobs::InlineExecutor::default();
        crate::prompts::resolve_prompt(crate::prompt::PromptAction::OverwriteExport, &mut e,
            &ex, &crate::test_support::TestClock(0), &tx, &crate::test_support::test_fs());
        assert_eq!(e.status_text(), "no marked block — export cancelled");
    }
    // Positive-control twin: same seed, mark PRESENT → bounded recv yields ExportDone.
    #[test]
    fn overwrite_export_confirm_dispatches_when_the_mark_is_present() {
        let mut e = crate::editor::Editor::new_from_text("AAA\nBBB\n", None, (80, 24));
        let origin = e.active().id;
        e.active_mut().marked_block =
            Some(crate::editor::MarkedBlock { start: 0, end: 4, hidden: false });
        let (tx, rx) = std::sync::mpsc::channel();
        let d = crate::test_support::scratch_dir("a22-t6-ctrl");
        let target = d.join("out.html");
        std::fs::write(&target, b"OLD").expect("existing target");
        e.pending_export = Some(crate::export::PendingExport {
            ext: "html".into(), target,
            scope: crate::export::ExportScope::MarkedBlock, origin });
        let ex = crate::jobs::InlineExecutor::default();
        crate::prompts::resolve_prompt(crate::prompt::PromptAction::OverwriteExport, &mut e,
            &ex, &crate::test_support::TestClock(0), &tx, &crate::test_support::test_fs());
        assert!(rx.recv_timeout(std::time::Duration::from_secs(10)).is_ok(),
            "the confirm path must dispatch when scope's conjuncts all hold");
    }
```

In `file_browser_commit.rs` tests (T2/T3 purpose-shape — Task 3 adds the wording half; these
call `commit_destination` directly, and Task 2 migrates them to `_with_probe`):

```rust
    // T2-shape — the redirect DERIVES MarkedBlock and CARRIES the ^KW origin.
    #[test]
    fn writeblock_typed_redirect_derives_block_scope_and_carries_the_kw_origin() {
        let d = tmp("a22-derive");
        let mut e = crate::editor::Editor::new_from_text("body\n", None, (80, 24));
        let ex = crate::jobs::InlineExecutor::default();
        let clk = crate::test_support::TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);
        let origin = e.active().id;
        e.open_destination_picker(&fs, &tx,
            crate::file_browser::DestinationPurpose::WriteBlock { origin },
            d.clone(), "report.html".into());
        crate::file_browser_commit::commit_destination(&mut e, &fs, &ex, &clk, &tx);
        match &e.file_browser.as_ref().expect("redirect reopens as Export").mode {
            crate::file_browser::BrowseMode::Destination { purpose, .. } => assert_eq!(
                purpose, &crate::file_browser::DestinationPurpose::Export {
                    ext: "html".into(),
                    scope: crate::export::ExportScope::MarkedBlock, origin },
                "a WholeDocument-deriving redirect fails the scope; a re-capturing one \
                 is caught by the carried-origin case below"),
            other => panic!("expected destination mode, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    // T2 carried-origin — active != origin at redirect time, so carried and re-derived
    // DIFFER: a re-capturing implementation reads B here and fails.
    #[test]
    fn writeblock_redirect_carries_the_original_origin_across_a_buffer_switch() {
        let d = tmp("a22-carried");
        let mut e = crate::editor::Editor::new_from_text("body\n", None, (80, 24));
        let ex = crate::jobs::InlineExecutor::default();
        let clk = crate::test_support::TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);
        let origin = e.active().id;
        e.open_destination_picker(&fs, &tx,
            crate::file_browser::DestinationPurpose::WriteBlock { origin },
            d.clone(), "report.html".into());
        crate::workspace::new_empty_buffer(&mut e); // the pump-vector stand-in
        assert_ne!(e.active().id, origin, "fixture: carried and re-derived must differ");
        crate::file_browser_commit::commit_destination(&mut e, &fs, &ex, &clk, &tx);
        match &e.file_browser.as_ref().expect("redirect proceeds — verify is at dispatch").mode {
            crate::file_browser::BrowseMode::Destination { purpose, .. } => match purpose {
                crate::file_browser::DestinationPurpose::Export { origin: o, .. } =>
                    assert_eq!(*o, origin,
                        "§5.2: NOT re-captured — re-derivation launders the switch"),
                other => panic!("expected Export purpose, got {other:?}"),
            },
            other => panic!("expected destination mode, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&d);
    }
```

Note: `workspace::new_empty_buffer` no-ops on a reusable empty untitled buffer
(`active_is_reusable_throwaway`) — fixtures give the first buffer real content so the switch
genuinely happens; every fixture asserts `assert_ne!` before relying on it.

### 1b. Implementation (complete)

**`export.rs`** — add after `PendingExport` (which gains fields, below):

```rust
/// What an export reads: the whole document, or the active buffer's marked block.
/// A FLAG, deliberately not offsets — offsets are re-read at dispatch because background
/// merges legally edit buffers while the picker is open and `Buffer::apply` remaps the
/// mark; a stored offset pair would go stale (A22 D1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportScope {
    WholeDocument,
    MarkedBlock,
}

/// Why a block-scoped dispatch refused (A22 D2 / D4-iv). Maps 1:1 onto a status string.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ExportRefusal {
    /// The active buffer is not the one this flow started in.
    BufferChanged,
    /// Scope is MarkedBlock but the active buffer has no mark (collapsed, undone, cleared).
    NoMarkedBlock,
}

/// Resolve what this export reads, verifying the flow's captured context first.
/// Runs on the main thread at dispatch — the same moment the whole-document path already
/// snapshots `to_string()` — so the slice is coherent with the remapped mark.
pub(crate) fn resolve_export_input(
    editor: &crate::editor::Editor,
    scope: ExportScope,
    origin: crate::editor::BufferId,
) -> Result<String, ExportRefusal> {
    if editor.active().id != origin { return Err(ExportRefusal::BufferChanged); }
    match scope {
        ExportScope::WholeDocument => Ok(editor.active().document.buffer.to_string()),
        ExportScope::MarkedBlock => match editor.active().marked_block {
            Some(b) => Ok(editor.active().document.buffer.slice(b.start..b.end)),
            None => Err(ExportRefusal::NoMarkedBlock),
        },
    }
}
```

`PendingExport` becomes:

```rust
/// Stored on `Editor` while waiting for an `OverwriteExport` confirmation.
// PartialEq, Eq ADDED (spec §4.3): T12c asserts whole-value equality on
// Option<PendingExport>, which does not compile without them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingExport {
    pub ext: String,
    pub target: PathBuf,
    /// What the confirmed export will read (A22 D1) — carried across the confirm.
    pub scope: ExportScope,
    /// The flow's originating buffer (A22 D4-iv) — re-verified at dispatch.
    pub origin: crate::editor::BufferId,
}
```

`do_export` becomes (keep the existing doc comment, extend it with the return contract):

```rust
/// … existing doc comment …
///
/// Returns `true` iff an export worker was dispatched; `false` iff a refusal fired (status
/// already set). The refusal tests assert on this return — "no worker was spawned" is
/// otherwise unprovable without racing the scheduler. Production callers discard it
/// (deliberately NOT #[must_use] — the refusal has already surfaced its status).
#[allow(clippy::too_many_arguments)] // the full dispatch context in one place — mirrors
// redirect_to_export's allow; splitting it would scatter the one verify-then-read decision.
pub(crate) fn do_export(
    editor: &mut crate::editor::Editor,
    ext: &str,
    target: &Path,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>,
    overwrite_confirmed: bool,
    scope: ExportScope,
    origin: crate::editor::BufferId,
    fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
) -> bool {
    let stdin = match resolve_export_input(editor, scope, origin) {
        Ok(s) => s,
        Err(r) => {
            let text = match r {
                ExportRefusal::NoMarkedBlock => "no marked block — export cancelled",
                ExportRefusal::BufferChanged => "buffer changed — export cancelled",
            };
            editor.set_status_full(crate::status::StatusKind::Warning, text.to_owned(),
                crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
            return false;
        }
    };
    let sink = sink_for_ext(ext);
    let buffer_id = editor.active().id; // == origin here, post-verification
    let target = target.to_path_buf();
    let msg_tx = msg_tx.clone();
    let opts = ExportOpts {
        typography: editor.export_cfg.typography,
        pdf_engine: editor.export_cfg.pdf_engine.clone(),
    };
    std::thread::spawn(move || {
        let result = guarded_export(|| run_pandoc(sink, &stdin, &target, &opts, &*fs));
        let _ = msg_tx.send(crate::app::Msg::ExportDone {
            buffer_id, target, result, overwrite_confirmed, scope,
        });
    });
    true
}
```

(The body change from today: the `stdin` line was `editor.active().document.buffer.to_string()`
— everything else in the thread closure is untouched except the added `scope` field.)

`run_export_with_probe`'s picker open becomes:

```rust
    editor.open_destination_picker(fs, msg_tx,
        crate::file_browser::DestinationPurpose::Export { ext: ext.to_owned(),
            scope: ExportScope::WholeDocument, origin: editor.active().id }, dir, field);
```

**`file_browser.rs`** — the enum (keep its doc comment):

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DestinationPurpose {
    SaveAs,
    WriteBlock { origin: crate::editor::BufferId },
    Export { ext: String, scope: crate::export::ExportScope, origin: crate::editor::BufferId },
}
```

**`editor.rs`** — beside the `pending_write_block` field:

```rust
/// State crossing the write-block overwrite confirm: the resolved target plus the
/// originating buffer, verified again when the confirm fires (A22 D4-iv).
#[derive(Debug, Clone)]
pub struct PendingWriteBlock {
    pub target: std::path::PathBuf,
    pub origin: BufferId,
}
```

and `pub pending_write_block: Option<PathBuf>` → `pub pending_write_block:
Option<PendingWriteBlock>` (the `None` initializer and all `= None` clears compile unchanged).

**`blocks_marked.rs::block_write`** — the picker open becomes:

```rust
    let origin = editor.active().id;
    editor.open_destination_picker(fs, msg_tx,
        crate::file_browser::DestinationPurpose::WriteBlock { origin }, dir, String::new());
```

**`file_browser_commit.rs`:**

- `redirect_to_export`: derivation before the `open_destination_picker` call (this task; the
  probe param is Task 2's):

```rust
    // A22 D1: scope/origin derive from the flow the writer STARTED. WriteBlock carries the
    // origin captured at ^KW — deliberately NOT re-captured here, so a buffer switched
    // under the picker is caught at dispatch, not laundered. The Export arm is unreachable
    // today (extension policy never runs for an Export purpose) but stays total.
    let (scope, origin) = match purpose {
        crate::file_browser::DestinationPurpose::WriteBlock { origin } =>
            (crate::export::ExportScope::MarkedBlock, *origin),
        crate::file_browser::DestinationPurpose::SaveAs =>
            (crate::export::ExportScope::WholeDocument, editor.active().id),
        crate::file_browser::DestinationPurpose::Export { scope, origin, .. } =>
            (*scope, *origin),
    };
    editor.open_destination_picker(fs, msg_tx,
        crate::file_browser::DestinationPurpose::Export { ext, scope, origin }, dir, field);
```

- `commit_destination`'s noun match: `WriteBlock` pattern → `WriteBlock { .. }` (string
  unchanged).
- The `WriteBlock` arm (origin bound now, VERIFIED in Task 4; prompt-open restructured to
  avoid the `.expect("just set")` re-read):

```rust
                crate::file_browser::DestinationPurpose::WriteBlock { origin } => {
                    let Some(b) = editor.active().marked_block else {
                        editor.set_status(crate::status::StatusKind::Info, "no marked block");
                        return;
                    };
                    if exists {
                        editor.pending_write_block = Some(crate::editor::PendingWriteBlock {
                            target: resolved.clone(), origin });
                        editor.open_prompt(crate::prompt::Prompt::write_block_overwrite(&resolved));
                    } else {
                        crate::prompts::perform_block_write(editor, &resolved, b.start, b.end, fs);
                    }
                }
```

- The `Export` arm:

```rust
                crate::file_browser::DestinationPurpose::Export { ext, scope, origin } => {
                    if exists {
                        editor.pending_export = Some(crate::export::PendingExport {
                            ext, target: resolved.clone(), scope, origin });
                        editor.open_prompt(crate::prompt::Prompt::export_overwrite(&resolved));
                    } else {
                        crate::export::do_export(editor, &ext, &resolved, msg_tx, false,
                            scope, origin, std::sync::Arc::clone(fs));
                    }
                }
```

**`prompts.rs`:**

```rust
        PromptAction::OverwriteExport => {
            if let Some(pe) = editor.pending_export.take() {
                // User explicitly confirmed clobbering the existing target. The bool return
                // is discarded — a refusal has already set its own status (A22 §5.4).
                crate::export::do_export(editor, &pe.ext, &pe.target, msg_tx, true,
                    pe.scope, pe.origin, std::sync::Arc::clone(fs));
            }
        }
        PromptAction::OverwriteWriteBlock => {
            if let Some(t) = editor.pending_write_block.take() {
                if let Some(b) = editor.active().marked_block {
                    perform_block_write(editor, &t.target, b.start, b.end, fs);
                } else {
                    editor.set_status(crate::status::StatusKind::Info, "no marked block");
                }
            }
        }
```

**`app.rs`** — `Msg::ExportDone` gains, after `overwrite_confirmed`:

```rust
        /// What this export read (A22 D3-4): drives the completion status wording only.
        scope: crate::export::ExportScope,
```

The custom `Debug` arm and both `Msg::ExportDone { …, .. }` dispatch arms compile unchanged
(`apply_export_done` is untouched until Task 5).

**`render_overlays.rs`** — title match patterns only (wording is Task 3's):
`DestinationPurpose::WriteBlock =>` → `DestinationPurpose::WriteBlock { .. } =>`;
`DestinationPurpose::Export { ext } =>` → `DestinationPurpose::Export { ext, .. } =>`.

### 1c. Compiler-forced test migrations (recipe — the build finds every site)

Run `cargo build` and fix each error mechanically; expected set (spec §8):

- Every test constructing `DestinationPurpose::WriteBlock` → `WriteBlock { origin:
  e.active().id }` (constructed BEFORE any buffer creation the fixture does): `prompts.rs`
  three picker tests, `file_browser_commit.rs` write-block end-to-end test,
  `render_overlays.rs` title-table row.
- Every test constructing `DestinationPurpose::Export { ext }` → add `scope:
  crate::export::ExportScope::WholeDocument, origin: e.active().id` (all existing fixtures
  are palette/SaveAs-context → WholeDocument): `export.rs` seed test (this IS T7 — also
  strengthen its message: "a MarkedBlock-defaulting run_export breaks the palette
  invariant"), `file_browser_commit.rs` redirect/export tests, `render_overlays.rs` title
  row, `e2e.rs` export journey.
- Pattern sites: `row2_onto_a_foreign_format…`'s `Export { ext }, ..` → `Export { ext, .. }`;
  `redirect_clears_the_pending_quit_drain_state`'s expected purpose gains
  `scope: WholeDocument, origin: e.active().id`.
- Every test constructing `Msg::ExportDone` (four in `app.rs`) → add
  `scope: crate::export::ExportScope::WholeDocument`.
- Tests reading `pending_write_block`: the Esc/Cancel assertions (`is_none()`) compile
  unchanged; any test asserting the stored PATH now asserts `.as_ref().unwrap().target`.

### 1d. Verify

`cargo test --workspace` (all new tests green, all migrated tests green) ·
`cargo build` + `cargo test --no-run` warning-free · `cargo clippy --workspace --all-targets`.

**REVIEW instruction (carry to the task's reviewer):** the `PendingWriteBlock` construction
hand-off is deliberately unconstrained by test — after Task 4 the construction runs
immediately post-verify where `active().id == origin` BY CONSTRUCTION, so a re-deriving
construction is extensionally identical (spec §11 carriage map). Check by reading that the
arm stores the BOUND `origin`, not a fresh `editor.active().id`.

---

## Task 2 — the injectable probe seam + the pandoc gate at the redirect (D4-ii)

**Files:** `wordcartel/src/file_browser_commit.rs`.

### 2a. Failing tests first (T4, both fixtures)

```rust
    // T4 flow-survival. Assertions and what each rules out (spec §11): exact status rules
    // out a gate-less implementation (final status would be the redirect Warning — and no
    // frame paints between same-dispatch status writes, so final IS the only observable);
    // mode/field rule out refuse-but-reopen-as-Export. Deliberately NOT claimed: that the
    // reason was never transiently set (terminal reads cannot distinguish set-then-
    // overwritten; §5.2's gate-first order is a REVIEW check, below).
    #[test]
    fn probe_refusal_keeps_the_original_writeblock_picker_alive() {
        let d = tmp("a22-t4-wb");
        let mut e = crate::editor::Editor::new_from_text("body\n", None, (80, 24));
        let ex = crate::jobs::InlineExecutor::default();
        let clk = crate::test_support::TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);
        let origin = e.active().id;
        e.open_destination_picker(&fs, &tx,
            crate::file_browser::DestinationPurpose::WriteBlock { origin },
            d.clone(), "report.html".into());
        crate::file_browser_commit::commit_destination_with_probe(
            &mut e, &fs, &ex, &clk, &tx, || false);
        assert_eq!(e.status_text(), "pandoc not found — install it to export");
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Error);
        match &e.file_browser.as_ref().expect("original picker survives the refusal").mode {
            crate::file_browser::BrowseMode::Destination { purpose, field, .. } => {
                assert!(matches!(purpose,
                    crate::file_browser::DestinationPurpose::WriteBlock { .. }),
                    "a refuse-but-still-redirect implementation reopens as Export");
                assert_eq!(field, "report.html", "the writer's typed name is intact");
            }
            other => panic!("expected destination mode, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    // T4 gate-precedes-drain-abort. ALL THREE drain fields seeded non-default —
    // quit_drain_advance is a bool defaulting false, so unseeded it reads the same on a
    // correct and a wrong implementation (spec-gate round 4). On a gate-after-drain-abort
    // implementation the three read None/None/false and each equality fails.
    #[test]
    fn probe_gate_runs_before_the_saveas_drain_abort() {
        let d = tmp("a22-t4-drain");
        let mut e = crate::editor::Editor::new_from_text("unsaved\n", None, (80, 24));
        let ex = crate::jobs::InlineExecutor::default();
        let clk = crate::test_support::TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);
        e.open_destination_picker(&fs, &tx,
            crate::file_browser::DestinationPurpose::SaveAs, d.clone(), "report.docx".into());
        e.pending_save_as = Some(crate::editor::PostSaveAction::Quit);
        e.quit_drain = Some(crate::editor::QuitDrain {
            queue: std::collections::VecDeque::new(),
            mode: crate::editor::QuitMode::SaveAll });
        e.quit_drain_advance = true;
        crate::file_browser_commit::commit_destination_with_probe(
            &mut e, &fs, &ex, &clk, &tx, || false);
        assert_eq!(e.status_text(), "pandoc not found — install it to export");
        assert!(e.quit_drain.is_some(), "gate must precede the drain-abort");
        assert!(e.pending_save_as.is_some(), "the armed post-save action survives");
        assert!(e.quit_drain_advance, "seeded true; a wrong clear reads false");
        assert!(e.file_browser.as_ref().is_some_and(|fb| matches!(&fb.mode,
            crate::file_browser::BrowseMode::Destination {
                purpose: crate::file_browser::DestinationPurpose::SaveAs, .. })),
            "the SaveAs picker survives; no Export picker opened");
        let _ = std::fs::remove_dir_all(&d);
    }
```

### 2b. Implementation

Rename the body to `commit_destination_with_probe` with the probe as its last parameter; the
`#[allow(clippy::too_many_lines)]` (and its comment) moves with the body. New thin wrapper:

```rust
/// Execute a destination-mode Enter. THE single place a picker commit becomes a write.
/// Production probe = `probe_pandoc` (OnceLock-cached); the seam exists because the merge
/// gate runs on machines WITHOUT pandoc — tests inject (the run_export_with_probe pattern).
pub(crate) fn commit_destination(
    editor: &mut crate::editor::Editor,
    fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
    executor: &dyn crate::jobs::Executor,
    clock: &dyn wordcartel_core::history::Clock,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>,
) {
    commit_destination_with_probe(editor, fs, executor, clock, msg_tx,
        crate::export::probe_pandoc)
}
```

(`probe_pandoc` is `pub fn() -> bool` — coerces to `impl Fn() -> bool`.) The
`file_browser_intercept.rs` Enter arm keeps calling `commit_destination` — unchanged.

`redirect_to_export` gains `pandoc_available: &dyn Fn() -> bool` (last param; the existing
`#[allow(clippy::too_many_arguments)]` covers it) and its body gains, as the FIRST statement:

```rust
    // A22 D4-ii: no point choosing an Export destination for an export that cannot run —
    // and the gate must run before ANY flow-abandoning side effect (status, drain-abort,
    // picker replacement), so a refusal leaves the writer exactly where they were.
    if !pandoc_available() {
        editor.set_status_full(crate::status::StatusKind::Error,
            "pandoc not found — install it to export".to_owned(),
            crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
        return;
    }
```

Both call sites in `commit_destination_with_probe` pass `&pandoc_available`.

### 2c. Migrations (tests that reach `redirect_to_export` must inject `|| true`)

The Enter intercept cannot inject, so these switch their commit step from
`press_enter(..)` / `commit_destination(..)` to
`commit_destination_with_probe(&mut e, &fs, &ex, &clk, &tx, || true)`, keeping every other
line (Enter-through-intercept coverage is retained by the non-redirect commit tests, e.g.
`export_commits_end_to_end_from_enter_through`):

1. the `row2_enter_onto` helper (covers `row2_onto_a_foreign_format_pandoc_writes_offers_
   export_instead`, `row2_onto_a_plain_text_foreign_format_is_still_refused`, and the docx
   half of `a_refused_row2_creates_no_file_at_all` — one change point);
2. `redirect_clears_the_pending_quit_drain_state`;
3. Task 1's two derivation tests (`writeblock_typed_redirect_…`, `writeblock_redirect_
   carries_…`).

Sweep for stragglers: `grep -n "ExportInstead\|Redirect {" wordcartel/src/file_browser_commit.rs`
— any TEST whose flow reaches either redirect arm and still calls the production wrapper is a
missed migration (it would fail only on a pandoc-less machine — exactly the environment
dependence the seam exists to kill).

### 2d. Verify + REVIEW instruction

Gates as in Task 1. **REVIEW:** §5.2's gate-first ORDER within `redirect_to_export`
(gate → reason-status → drain-abort → derive → reopen) is prescribed code order,
deliberately not fully test-constrained (a transient reason-set is unobservable) — verify by
reading the diff that the gate is the first statement and nothing precedes it.

---

## Task 3 — chrome: redirect wording, footer, title (D3 surfaces 1–3)

**Files:** `wordcartel/src/{file_browser_commit.rs, file_browser.rs, render_overlays.rs}`.

### 3a. Failing tests first

Extend Task 1's `writeblock_typed_redirect_derives_…` (this completes T2):

```rust
        assert_eq!(e.status_text(),
            "html is an export format — opening Export for the marked block",
            "surface 1: a scope-blind redirect keeps 'opening Export instead'");
```

and add to `redirect_clears_the_pending_quit_drain_state` (the SaveAs contrast — the
WholeDocument wording must stay byte-identical):

```rust
        assert_eq!(e.status_text(), "html is an export format — opening Export instead",
            "SaveAs redirect wording is unchanged byte-for-byte");
```

T3 (Row-2 wording): add to `row2_onto_a_foreign_format_pandoc_writes_offers_export_instead`
a WriteBlock twin — same `row2_enter_onto`-style fixture but the picker opened with
`DestinationPurpose::WriteBlock { origin }` and a marked block set; assert the reopened
purpose is `Export { ext: "docx", scope: MarkedBlock, origin }` and:

```rust
        assert!(e.status_text().ends_with("is a docx file — opening Export for the marked block"),
            "surface 1 Row-2: scope-blind wording says 'opening Export instead': {:?}",
            e.status_text());
```

T9 (rendered): extend `each_picker_mode_is_titled_for_what_it_actually_does`'s case table —
the two Export rows replace the current one (`origin` from a throwaway
`Editor::new_from_text`-constructed id is fine; the painter never reads it):

```rust
            (BrowseMode::Destination { purpose: DestinationPurpose::Export { ext: "pdf".into(),
                scope: crate::export::ExportScope::WholeDocument, origin },
                field: String::new(), field_cursor: 0 }, "Export .pdf to:", "(marked block)"),
            (BrowseMode::Destination { purpose: DestinationPurpose::Export { ext: "pdf".into(),
                scope: crate::export::ExportScope::MarkedBlock, origin },
                field: String::new(), field_cursor: 0 }, "Export .pdf (marked block) to:", "Open:"),
```

(The WholeDocument row's `forbidden` string `"(marked block)"` rules out an implementation
that stamps the marker on every export title; the MarkedBlock row rules out one that never
stamps it.) Plus a footer paint test in the same module, using the module's harness
(`empty_destination_fb` / `paint_file_browser` / `row_text`):

```rust
    // T9 footer — the pre-commit surface reaches the SCREEN (C5: render, don't assert the
    // struct). A scope-blind footer_target paints the plain redirect note here.
    #[test]
    fn writeblock_footer_redirect_note_names_the_block_scope_on_screen() {
        let dir = std::env::temp_dir().join(format!("wc-a22-footer-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        let origin = e.active().id;
        let mut fb = empty_destination_fb(dir.clone(), "notes.html");
        fb.mode = BrowseMode::Destination {
            purpose: DestinationPurpose::WriteBlock { origin },
            field: "notes.html".into(), field_cursor: "notes.html".len() };
        e.file_browser = Some(fb);
        crate::derive::rebuild(&mut e);
        let cs = ChromeStyles::build(&e.theme, e.depth, e.canvas);
        let mut term = Terminal::new(TestBackend::new(80, 24)).expect("test terminal");
        term.draw(|f| paint_file_browser(f, &mut e, &cs)).expect("draw");
        let all: String = (0..24).map(|y| row_text(&term, y)).collect::<Vec<_>>().join("\n");
        assert!(all.contains("(exports the marked block)"),
            "the painted footer must carry the scope note: {all}");
        let _ = std::fs::remove_dir_all(&dir);
    }
```

(If `empty_destination_fb`'s signature differs from `(dir, field)`, follow its definition at
the top of the test mod — the FIELD must end up `"notes.html"` so `footer_target`'s Redirect
arm fires.)

### 3b. Implementation

**Redirect statuses** (`commit_destination_with_probe`, both arms — the reason is built at
the call sites; spec §6.1, WholeDocument strings byte-identical):

```rust
                    HighlightVerdict::ExportInstead(ext) => {
                        let reason = if matches!(purpose,
                            crate::file_browser::DestinationPurpose::WriteBlock { .. })
                        {
                            format!("{} is a {ext} file \u{2014} opening Export for the \
                                     marked block", raw.display())
                        } else {
                            format!("{} is a {ext} file \u{2014} opening Export instead",
                                raw.display())
                        };
                        redirect_to_export(editor, fs, msg_tx, &purpose, &raw, ext, &reason,
                            dir, &pandoc_available);
                        return;
                    }
```

and the typed arm identically:
`"{ext} is an export format — opening Export for the marked block"` /
`"{ext} is an export format — opening Export instead"`.

**Footer** (`file_browser.rs::footer_target`, the `Redirect` arm):

```rust
            crate::file_browser_commit::ExtVerdict::Redirect { path, ext } => {
                // A22 D3 surface 2: the ONLY pre-commit surface — a Write-Block flow names
                // the scope the offered Export will use.
                let block = matches!(purpose,
                    crate::file_browser::DestinationPurpose::WriteBlock { .. });
                return Some(if block {
                    format!("\u{2192} {} \u{2014} {ext} is an export format (exports the \
                             marked block)", path.display())
                } else {
                    format!("\u{2192} {} \u{2014} {ext} is an export format", path.display())
                });
            }
```

**Title** (`render_overlays.rs`):

```rust
            DestinationPurpose::Export { ext, scope, .. } => match scope {
                crate::export::ExportScope::MarkedBlock =>
                    format!(" Export .{ext} (marked block) to: {dir} "),
                crate::export::ExportScope::WholeDocument =>
                    format!(" Export .{ext} to: {dir} "),
            },
```

### 3c. Verify

Gates as in Task 1. The SaveAs-wording and WholeDocument-title assertions are the
byte-preservation guards (spec §7.4) — if any existing string test fails, the implementation
changed a WholeDocument string; fix the implementation, never the old test.

---

## Task 4 — Write-Block origin verify, both moments (D4-iv)

**Files:** `wordcartel/src/{file_browser_commit.rs, prompts.rs}`.

### 4a. Failing tests first (T8, three fixtures — in `file_browser_commit.rs` tests except
the confirm twin, which lives in `prompts.rs`)

```rust
    // T8 / B unmarked. The EXACT status is the sole discriminator (spec §11, round-5
    // correction): a verify-missing implementation falls through to the mark re-read, ALSO
    // refuses ("no marked block"), and ALSO leaves the target absent — absence here is
    // corroborative only. B-unmarked also pins refusal ORDER (origin verify first).
    #[test]
    fn writeblock_commit_refuses_when_the_buffer_changed() {
        let d = tmp("a22-t8-unmarked");
        let mut e = crate::editor::Editor::new_from_text("body\n", None, (80, 24));
        let ex = crate::jobs::InlineExecutor::default();
        let clk = crate::test_support::TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);
        let origin = e.active().id;
        e.active_mut().marked_block =
            Some(crate::editor::MarkedBlock { start: 0, end: 4, hidden: false });
        e.open_destination_picker(&fs, &tx,
            crate::file_browser::DestinationPurpose::WriteBlock { origin },
            d.clone(), "excerpt.md".into());
        crate::workspace::new_empty_buffer(&mut e); // B: no mark
        assert_ne!(e.active().id, origin);
        crate::file_browser_commit::commit_destination_with_probe(
            &mut e, &fs, &ex, &clk, &tx, || true);
        assert_eq!(e.status_text(), "buffer changed — write block cancelled",
            "verify-missing OR mark-first implementations read 'no marked block' here");
        assert!(!d.join("excerpt.md").exists(), "corroborative (see comment above)");
        let _ = std::fs::remove_dir_all(&d);
    }

    // T8 / B MARKED — the write-suppression discriminator: a verify-missing implementation
    // passes the mark re-read and SYNCHRONOUSLY writes B's block bytes, so the absence
    // assertion is sound and load-bearing here (perform_block_write is main-thread).
    #[test]
    fn writeblock_commit_never_writes_the_wrong_buffers_block() {
        let d = tmp("a22-t8-marked");
        let mut e = crate::editor::Editor::new_from_text("A-body\n", None, (80, 24));
        let ex = crate::jobs::InlineExecutor::default();
        let clk = crate::test_support::TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);
        let origin = e.active().id;
        e.active_mut().marked_block =
            Some(crate::editor::MarkedBlock { start: 0, end: 2, hidden: false });
        e.open_destination_picker(&fs, &tx,
            crate::file_browser::DestinationPurpose::WriteBlock { origin },
            d.clone(), "excerpt.md".into());
        crate::workspace::new_empty_buffer(&mut e);
        // B is the fresh "\n" buffer — mark its single byte directly.
        e.active_mut().marked_block =
            Some(crate::editor::MarkedBlock { start: 0, end: 1, hidden: false });
        crate::file_browser_commit::commit_destination_with_probe(
            &mut e, &fs, &ex, &clk, &tx, || true);
        assert_eq!(e.status_text(), "buffer changed — write block cancelled");
        assert!(!d.join("excerpt.md").exists(),
            "a verify-missing implementation has ALREADY written B's block by now");
        let _ = std::fs::remove_dir_all(&d);
    }
```

In `prompts.rs` tests:

```rust
    // T8 confirm twin — discriminates a CONFIRM-SITE re-derivation (`origin :=
    // active().id`): that implementation passes its own check, hits the mark re-read on
    // unmarked B, and reads "no marked block" — the equality fails. (Construction-site
    // re-derivation is extensionally identical post-verify — spec carriage map; REVIEW.)
    #[test]
    fn overwrite_write_block_refuses_when_the_buffer_changed() {
        let mut e = crate::editor::Editor::new_from_text("body\n", None, (80, 24));
        let origin = e.active().id;
        e.active_mut().marked_block =
            Some(crate::editor::MarkedBlock { start: 0, end: 4, hidden: false });
        let d = crate::test_support::scratch_dir("a22-t8-confirm");
        let target = d.join("excerpt.md");
        std::fs::write(&target, b"OLD").expect("existing target");
        e.pending_write_block = Some(crate::editor::PendingWriteBlock {
            target: target.clone(), origin });
        crate::workspace::new_empty_buffer(&mut e); // B: no mark
        assert_ne!(e.active().id, origin);
        let ex = crate::jobs::InlineExecutor::default();
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::prompts::resolve_prompt(crate::prompt::PromptAction::OverwriteWriteBlock,
            &mut e, &ex, &crate::test_support::TestClock(0), &tx,
            &crate::test_support::test_fs());
        assert_eq!(e.status_text(), "buffer changed — write block cancelled");
        assert_eq!(std::fs::read(&target).expect("still there"), b"OLD",
            "sound absence-of-effect: the write path is synchronous");
    }
```

### 4b. Implementation

`commit_destination_with_probe`, `WriteBlock { origin }` arm — insert BEFORE the mark
re-read:

```rust
                    // A22 D4-iv: the flow's buffer, captured at ^KW, must still be active —
                    // the plugin pump can switch buffers under the open picker. Verify
                    // BEFORE the mark re-read so the status names the real reason.
                    if editor.active().id != origin {
                        editor.set_status_full(crate::status::StatusKind::Warning,
                            "buffer changed — write block cancelled".to_owned(),
                            crate::status::StatusLifetime::Sticky,
                            crate::status::StatusSource::Host, None);
                        return;
                    }
```

`prompts.rs`, `OverwriteWriteBlock` arm:

```rust
        PromptAction::OverwriteWriteBlock => {
            if let Some(t) = editor.pending_write_block.take() {
                if editor.active().id != t.origin {
                    editor.set_status_full(crate::status::StatusKind::Warning,
                        "buffer changed — write block cancelled".to_owned(),
                        crate::status::StatusLifetime::Sticky,
                        crate::status::StatusSource::Host, None);
                } else if let Some(b) = editor.active().marked_block {
                    perform_block_write(editor, &t.target, b.start, b.end, fs);
                } else {
                    editor.set_status(crate::status::StatusKind::Info, "no marked block");
                }
            }
        }
```

The pre-existing `"no marked block"` refusals keep their `Info` kind and wording verbatim
(spec §6.2 — new strings only).

### 4c. Verify + REVIEW instruction

Gates as in Task 1. **REVIEW:** confirm the commit arm stores the BOUND `origin` into
`PendingWriteBlock` (not a fresh `active().id`) — unconstrained by test, sound only because
it sits post-verify; a later refactor moving the construction above the verify would break
the soundness argument silently.

---

## Task 5 — completion status through the message layer (D3 surface 4)

**Files:** `wordcartel/src/{jobs_apply.rs, app.rs, prompts.rs}`.

### 5a. Failing tests first

T10 (`jobs_apply.rs` tests — beside the existing `apply_export_done_*` tests, whose calls
gain the new argument in this task):

```rust
    // T10 — the endpoint wording, both scopes, EXACT match. The pre-existing app.rs
    // assertion is only `contains("exported")` — it passes under both wordings AND under a
    // scope-dropped regression; these equalities carry the discrimination (spec §11 T10).
    #[test]
    fn export_done_status_names_the_scope() {
        let d = crate::test_support::scratch_dir("a22-t10");
        for (scope, want) in [
            (crate::export::ExportScope::WholeDocument, "exported "),
            (crate::export::ExportScope::MarkedBlock, "exported block to "),
        ] {
            let mut e = crate::editor::Editor::new_from_text("x\n", None, (80, 24));
            let target = d.join("out.html");
            let _ = std::fs::remove_file(&target);
            apply_export_done(&mut e, target.clone(),
                Ok(crate::export::ExportResult::Bytes(b"<p>x</p>".to_vec())), true, scope,
                &crate::fsx::RealFs);
            let expect = format!("{want}{}", target.display());
            assert_eq!(e.status_text(), expect,
                "a reducer/endpoint substituting the other scope produces the other wording");
        }
    }
```

T13b (`app.rs` tests — follow the existing ExportDone reduce-test pattern including
`cua_keymap()`/`TestClock`; both scopes):

```rust
    // T13b — scope carriage through reduce_dispatch. A reduce arm that forwards a default
    // instead of the bound scope produces the WholeDocument wording for the MarkedBlock
    // message (or vice versa) and fails the exact match.
    #[test]
    fn reduce_forwards_export_done_scope_to_the_status() {
        let tmp_dir = crate::test_support::scratch_dir("a22-t13b");
        for (scope, want) in [
            (crate::export::ExportScope::MarkedBlock, "exported block to "),
            (crate::export::ExportScope::WholeDocument, "exported "),
        ] {
            let mut e = Editor::new_from_text("# Hello\n", None, (80, 24));
            let buffer_id = e.active().id;
            let reg = Registry::builtins();
            let ex = InlineExecutor::default();
            let clk = TestClock(0);
            let (tx, _rx) = std::sync::mpsc::channel();
            let output_path = tmp_dir.join("notes.html");
            let _ = std::fs::remove_file(&output_path);
            let msg = Msg::ExportDone { buffer_id, target: output_path.clone(),
                result: Ok(ExportResult::Bytes(b"<h1>x</h1>".to_vec())),
                overwrite_confirmed: true, scope };
            crate::app::reduce(msg, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx,
                &crate::test_support::test_fs());
            assert_eq!(e.status_text(), format!("{want}{}", output_path.display()));
        }
    }
```

T13c (`prompts.rs` tests — the second delivery site, under an open modal; ctx built the way
`test_support::press_key_fb` builds one):

```rust
    // T13c — the prompts-intercept ExportDone arm is the SECOND delivery site; it could
    // substitute WholeDocument while reduce_dispatch is correct, and only this test fails.
    #[test]
    fn prompt_intercept_forwards_export_done_scope_to_the_status() {
        let d = crate::test_support::scratch_dir("a22-t13c");
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (80, 24));
        e.open_prompt(crate::prompt::Prompt::save_overwrite(std::path::Path::new("/x")));
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let ex = crate::jobs::InlineExecutor::default();
        let clk = crate::test_support::TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        let fs = crate::test_support::test_fs();
        let ctx = crate::overlays::DispatchCtx {
            reg: &reg, keymap: &km, ex: &ex, clock: &clk, msg_tx: &tx, fs: &fs };
        let target = d.join("out.html");
        let _ = std::fs::remove_file(&target);
        let buffer_id = e.active().id;
        let msg = crate::app::Msg::ExportDone { buffer_id, target: target.clone(),
            result: Ok(crate::export::ExportResult::Bytes(b"<p>x</p>".to_vec())),
            overwrite_confirmed: true, scope: crate::export::ExportScope::MarkedBlock };
        let _ = crate::prompts::intercept(msg, &mut e, &ctx);
        assert_eq!(e.status_text(), format!("exported block to {}", target.display()));
    }
```

### 5b. Implementation

`jobs_apply.rs` — signature gains `scope: crate::export::ExportScope` before `fs`; hoist the
duplicated wording (both `Ok` arms call it):

```rust
/// A22 D3 surface 4: the completion line names what was exported.
fn export_done_status(scope: crate::export::ExportScope, target: &std::path::Path) -> String {
    match scope {
        crate::export::ExportScope::WholeDocument => format!("exported {}", target.display()),
        crate::export::ExportScope::MarkedBlock =>
            format!("exported block to {}", target.display()),
    }
}
```

Both success arms: `let status = export_done_status(scope, &target);` (the `WholeDocument`
output is byte-for-byte today's string). TOCTOU guard and error arms untouched.

Both dispatch sites bind and forward: in `app.rs`'s reduce arm and `prompts.rs`'s intercept
arm, `Msg::ExportDone { target, result, overwrite_confirmed, scope, .. }` → pass `scope` in
the existing call, position before the fs argument. Existing `apply_export_done_*` tests in
`jobs_apply.rs` gain `crate::export::ExportScope::WholeDocument` at the new position
(compiler-forced).

### 5c. Verify

Gates as in Task 1. The existing `app.rs` `contains("exported")` test stays untouched — it
guards the write happening; T10/T13 guard the wording.

---

## Task 6 — Export commit-arm carriage (T12; test-only)

**Files:** `wordcartel/src/file_browser_commit.rs` (tests only — no production code; these
are the round-5 fixtures that pin the `purpose → commit arm → do_export/PendingExport`
hand-offs every endpoint test leaves free).

```rust
    /// T12 shared fixture: a REAL redirect from a ^KW flow on buffer A — the value under
    /// test is the carried one, never hand-seeded. Returns (editor, fs, ex, clk, tx, rx,
    /// origin, dir); the Export picker is open with field "report.html".
    fn redirected_block_export(name: &str)
        -> (crate::editor::Editor, std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
            crate::jobs::InlineExecutor, crate::test_support::TestClock,
            std::sync::mpsc::Sender<crate::app::Msg>,
            std::sync::mpsc::Receiver<crate::app::Msg>,
            crate::editor::BufferId, std::path::PathBuf)
    {
        let d = tmp(name);
        let mut e = crate::editor::Editor::new_from_text("AAA\nBBB\n", None, (80, 24));
        let ex = crate::jobs::InlineExecutor::default();
        let clk = crate::test_support::TestClock(0);
        let (tx, rx) = std::sync::mpsc::channel();
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);
        let origin = e.active().id;
        e.active_mut().marked_block =
            Some(crate::editor::MarkedBlock { start: 0, end: 4, hidden: false });
        e.open_destination_picker(&fs, &tx,
            crate::file_browser::DestinationPurpose::WriteBlock { origin },
            d.clone(), "report.html".into());
        crate::file_browser_commit::commit_destination_with_probe(
            &mut e, &fs, &ex, &clk, &tx, || true); // the redirect
        assert!(matches!(&e.file_browser.as_ref().expect("export picker").mode,
            crate::file_browser::BrowseMode::Destination { purpose:
                crate::file_browser::DestinationPurpose::Export { .. }, .. }));
        (e, fs, ex, clk, tx, rx, origin, d)
    }

    // T12a — scope hand-off, no-overwrite path. A commit arm hardcoding WholeDocument
    // into the do_export call DISPATCHES here (no refusal status) and fails the equality.
    #[test]
    fn export_commit_arm_forwards_the_carried_block_scope() {
        let (mut e, fs, ex, clk, tx, rx, _origin, d) = redirected_block_export("a22-t12a");
        crate::blocks_marked::block_clear(&mut e); // flip the mark-present conjunct
        crate::file_browser_commit::commit_destination_with_probe(
            &mut e, &fs, &ex, &clk, &tx, || true); // Enter on "report.html", non-existing
        assert_eq!(e.status_text(), "no marked block — export cancelled");
        drop(tx);
        assert!(rx.recv().is_err(), "no worker may have been dispatched");
        let _ = std::fs::remove_dir_all(&d);
    }

    // T12b — origin hand-off, no-overwrite path. A re-deriving commit arm (origin :=
    // active().id) LAUNDERS the switch: verification passes against B, the mark re-read
    // refuses "no marked block — export cancelled" — a DIFFERENT exact string.
    #[test]
    fn export_commit_arm_forwards_the_carried_origin() {
        let (mut e, fs, ex, clk, tx, rx, origin, d) = redirected_block_export("a22-t12b");
        crate::workspace::new_empty_buffer(&mut e); // switch to unmarked B
        assert_ne!(e.active().id, origin);
        crate::file_browser_commit::commit_destination_with_probe(
            &mut e, &fs, &ex, &clk, &tx, || true);
        assert_eq!(e.status_text(), "buffer changed — export cancelled",
            "re-derivation reads 'no marked block — export cancelled' instead");
        drop(tx);
        assert!(rx.recv().is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    // T12c — PendingExport construction, overwrite path. The switch makes carried and
    // re-derived origins DIFFER at construction time (the Export arm has no commit-time
    // verify). WHOLE-VALUE equality per spec §4.3: a field-wise assertion would silently
    // stop constraining any field a future change adds. The expected `target` is computed
    // by the SAME production resolver the commit path uses, so a symlinked scratch dir
    // cannot fail the path axis for a non-carriage reason.
    #[test]
    fn export_commit_arm_builds_pending_export_from_the_carried_purpose() {
        let (mut e, fs, ex, clk, tx, _rx, origin, d) = redirected_block_export("a22-t12c");
        std::fs::write(d.join("report.html"), b"OLD").expect("existing target");
        crate::workspace::new_empty_buffer(&mut e);
        crate::file_browser_commit::commit_destination_with_probe(
            &mut e, &fs, &ex, &clk, &tx, || true);
        let resolved = crate::fsx::resolve_write_destination(&*fs, &d.join("report.html"))
            .expect("the scratch target resolves");
        assert_eq!(e.pending_export, Some(crate::export::PendingExport {
            ext: "html".into(), target: resolved,
            scope: crate::export::ExportScope::MarkedBlock, origin }),
            "a WholeDocument-substituting construction fails on scope; a re-deriving one \
             fails on origin (it stores B); a future field lands in this literal by \
             compiler force and is constrained from day one");
        // The confirm leg (complements T6's seeded side): dispatch refuses on the switch.
        let (tx2, _rx2) = std::sync::mpsc::channel();
        crate::prompts::resolve_prompt(crate::prompt::PromptAction::OverwriteExport, &mut e,
            &ex, &clk, &tx2, &crate::test_support::test_fs());
        assert_eq!(e.status_text(), "buffer changed — export cancelled");
        let _ = std::fs::remove_dir_all(&d);
    }
```

(T12a/b's second `commit_destination_with_probe` call commits the REOPENED picker — its field
is pre-seeded `"report.html"` by the redirect, dir `d`, target non-existing, so the Export
arm's not-exists branch calls `do_export` directly. If `classify_destination_enter` descends
instead because the un-pumped listing pinned a directory row: these fixtures never pump the
listing, `entries` is empty, `highlighted` is `None` → Row 4 commits. The redirect leg of the
shared fixture relies on the same fact, as Task 1's derivation tests already do.)

**Verify:** gates as in Task 1. No production diff — `git diff --stat` for this task touches
only `file_browser_commit.rs`'s test module.

---

## Task 7 — `cancel_destination` tidy (D4-iii) + effort close-out

**Files:** `wordcartel/src/file_browser.rs`.

### 7a. Failing test first (in `file_browser.rs` tests)

```rust
    // D4-iii — Esc on a destination picker sweeps the SAME pending set everywhere.
    // Neither field is ever Some while the picker is open today (both are set post-picker),
    // so this is hygiene, not a live-bug fix — but the test is still discriminating: an
    // implementation without the added clear leaves pending_export Some.
    #[test]
    fn cancel_destination_sweeps_pending_export_like_the_rest() {
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (80, 24));
        let origin = e.active().id;
        e.pending_export = Some(crate::export::PendingExport { ext: "html".into(),
            target: std::path::PathBuf::from("/t/out.html"),
            scope: crate::export::ExportScope::WholeDocument, origin });
        e.pending_write_block = Some(crate::editor::PendingWriteBlock {
            target: std::path::PathBuf::from("/t/x.md"), origin });
        cancel_destination(&mut e);
        assert!(e.pending_export.is_none(), "the added clear is missing");
        assert!(e.pending_write_block.is_none(), "the pre-existing clear regressed");
    }
```

### 7b. Implementation

In `cancel_destination`, beside the `pending_write_block` clear:

```rust
    // A22 D4-iii: not load-bearing today (only ever set after the picker closes, like
    // pending_write_block below) — cleared for the same symmetry: every place that
    // abandons a destination flow sweeps the same pending set.
    editor.pending_export = None;
```

### 7c. Close-out (the controller's checklist, recorded here so it is not lost)

- Full gates one last time: `cargo test --workspace`, `cargo build`, `cargo test --no-run`,
  `cargo clippy --workspace --all-targets`.
- PTY smoke suite: `scripts/smoke/run.sh` — quote its one-line summary VERBATIM in the
  pre-merge report (mandatory-run, advisory-pass; no smoke check exercises export, so no
  change is expected).
- Backlog bookkeeping happens AT MERGE, not in this plan's tasks: `backlog.toml` A22 →
  shipped, `scripts/backlog bless`, move the A22 prose section to `docs/backlog-archive.md`
  and repoint its `doc =` (the marker-bijection gate).
- The two final gates per project law (Fable whole-branch + Codex pre-merge GO/NO-GO) run
  before any merge; the whole-branch probe should exercise the ORDERINGS of the new
  mechanism's concurrent inputs (the E10 lesson) — specifically: mark cleared between commit
  and confirm, buffer switched between commit and confirm, and both orderings of
  refusal-then-retry.
