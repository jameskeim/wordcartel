# A22 — Block-scoped export: the Write-Block → Export redirect honours the mark

**Status:** design spec for effort branch `effort-a22-block-scoped-export` (base `8f5ec04`).
**Decisions:** `scratchpad/a22/decisions.md` (D1–D4) is binding; this spec implements it and does
not re-open it. Grounding evidence: `scratchpad/a22/fable-grounding.md`, verified at `8f5ec04`.
**Backlog:** item `A22` (kind `bug`), prose `docs/ux-backlog.md` `<!-- item: A22 -->`; sequenced
first in `docs/design/backlog-sequence.md`.

All code references anchor on symbol names; line numbers, where given, are as observed at
`8f5ec04` and are advisory only.

---

## 1. The defect and its mechanism

In Write-Block mode (`^KW` → `blocks_marked::block_write` → destination picker with
`DestinationPurpose::WriteBlock`), a destination whose extension pandoc can produce redirects
into the Export flow, and Export then exports the **whole document**, not the marked block.

Mechanism, verified: `file_browser_commit::commit_destination`'s `CommitOutcome::Commit` arm
reaches `redirect_to_export` from exactly two places — the Row-2
`HighlightVerdict::ExportInstead(ext)` arm (writer highlighted an existing
`.docx`/`.pdf`/`.html`/`.tex`; empty field) and the typed-path `ExtVerdict::Redirect { path, ext }`
arm (`OUTPUT_EXTS = ["docx", "pdf", "html", "tex"]`). `redirect_to_export` reads `purpose` only
to decide the SaveAs drain-abort; it then reopens the picker as
`DestinationPurpose::Export { ext }`, which carries **no scope**. The eventual commit dispatches
`export::do_export`, whose input is unconditionally
`editor.active().document.buffer.to_string()`. The writer's block intent is dropped at the
purpose hand-off; these two call sites are the complete scope-loss surface (grounding §1.7 —
no other block-intent → whole-document channel exists).

Compare the flow that keeps its promise: the `WriteBlock` commit arm re-reads
`editor.active().marked_block` at commit time and `prompts::perform_block_write` slices
`buffer.slice(start..end)`; `PromptAction::OverwriteWriteBlock` re-reads the mark again after the
confirm. The fix extends exactly this discipline across the redirect.

---

## 2. Decision summary (binding, from D1–D4)

| # | Decision |
|---|---|
| D1 | Export honours the marked block when entered from Write-Block: `Export { ext, scope }` with `ExportScope { WholeDocument, MarkedBlock }`; scope is a **flag re-read at dispatch, never stored offsets**; disclosure chrome folded in. Palette exports stay whole-document. |
| D2 | Block gone at dispatch → **refuse**, at BOTH dispatch moments (commit arm and post-`OverwriteExport` confirm). Status "no marked block — export cancelled". No whole-document fallback. |
| D3 | All four chrome surfaces name the scope: redirect status, picker footer, picker title, completion status. `Msg::ExportDone` gains a scope field for the fourth. |
| D4 | Folded in: (ii) pandoc probe at the redirect site; (iii) `cancel_destination` `pending_export` tidy; (iv) `BufferId` capture-at-open / verify-at-dispatch, covering Write-Block too. (i) typed-vs-highlighted foreign-extension asymmetry is OUT — filed as A23. |

Engineering calls inherited from grounding (evidence-decided, not forks): `MarkedBlock.hidden`
is ignored (display-only; `perform_block_write` already ignores it); `apply_export_done` needs
no scope *logic* (finalization is target-and-bytes — it gains only the status wording); offsets
are never snapshotted because background merges (`FilterDone`/`TransformDone`/`JobDone`) are
deliberately processed while the picker is open and the `Buffer::apply` funnel remaps
`marked_block` (`map_pos`/`map_pos_before`, collapse-clear).

---

## 3. Design overview

One principle, applied uniformly: **a destination flow captures its context at flow start and
verifies it at every dispatch.**

- *Context* = the originating `BufferId` (new — closes the plugin-pump `next_buffer` hole,
  grounding §1.5.2) plus, for Export, the `ExportScope`.
- *Flow start* = `block_write` (^KW), `run_export_with_probe` (palette), or
  `redirect_to_export` (a fresh Export picker seeded from a dying save flow).
- *Dispatch* = the moment content is read and a write/subprocess is launched: the
  `commit_destination` purpose arms, and the post-confirm `PromptAction::OverwriteExport` /
  `OverwriteWriteBlock` arms.
- *Verify* = `editor.active().id == origin`, then (for `MarkedBlock` scope and for Write-Block)
  `editor.active().marked_block` is present. Either check failing refuses loudly; nothing is
  written.

The scope/origin ride the two async boundaries on existing carriers: boundary 1 (the async
picker listing, `Editor::open_destination_picker` → `file_browser::start_listing`) on
`BrowseMode::Destination`'s `purpose` field, which the listing-epoch machinery never touches;
boundary 2 (the overwrite confirm) on `PendingExport` (Export) and on `pending_write_block`,
which becomes a two-field struct (Write-Block).

---

## 4. Type changes (exact shapes)

### 4.1 `ExportScope` — new, in `wordcartel/src/export.rs`

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
```

`export.rs` owns it (the export domain reads it); `file_browser.rs` references it as
`crate::export::ExportScope` (same-crate cross-module reference, mirroring `export.rs`'s
existing references to `crate::file_browser::DestinationPurpose`).

### 4.2 `DestinationPurpose` — `wordcartel/src/file_browser.rs`

```rust
pub enum DestinationPurpose {
    SaveAs,
    WriteBlock { origin: crate::editor::BufferId },
    Export {
        ext: String,
        scope: crate::export::ExportScope,
        origin: crate::editor::BufferId,
    },
}
```

`SaveAs` is untouched (D4 scopes verification to Write-Block and Export; §12 records the SaveAs
residual). `WriteBlock` becomes a struct variant — the compiler forces every unit-pattern site
(census §8). `BufferId` is `Copy` (`#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Ord,
PartialOrd)] pub struct BufferId(pub u64)`), so binding it out of a borrowed purpose costs
nothing. The enum's existing derives (`Debug, Clone, PartialEq, Eq`) are unchanged and all new
field types satisfy them.

### 4.3 `PendingExport` — `wordcartel/src/export.rs`

```rust
#[derive(Debug, Clone)]
pub struct PendingExport {
    pub ext: String,
    pub target: PathBuf,
    pub scope: ExportScope,
    pub origin: crate::editor::BufferId,
}
```

Set at one production site (`commit_destination`'s Export arm), consumed at one
(`PromptAction::OverwriteExport`). The existing whole-value clears (`Esc` in
`prompts::intercept`, `PromptAction::Cancel`, and — after §5.6 — `cancel_destination`) survive
unchanged; no half-cleared-pair hazard is introduced (the M6 lesson in `prompts.rs`).

### 4.4 `PendingWriteBlock` — new, in `wordcartel/src/editor.rs`, replacing the bare `PathBuf`

```rust
/// State crossing the write-block overwrite confirm: the resolved target plus the
/// originating buffer, verified again when the confirm fires (A22 D4-iv).
#[derive(Debug, Clone)]
pub struct PendingWriteBlock {
    pub target: std::path::PathBuf,
    pub origin: BufferId,
}
```

`Editor::pending_write_block: Option<PathBuf>` becomes `Option<PendingWriteBlock>`. The `= None`
clears (`prompts::intercept` Esc arm, `PromptAction::Cancel`, `file_browser::cancel_destination`)
and the `None` initializer in `Editor` construction compile unchanged; the set site and the take
site are compiler-forced (§5.3, §5.5).

### 4.5 `Msg::ExportDone` — `wordcartel/src/app.rs`

Gains one field, after `overwrite_confirmed`:

```rust
    ExportDone {
        buffer_id: crate::editor::BufferId,
        target: std::path::PathBuf,
        result: Result<crate::export::ExportResult, crate::filter::FilterError>,
        overwrite_confirmed: bool,
        /// What this export read (A22 D3-4): drives the completion status wording only.
        scope: crate::export::ExportScope,
    },
```

The custom `Debug` impl's `Msg::ExportDone { buffer_id, target, .. }` arm survives via `..`.
Both dispatch sites (`app.rs` `reduce_dispatch` and the `prompts::intercept` background-result
arm) currently drop trailing fields with `..`; each now binds `scope` and forwards it to
`apply_export_done` (§5.7). This is the one place scope leaks from the flow layer into the
job-message layer — accepted explicitly in D3.

---

## 5. Flow changes

### 5.1 Flow start — capturing `origin`

- `blocks_marked::block_write` (^KW): constructs
  `DestinationPurpose::WriteBlock { origin: editor.active().id }`. The existing
  "no marked block" pre-check and directory seeding are unchanged.
- `export::run_export_with_probe`: constructs
  `DestinationPurpose::Export { ext: ext.to_owned(), scope: ExportScope::WholeDocument,
  origin: editor.active().id }`. Its two refusals (unsaved buffer, pandoc probe) are unchanged.
- `prompts::open_save_as`, `save.rs`'s SaveAs re-open, and all Select/Recents flows: untouched.

### 5.2 `redirect_to_export` — derivation, probe gate, and what survives

Current shape (private fn in `file_browser_commit.rs`, `#[allow(clippy::too_many_arguments)]`):
`redirect_to_export(editor, fs, msg_tx, purpose, path, ext, reason, fallback_dir)`. It gains one
parameter, the injectable pandoc probe (threaded from `commit_destination_with_probe`, §5.3):

```rust
fn redirect_to_export(
    editor: &mut crate::editor::Editor,
    fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>,
    purpose: &crate::file_browser::DestinationPurpose,
    path: &Path,
    ext: String,
    reason: &str,
    fallback_dir: PathBuf,
    pandoc_available: &dyn Fn() -> bool,
) {
```

Ordered behavior:

1. **Pandoc gate FIRST (D4-ii).** `if !pandoc_available()` → set the refusal status
   (byte-identical to `run_export_with_probe`'s: text `"pandoc not found — install it to
   export"`, `StatusKind::Error`, `StatusLifetime::Sticky`, `StatusSource::Host`) and `return`.
   Nothing else runs: the redirect `reason` is not shown, the SaveAs drain-abort does not fire,
   and — because both redirect call sites `return` before `commit_destination` reaches its
   `editor.file_browser = None` line — the **original picker stays open** with the writer's
   field intact; they can retype a markdown name. The saved-source gate is deliberately NOT
   added: `do_export` feeds pandoc from stdin and never needs `document.path` (D4).
2. Redirect status (the `reason`, now scope-aware — §6.1) — `Warning` + `Sticky`, as today.
3. SaveAs drain-abort — unchanged (`pending_save_as`/`quit_drain`/`quit_drain_advance` cleared
   only for `SaveAs`; the `matches!` becomes no less exact under the new variant shapes).
4. **Scope/origin derivation**, exhaustive over the incoming `purpose`:
   - `WriteBlock { origin }` → `scope = ExportScope::MarkedBlock`, `origin = *origin` — the
     origin captured at ^KW carries through; it is deliberately NOT re-captured here, so a
     buffer switched between ^KW and the redirect is caught at dispatch, not laundered.
   - `SaveAs` → `scope = ExportScope::WholeDocument`, `origin = editor.active().id` — the
     redirect IS this Export flow's start.
   - `Export { ext: _, scope, origin }` → carry both through unchanged. Unreachable today —
     `commit_destination`'s `chosen` match sends `Export { .. } => raw` before either
     policy/highlight classification can fire — but the match stays exhaustive rather than
     `unreachable!()`, per the house preference for compiler-forced totality.
5. Reopen as `DestinationPurpose::Export { ext, scope, origin }` — dir/field seeding unchanged.

### 5.3 `commit_destination` — the probe seam and the Write-Block verification

**Probe seam.** `commit_destination(editor, fs, executor, clock, msg_tx)` becomes a thin
wrapper delegating to a new
`commit_destination_with_probe(editor, fs, executor, clock, msg_tx, pandoc_available:
impl Fn() -> bool)` with `crate::export::probe_pandoc` as the production probe — the exact
pattern of `run_export` / `run_export_with_probe`, and for the same reason: the merge gate runs
on machines without pandoc, so every test that exercises a redirect arm must inject `|| true`
(or `|| false` for the refusal test) rather than depend on the host. The sole production caller
(`file_browser_intercept::intercept`'s `KeyCode::Enter` arm) keeps calling `commit_destination`
and does not change. Existing tests that drive the redirect arms
(`file_browser_commit.rs` tests near the current :1362-1381 and :1538 anchors) migrate to
`commit_destination_with_probe(.., || true)`.

**`WriteBlock { origin }` arm** (after symlink resolution and `editor.file_browser = None`,
exactly where the arm sits today):

1. **Origin verify (new, first):** `if editor.active().id != origin` → status
   `"buffer changed — write block cancelled"` (`Warning`, `Sticky`), `return`.
2. Mark re-read, unchanged in position and meaning: `let Some(b) = editor.active().marked_block
   else { … "no marked block" … return }`. (This existing refusal keeps its current
   `StatusKind::Info` wording verbatim — behavior-preserving; only NEW strings get the
   Warning/Sticky treatment, §6.2.)
3. Exists → `pending_write_block = Some(PendingWriteBlock { target: resolved, origin })` +
   `Prompt::write_block_overwrite(..)` naming the `.target` path; not-exists →
   `perform_block_write(editor, &resolved, b.start, b.end, fs)` as today.

**`Export { ext, scope, origin }` arm:**

- Exists → `pending_export = Some(PendingExport { ext, target: resolved, scope, origin })` +
  `Prompt::export_overwrite(..)` (prompt text unchanged — it names the target).
- Not-exists → `do_export(editor, &ext, &resolved, msg_tx, false, scope, origin,
  Arc::clone(fs))`.

The refusal-ordering cost accepted in D2 is inherited, not introduced: `editor.file_browser =
None` precedes the purpose dispatch, so any dispatch-time refusal leaves the picker closed and
the writer restarts from ^KW.

### 5.4 `do_export` — verification, scoped input, and the refusal seam

`do_export` gains `scope: ExportScope` and `origin: crate::editor::BufferId` (inserted after
`overwrite_confirmed`), taking it to 8 parameters — it carries an item-local
`#[allow(clippy::too_many_arguments)]` with a one-line rationale (the full dispatch context in
one place, mirroring `redirect_to_export`'s existing allow).

The input decision is a **pure, separately testable seam** (the E11 rule demands fixtures where
each conjunct alone decides — §11):

```rust
/// Why a block-scoped dispatch refused (A22 D2 / D4-iv). Maps 1:1 onto a status string.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ExportRefusal {
    /// The active buffer is not the one this flow started in.
    BufferChanged,
    /// Scope is MarkedBlock but the active buffer has no mark (collapsed, undone, cleared).
    NoMarkedBlock,
}

/// Resolve what this export reads, verifying the flow's captured context first.
/// Runs on the main thread at dispatch — the same moment the whole-document path
/// already snapshots `to_string()` — so the slice is coherent with the remapped mark.
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

Grounding notes the seam relies on: `TextBuffer::slice(start..end)` is exactly what
`perform_block_write` calls today; `marked_block` offsets are maintained by the `Buffer::apply`
funnel across any mid-picker merge, and cleared (never left dangling) by collapse and by
undo/redo — so a present mark is always sliceable. `hidden` is deliberately not consulted.

`do_export` becomes:

1. `let stdin = match resolve_export_input(editor, scope, origin) { Ok(s) => s, Err(r) => {
   status per §6.2; return; } }` — on refusal **no thread is spawned and no `ExportDone` is ever
   sent**; nothing downstream awaits one (`ExportDone` is fire-and-forget into the reducer), so
   no flow hangs.
2. `buffer_id = editor.active().id` — unchanged line; post-verification it equals `origin`.
3. Thread spawn, sinks, temp path, `run_pandoc`: unchanged. The `Msg::ExportDone` send adds
   `scope`.

Both existing `do_export` call sites (`commit_destination`'s Export arm; the `OverwriteExport`
prompt arm) now pass `scope`/`origin`; verification therefore covers BOTH dispatch moments with
one rule (D2), including whole-document palette exports (uniform origin verification — the
Export flow's context is captured at `run_export_with_probe` and verified like any other).

### 5.5 The confirm arms — `prompts::resolve_prompt`

- `PromptAction::OverwriteExport`: `if let Some(pe) = editor.pending_export.take() {
  do_export(editor, &pe.ext, &pe.target, msg_tx, true, pe.scope, pe.origin, Arc::clone(fs)) }` —
  verification and block re-read happen inside `do_export`; D2's post-confirm refusal falls out.
- `PromptAction::OverwriteWriteBlock`: takes `PendingWriteBlock { target, origin }`; **origin
  verify first** (`"buffer changed — write block cancelled"`, `Warning`+`Sticky`), then the
  existing mark re-read (`"no marked block"` Info wording unchanged), then
  `perform_block_write(editor, &target, b.start, b.end, fs)`.

### 5.6 `cancel_destination` tidy (D4-iii)

`file_browser::cancel_destination` gains `editor.pending_export = None;` beside the existing
`pending_write_block` clear, with a one-line comment noting neither clear is load-bearing (both
fields are only ever set after the picker closes) and the symmetry is the point — every
"abandon a destination flow" site sweeps the same set.

### 5.7 `apply_export_done` — wording only

Signature gains `scope: crate::export::ExportScope` (before `fs`). The TOCTOU guard, write,
rename, and error arms are unchanged. The two success statuses become scope-keyed:

- `WholeDocument` → `format!("exported {}", target.display())` — **byte-for-byte today's
  string**, `Info`, via `set_status` (both the `Bytes` and `TempReady` arms).
- `MarkedBlock` → `format!("exported block to {}", target.display())` — same kind/lifetime.

Both dispatch sites (`reduce_dispatch`'s arm and `prompts::intercept`'s arm) bind and forward
`scope`.

---

## 6. Chrome — the four surfaces, exact wording

### 6.1 Scope disclosure (D3)

| # | Surface | Symbol | WholeDocument (— must stay byte-identical to today) | MarkedBlock (new) |
|---|---|---|---|---|
| 1 | Redirect status, Row-2 | `commit_destination`, `ExportInstead` arm | `"{path} is a {ext} file — opening Export instead"` | `"{path} is a {ext} file — opening Export for the marked block"` |
| 1 | Redirect status, typed | `commit_destination`, `ExtVerdict::Redirect` arm | `"{ext} is an export format — opening Export instead"` | `"{ext} is an export format — opening Export for the marked block"` |
| 2 | Picker footer (pre-commit — the only surface visible before Enter) | `file_browser::footer_target`, `ExtVerdict::Redirect` arm | `"→ {path} — {ext} is an export format"` | `"→ {path} — {ext} is an export format (exports the marked block)"` |
| 3 | Picker title | `render_overlays.rs`, `Export` title arm | `" Export .{ext} to: {dir} "` | `" Export .{ext} (marked block) to: {dir} "` |
| 4 | Completion status | `jobs_apply::apply_export_done` | `"exported {target}"` | `"exported block to {target}"` |

Surface 1's selector is the *incoming* purpose at the redirect site (`WriteBlock` → the
marked-block wording; `SaveAs` → today's wording, byte-identical). Surface 2's selector is
`fb.mode`'s purpose, which `footer_target` already destructures (`let BrowseMode::Destination {
field, purpose, .. } = &fb.mode`); only its `Redirect` arm changes — the `Descend`/`Nothing`/
`Refused`/exists-note behavior is untouched, and the Row-2/empty-field guard means this arm
still never fires without field text. Surface 3's selector is the Export purpose's own `scope`
(the `WriteBlock` and `SaveAs` title arms are untouched except the compiler-forced
`WriteBlock { .. }` pattern spelling). Surface 4's selector is `Msg::ExportDone.scope`. The
em-dashes and `→` in the existing strings are `\u{2014}` / `\u{2192}` escapes in source; new
strings follow the same convention.

### 6.2 Refusals (new strings; all `StatusKind::Warning`, `StatusLifetime::Sticky`,
`StatusSource::Host`, via `set_status_full`)

| Trigger | Sites | Text |
|---|---|---|
| `ExportRefusal::NoMarkedBlock` | `do_export` (covers commit + confirm) | `"no marked block — export cancelled"` (D2 verbatim) |
| `ExportRefusal::BufferChanged` | `do_export` (covers commit + confirm) | `"buffer changed — export cancelled"` |
| Write-Block origin mismatch | `commit_destination` WriteBlock arm; `OverwriteWriteBlock` arm | `"buffer changed — write block cancelled"` |
| Pandoc missing at redirect | `redirect_to_export` | `"pandoc not found — install it to export"` — byte-identical to `run_export_with_probe`'s, but `Error` kind like its original |

The pre-existing Write-Block `"no marked block"` (`Info`, via `set_status`) refusals keep their
current kind and wording — this spec adds strings; it does not restyle shipped ones.

---

## 7. Invariants

1. **Palette exports stay whole-document (D1).** The only production constructors of
   `DestinationPurpose::Export` are `run_export_with_probe` (hardcodes
   `ExportScope::WholeDocument`) and `redirect_to_export` (derives from the incoming purpose;
   `MarkedBlock` iff the flow began as `WriteBlock`). Adding a third constructor is a
   compiler-visible act (the variant's fields must be filled). Test-enforced via §11-T2/T7.
   This preserves the codebase's own law — `select_marked_block`'s doc comment: *"The marked
   block is a target, not implicit scope"* — for every entry that is not the explicit ^KW flow.
2. **Scope is a flag; offsets are read once, at dispatch, on the main thread** — never stored,
   never read on the worker thread. The worker receives an owned `String`.
3. **Refusal writes nothing and sends nothing.** Every §6.2 refusal returns before any
   filesystem effect and before any thread spawn; no `ExportDone` is fabricated.
4. **Behavior preservation elsewhere.** SaveAs flows, Select/Recents pickers, extension policy,
   `classify_destination_enter`/`classify_highlight_target` (A23's territory), the TOCTOU
   guard, sinks/argv/temp-path composition, and every WholeDocument-path string are unchanged
   byte-for-byte.
5. **Idle/resource behavior unchanged** — no new timers, polling, or background work; every new
   check runs inside an existing user-driven dispatch.

---

## 8. Compiler-forced site census

From the verified 13-site `DestinationPurpose::Export` census plus the `WriteBlock`
unit→struct change and the carrier/message changes. "Forced" = fails to compile until edited.

**Production (13):**

| File | Symbol / arm | Change |
|---|---|---|
| `file_browser.rs` | `DestinationPurpose` | enum reshape (§4.2) |
| `export.rs` | `ExportScope` (new), `PendingExport`, `resolve_export_input` (new), `ExportRefusal` (new), `do_export`, `run_export_with_probe` | §4.1/4.3/5.4/5.1 |
| `editor.rs` | `PendingWriteBlock` (new), `pending_write_block` field type | §4.4 |
| `blocks_marked.rs` | `block_write` | `WriteBlock { origin }` construct |
| `file_browser_commit.rs` | `redirect_to_export` (probe param + derivation), `commit_destination` → `_with_probe` seam, noun-match `WriteBlock` pattern, `WriteBlock { origin }` arm (verify + `PendingWriteBlock`), `Export { ext, scope, origin }` arm | §5.2/5.3 |
| `file_browser_intercept.rs` | Enter arm | **unchanged** (calls the wrapper) |
| `file_browser.rs` | `footer_target` Redirect arm; `cancel_destination` | §6.1-2 / §5.6 |
| `prompts.rs` | `OverwriteExport`, `OverwriteWriteBlock`, intercept `ExportDone` arm | §5.5/5.7 |
| `render_overlays.rs` | title match: `WriteBlock { .. }` pattern, `Export` arm reads `scope` | §6.1-3 |
| `app.rs` | `Msg::ExportDone` decl; `reduce_dispatch` arm forwards `scope` | §4.5 |
| `jobs_apply.rs` | `apply_export_done` signature + status | §5.7 |

Unforced and unchanged by design: `matches!(purpose, DestinationPurpose::SaveAs)` sites
(`redirect_to_export`, the `Nothing` arm), `Export { .. }`/wildcard matches
(`file_browser.rs` extension-policy gate, the `chosen` match, the noun match's Export arm),
`Msg::ExportDone`'s custom `Debug` arm.

**Tests (compiler-forced, ~15):** Export constructs/patterns — `export.rs` (picker-seed
assert), `file_browser_commit.rs` (redirect-target assert, export-commit journeys, the
destination-mode pattern test), `render_overlays.rs` (title fixture), `e2e.rs` (export
journey); WriteBlock constructs — `prompts.rs` picker tests (three), `file_browser_commit.rs`
end-to-end write-block test, `render_overlays.rs` title fixture; `Msg::ExportDone` constructs —
four in `app.rs`; `apply_export_done` direct calls — the failure-path tests in `jobs_apply.rs`.
Plus the redirect-arm tests migrating to `commit_destination_with_probe(.., || true)` (§5.3).
Every migration picks `origin = e.active().id` and, for Export, the scope the fixture means —
a test that only compiles is not done; §11 adds the fixtures that *bite*.

None of the touched files carries a `module_budgets` hub budget. `commit_destination` grows by
two small verify/refusal blocks under its existing reasoned `#[allow(clippy::too_many_lines)]`;
`do_export` gains a reasoned `#[allow(clippy::too_many_arguments)]` (§5.4).

---

## 9. Command-surface contract conformance

Per `docs/design/command-surface-contract.md`: **N/A — this effort does not touch the command
surface**, argued rather than asserted:

- No command is added, removed, renamed, or re-categorized. The four registry export commands
  (`export_html`/`export_docx`/`export_pdf`/`export_tex`, `MenuCategory::Export`) keep their
  exact registrations and remain whole-document (invariant §7.1).
- No user-settable option is introduced. `ExportScope` is **flow context** — derived from how
  the writer entered the flow — not a persisted setting: no `SettingsSnapshot` field, no config
  key, no state that outlives the flow. Law 2 ("every user-settable option is a command")
  therefore does not attach; there is nothing to set.
- Palette (law 3), menu (law 4), keybindings/hints (law 7): membership and hints are untouched.
  ^KW's binding to `block_write` is unchanged.
- **The law-10 shadow, stated openly:** after this effort, a block-scoped export exists as a
  capability reachable by NO command — only via the ^KW → redirect path. That is not a
  violation (the laws govern commands and options that exist; law 10's test is "should a plugin
  ever be able to do this?", and answering yes would mean *adding* `export_*_block` commands or
  a post-P parameterized export). Making block-scoped export plugin-reachable is a deliberate
  future act with full contract consequences (laws 3, 4, 8, 10) — out of A22-as-bug, and
  recorded here so the contract's history is not surprised by it later.

---

## 10. What could go wrong (designed-for failure modes)

- **Mark collapses or is undone mid-flow** → the dispatch re-read refuses (D2). Reachable, not
  theoretical: a background `FilterDone`/`TransformDone` merge can collapse the block (the
  funnel clears `start >= end`), and a plugin can drive edits through the pump mid-picker.
- **Active buffer switches mid-flow** (plugin `next_buffer`/`prev_buffer` via the pump — no
  overlay guard) → origin verification refuses at whichever dispatch moment comes next, for
  Export and Write-Block both (D4-iv).
- **Pandoc missing** → refused at the redirect, before the writer invests in the Export picker;
  the original picker and field survive (§5.2).
- **A markdown slice cut mid-construct** (half a table, an unclosed fence) exports as pandoc
  parses it — garbage-in-garbage-out, identical to ^KW's block write today. Not guarded; noted
  so review does not rediscover it.
- **Target appears between check and finalization** → the existing TOCTOU guard in
  `apply_export_done`, unchanged and scope-agnostic.

---

## 11. Test strategy

Two binding lessons govern. **E11:** for a conjunctive predicate, each conjunct needs a fixture
where that conjunct alone decides the outcome — here the dispatch predicate is
`scope carried ∧ origin unchanged ∧ mark present`, so each test below flips exactly one
conjunct against a baseline that passes. **C5:** render the screen, don't assert the struct —
chrome surfaces are asserted on painted frames / user-visible strings, not on enum fields.

**The fixture that fails if scope is dropped (T1):** buffer `"AAA\nBBB\nCCC\n"` with
`marked_block` over `"BBB\n"`. `resolve_export_input(e, MarkedBlock, id)` must return exactly
`"BBB\n"`; `WholeDocument` must return the full text. A regression that drops scope anywhere on
the carrier path collapses the two and T1/T6 fail. No pandoc needed — the seam is upstream of
the subprocess.

- **T1 — `resolve_export_input`, four unit cases** (export.rs): whole / block / block-gone
  (`NoMarkedBlock`) / buffer-switched (`BufferChanged`, via a second buffer + `switch_to_index`).
  Each case flips one conjunct.
- **T2 — redirect derivation, typed path:** WriteBlock picker (origin = active id), field
  `report.docx`, Enter via `commit_destination_with_probe(.., || true)` → reopened picker's
  purpose is `Export { ext: "docx", scope: MarkedBlock, origin: <same id> }` AND the status
  reads `"docx is an export format — opening Export for the marked block"`. SaveAs twin →
  `WholeDocument` + today's status byte-for-byte (the conjunct-isolating contrast).
- **T3 — redirect derivation, Row-2 path:** empty field, highlighted existing `report.docx`
  in a WriteBlock picker → same purpose assertions, Row-2 wording.
- **T4 — probe gate:** same setup as T2 with `|| false` → status
  `"pandoc not found — install it to export"`, `editor.file_browser` still `Some` with mode
  still `WriteBlock` and the field intact, quit-drain state untouched, no Export picker.
- **T5 — dispatch refusals through `do_export`:** MarkedBlock scope with no mark → status
  `"no marked block — export cancelled"`, no file created, and no `ExportDone` on the channel
  (`try_recv` is `Err(Empty)` — nothing was spawned to send one); origin-mismatch twin →
  `"buffer changed — export cancelled"`.
- **T6 — confirm-boundary scope carriage (the D2 post-confirm moment):** seed
  `pending_export = Some(PendingExport { scope: MarkedBlock, origin, .. })` with the mark then
  CLEARED, fire `PromptAction::OverwriteExport` → the cancel status, nothing written. This
  fixture fails if the confirm boundary silently degrades scope to `WholeDocument` — which
  would export happily — isolating the carriage conjunct.
- **T7 — palette invariant:** extend the existing `run_export_with_probe` picker-seed test to
  assert the purpose equals `Export { ext: "html", scope: WholeDocument, origin: active id }`.
- **T8 — Write-Block origin verify, both moments:** open ^KW picker, switch buffers, commit →
  `"buffer changed — write block cancelled"`, no write; twin through
  `PendingWriteBlock`/`OverwriteWriteBlock`.
- **T9 — chrome, rendered (C5):** paint the file browser (`TestBackend`, the existing
  `render_overlays.rs` destination-title fixture pattern) with a `MarkedBlock` Export purpose →
  the title row contains `" Export .pdf (marked block) to:"`; the `WholeDocument` twin renders
  today's title byte-for-byte. Footer: a WriteBlock picker with field `notes.html` → the
  rendered frame contains `"(exports the marked block)"` (assert the frame; `footer_target` is
  the single source the painter consumes, but the assertion is on painted cells).
- **T10 — completion status:** `apply_export_done` with an injected `Ok(Bytes(..))` and
  `MarkedBlock` → status exactly `"exported block to {target}"`; `WholeDocument` twin → exactly
  `"exported {target}"`. **What the existing `app.rs` assertion does and does not constrain:**
  the ExportDone finalization test asserts only `status_text().contains("exported")` — that
  substring passes under BOTH wordings and under a scope-dropped regression, so it constrains
  the write happening at all, and nothing about scope. It stays (it guards the write); T10
  carries the scope discrimination with exact-match assertions.
- **T11 — mid-flow edit remap (the reason scope is a flag):** open the flow, apply an edit
  through the funnel that shifts the mark (insert before `start`), dispatch → the export
  contains the block's CURRENT text. This is the fixture that fails under the rejected
  stored-offsets design and passes under re-read.

Suites ride where their subjects live (`export.rs`, `file_browser_commit.rs`, `prompts.rs`,
`render_overlays.rs`, `jobs_apply.rs`); no new test infrastructure. Existing suites must pass
unmodified except the §8 compiler-forced migrations and the two tests whose asserted strings
this spec deliberately changes (none — all changed strings are new-path-only; WholeDocument
strings are preserved byte-for-byte). PTY smoke suite: run per project law
(mandatory-run, advisory-pass); no smoke check exercises export, so no change is expected.

---

## 12. Out of scope / residuals (recorded, not silently dropped)

- **A23** — typed-vs-highlighted foreign-extension asymmetry (typing `notes.rtf` is honoured
  and silently writes markdown under a foreign name; highlighting the same file is refused).
  Filed separately per D4 with the grounding's §1.1 evidence.
- **SaveAs origin verification.** SaveAs shares the hazard *shape* (it writes the ACTIVE
  buffer's content at save time; a pump-driven buffer switch mid-picker would save the wrong
  buffer's content to the chosen path), but D4-iv scopes verification to Write-Block and
  Export. The `PendingWriteBlock`/purpose-origin machinery makes a SaveAs extension a small
  follow-up; deliberately not designed here.
- **Restyling the pre-existing `"no marked block"` Info statuses** — untouched (§6.2).
- **The saved-source gate on the redirect path** — deliberately not added (D4; `do_export`
  needs no `document.path`).
- **`export_*_block` commands / plugin-reachable block export** — the §9 law-10 shadow; a
  future contract-touching effort if ever wanted.
