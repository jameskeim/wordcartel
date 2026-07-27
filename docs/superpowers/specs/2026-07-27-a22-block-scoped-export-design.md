# A22 — Block-scoped export: the Write-Block → Export redirect honours the mark

**Status:** design spec for effort branch `effort-a22-block-scoped-export` (base `8f5ec04`).
**Decisions:** `scratchpad/a22/decisions.md` (D1–D5) is binding; this spec implements it and does
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingExport {
    pub ext: String,
    pub target: PathBuf,
    pub scope: ExportScope,
    pub origin: crate::editor::BufferId,
}
```

`PartialEq, Eq` are ADDED to the existing `Debug, Clone` (spec gate round 6): T12c asserts
whole-value equality on `Option<PendingExport>`, which does not compile without them. All four
field types satisfy both (`String`, `PathBuf`, `ExportScope` per §4.1, and `BufferId`, whose
derives already include `PartialEq, Eq`). Whole-value equality is preferred over four
field-by-field assertions because T12c's purpose is to prove the commit arm forwards the WHOLE
struct — a field-wise assertion would silently stop constraining any field a future change adds.

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
   status per §6.2; return false; } }` — on refusal **no thread is spawned and no `ExportDone`
   is ever sent**; nothing downstream awaits one (`ExportDone` is fire-and-forget into the
   reducer), so no flow hangs.
2. `buffer_id = editor.active().id` — unchanged line; post-verification it equals `origin`.
3. Thread spawn, sinks, temp path, `run_pandoc`: unchanged. The `Msg::ExportDone` send adds
   `scope`. Returns `true`.

**Return value (new): `do_export` returns `bool` — `true` iff an export worker was dispatched,
`false` iff a §6.2 refusal fired.** This is the synchronous observable the refusal tests assert
on (§11-T5): "no worker was spawned" is otherwise a claim about thread scheduling that an
immediate channel/filesystem check cannot prove (the E11 vacuity class — the checked state is
also the not-finished-yet state). Precedent for a picker-flow fn whose return value carries
"did the follow-up start": `Editor::open_destination_picker` returns whether it opened, with a
doc comment mandating callers use the RETURN VALUE rather than sniffing state. Both production
call sites discard it (the refusal has already set its status; there is no follow-up control
flow to steer), so the fn is deliberately NOT `#[must_use]`; tests consume it.

Both existing `do_export` call sites (`commit_destination`'s Export arm; the `OverwriteExport`
prompt arm) now pass `scope`/`origin`; verification therefore covers BOTH dispatch moments with
one rule (D2), including whole-document palette exports (uniform origin verification — the
Export flow's context is captured at `run_export_with_probe` and verified like any other).

### 5.5 The confirm arms — `prompts::resolve_prompt`

- `PromptAction::OverwriteExport`: `if let Some(pe) = editor.pending_export.take() {
  do_export(editor, &pe.ext, &pe.target, msg_tx, true, pe.scope, pe.origin, Arc::clone(fs)); }` —
  the `bool` return is discarded (§5.4); verification and block re-read happen inside
  `do_export`, so D2's post-confirm refusal falls out.
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

**Production, compiler-forced (9 files):**

| File | Symbol / arm | Change |
|---|---|---|
| `file_browser.rs` | `DestinationPurpose` | enum reshape (§4.2) |
| `export.rs` | `ExportScope` (new), `PendingExport`, `resolve_export_input` (new), `ExportRefusal` (new), `do_export` (signature + `bool` return), `run_export_with_probe` | §4.1/4.3/5.4/5.1 |
| `editor.rs` | `PendingWriteBlock` (new), `pending_write_block` field type | §4.4 |
| `blocks_marked.rs` | `block_write` | `WriteBlock { origin }` construct |
| `file_browser_commit.rs` | `redirect_to_export` (probe param + derivation), `commit_destination` → `_with_probe` seam, noun-match `WriteBlock` pattern, `WriteBlock { origin }` arm (verify + `PendingWriteBlock`), `Export { ext, scope, origin }` arm | §5.2/5.3 |
| `prompts.rs` | `OverwriteExport` (new `PendingExport` fields), `OverwriteWriteBlock` (`PendingWriteBlock` take), intercept `ExportDone` arm (must supply `apply_export_done`'s new param) | §5.5/5.7 |
| `render_overlays.rs` | title match: `WriteBlock { .. }` pattern, `Export` arm reads `scope` | §6.1-3 |
| `app.rs` | `Msg::ExportDone` decl; `reduce_dispatch` arm (must supply `apply_export_done`'s new param) | §4.5 |
| `jobs_apply.rs` | `apply_export_done` signature + status | §5.7 |

**Production, required but NOT compiler-forced** (behavior changes the compiler will not
demand — the plan must carry them as explicit tasks with tests, since nothing fails to build if
they are forgotten): `file_browser.rs::footer_target`'s `Redirect` arm scope wording (§6.1-2 —
`purpose` is already destructured; the new wording is an addition, not a type change);
`file_browser.rs::cancel_destination`'s added `pending_export = None` (§5.6 — an added
statement); the origin-verify blocks themselves in the `WriteBlock` arm and
`OverwriteWriteBlock` (the *bindings* are forced by the variant reshape, the *checks* are not);
the §6.1 scope-keyed redirect-status wordings (a `WholeDocument`-only string would compile).
§11's T2/T3/T6/T8/T9 are the fixtures that catch each of these respectively.

Unforced and unchanged by design: `file_browser_intercept.rs`'s Enter arm (calls the unchanged
`commit_destination` wrapper and compiles as-is), `matches!(purpose, DestinationPurpose::SaveAs)`
sites (`redirect_to_export`, the `Nothing` arm), `Export { .. }`/wildcard matches
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

### 9.1 Law 10 — answered directly, not deferred

Law 10's test is *"should a plugin ever be able to do this?"* — and for block-scoped export the
answer is **yes, and it already can.** No new command is required to make that true.

**The governing fact:** `block_write` is a registered command —
`registry.rs`: `r.register("block_write", "Write Block to File…", Some(MenuCategory::Block), …)`,
also bound to ^KW. A plugin dispatches it through `Registry::dispatch` exactly as it dispatches
any other command; that opens the Write-Block destination picker, and a pandoc-producible
extension in that picker is precisely the path this effort makes block-scoped.

**The parity that settles it:** the four `export_*` commands do **not** give a plugin a
parameterized export either. `export::run_export` opens the Export destination picker; the
destination is chosen interactively. Commands are nullary today (law 10 says so explicitly:
"Commands stay **nullary** today; parameterized set-value commands … are an Effort-P concern").
So whole-document export's plugin reachability is *"dispatch a command, a picker opens"* — and
after this effort block-scoped export's plugin reachability is the same sentence with a
different command name. The two capabilities are equally reachable, by construction.

**Therefore this effort adds no capability that lacks a command.** It changes what an existing
command's flow *produces* (a block-scoped artifact rather than a whole-document one when the
writer redirects), not what surface exists to invoke it. `block_write` is the command; it was
registered before this effort and is unmodified by it.

**What would be a law-10 event, and is deliberately not in this effort:** a *parameterized*
block export — a plugin naming both the scope and the destination without a picker. That is the
same post-P parameterization law 10 already contemplates for set-value commands, and it applies
equally to `export_html` today. If it is ever built, it is built for whole-document and
block-scoped export together, as one deliberate act with full contract consequences (laws 3, 4,
8, 10). Recorded here so the contract's History is not surprised by it later.

**Review note (spec gate round 1).** Codex read the earlier draft of this section as conceding
"a capability reachable by NO command" and correctly called that a law-10 violation. The
concession was the draft's error, not the design's: it was written without `registry.rs:435` in
view. The corrected reading above is the human-adjudicated resolution (decision D5) — Codex's
finding is accepted as valid against the text it reviewed, and answered on the evidence rather
than by adding commands (which would be the excluded Option E) or by amending the contract.

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
conjunct against a baseline that passes. The rule applies **per assertion, not per test**: a
test sound on one conjunct proves nothing about its others (spec-gate round 3 caught exactly
this — T4's mode-survival assertion was sound while its drain assertion, with nothing seeded,
read the same on a correct and a wrong implementation). For every multi-assertion case below,
each assertion states or implies what it reads when that specific behavior is wrong; an
assertion whose expected value is also the fixture's default/untouched state is a defect.
**C5:** render the screen, don't assert the struct — chrome surfaces are asserted on painted
frames / user-visible strings, not on enum fields.

A third rule (spec-gate round 5): **sweep for carriage, not just vacuity.** `scope` and
`origin` are THREADED values; a test suite that pins both endpoints of a chain while leaving a
hand-off free is passed by an implementation that substitutes a default or **re-derives the
value locally** at that hand-off — and a re-deriving implementation produces correct-looking
output on every fixture where the re-derived value happens to equal the carried one, which is
every fixture that does not deliberately make them differ. Per hand-off, one test must fail if
THAT hand-off substitutes or re-derives. The carriage map below assigns each hand-off its
discriminating test — or says plainly where a hand-off is unconstrained and why that is sound.

| Hand-off | Discriminated by |
|---|---|
| flow start → purpose (^KW / palette) | T2 baseline; T7 |
| redirect → Export purpose (scope; carried origin) | T2 (+ its carried-origin fixture), T3 |
| Export purpose → commit arm → `do_export` (scope) | T12a |
| Export purpose → commit arm → `do_export` (origin) | T12b |
| commit arm → `PendingExport` (scope, origin) | T12c struct equality (switched-buffer variant for origin) |
| `PendingExport` → `do_export` (confirm) | T6 (seeded side) + T12c confirm leg |
| `do_export` → `Msg::ExportDone` (scope) | T5 positive control's scope assertion (T13a) |
| `Msg::ExportDone` → `apply_export_done`, via `reduce_dispatch` | T13b |
| `Msg::ExportDone` → `apply_export_done`, via `prompts::intercept` | T13c |
| WriteBlock purpose → commit-arm verify | T8 |
| commit arm → `PendingWriteBlock` (origin) | **unconstrained, and soundly so:** construction runs immediately AFTER the commit-arm origin verify, so `active().id == origin` there BY CONSTRUCTION — a re-deriving implementation is extensionally identical at this site, not merely untested. The site that matters is the confirm-time check, which T8's twin discriminates. |
| `PendingWriteBlock` → `OverwriteWriteBlock` verify | T8 twin |

A fourth rule governs every refusal assertion, because `do_export` spawns a thread: **absence
is scheduling-weak on the export path.** An immediate `try_recv()`/file-existence check after a
call that *might* have spawned a worker proves nothing — a wrongly-spawned worker may simply
not have finished, and the assertion reads the same either way (the E11 vacuity class: the
asserted state is also the not-done-yet state). The tree already encodes this:
`file_browser_commit.rs`'s export-commit tests and the `e2e.rs` export journey use BOUNDED
receives precisely because `do_export` is threaded. Refusal tests therefore assert on
**synchronous observables that a wrong implementation flips** — `do_export`'s `bool` return
(§5.4) and the refusal status — never on an immediate absence probe. Write-Block is different
in kind: `perform_block_write` runs `save_atomic_with_fs` synchronously on the main thread, so
file-absence checks ARE sound there (T8).

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
  - *Carried-not-recaptured origin (its own fixture — in the baseline above, active id ==
    origin, so a re-capturing implementation reads identically):* open the WriteBlock picker
    on buffer A, switch the active buffer to B (`switch_to_index` — the redirect runs in the
    `Commit` arm BEFORE the WriteBlock arm's origin verify, so it proceeds), type
    `report.html`, Enter with `|| true` → the reopened Export purpose's `origin` is **A**.
    **On an implementation that re-captures `editor.active().id` at the redirect (§5.2 step 4
    violated), it reads B and the assertion fails** — and the downstream `BufferChanged`
    refusal this carriage exists to arm would be silently laundered away.
- **T3 — redirect derivation, Row-2 path:** empty field, highlighted existing `report.docx`
  in a WriteBlock picker → same purpose assertions, Row-2 wording.
- **T4 — probe gate, two fixtures (the ordering conjunct needs its own):**
  - *Flow survival (WriteBlock):* same setup as T2 with `|| false` → status exactly
    `"pandoc not found — install it to export"`, `editor.file_browser` still `Some` with mode
    still `WriteBlock` and the field intact, no Export picker. What each assertion rules out:
    the exact status rules out an implementation with no gate (final status would be the
    redirect Warning wording) — and, because the entire commit is one synchronous dispatch
    with no frame painted between status writes, the final status IS the only status the
    writer can ever see; the mode/field assertions rule out an implementation that refuses but
    still reopens the picker as Export (mode would read `Export { .. }`). **Deliberately NOT
    claimed:** that the redirect `reason` was never *transiently* set before the refusal — a
    single terminal status read cannot distinguish "never set" from "set then overwritten",
    and since no render interleaves, the transient is not user-observable either. §5.2 step
    1's gate-first sequence therefore stands as prescribed code order for implementation and
    review; the TESTS constrain its observable consequences (this fixture and the next), not
    the internal write sequence.
  - *Gate-precedes-drain-abort (SaveAs, all three fields SEEDED):* seed
    `editor.quit_drain = Some(..)`, `editor.pending_save_as = Some(..)`, AND
    `editor.quit_drain_advance = true` — the complete field set `redirect_to_export`'s
    SaveAs arm clears together (`pending_save_as`/`quit_drain`/`quit_drain_advance`;
    `quit_drain_advance` is a `bool` defaulting `false`, so it MUST be seeded `true` or a
    wrong clear is indistinguishable from untouched). Open a SaveAs picker, type
    `report.docx`, Enter via `commit_destination_with_probe(.., || false)` → the pandoc
    refusal status, picker still `Some`/SaveAs, and `quit_drain` still `Some`,
    `pending_save_as` still `Some`, `quit_drain_advance` still `true`. Each seeded-non-default
    read rules out an implementation that clears THAT field before (or despite) the gate: on a
    gate-after-drain-abort implementation the three read `None`/`None`/`false` and each
    equality fails; with defaults unseeded, every one of them would read the same on correct
    and wrong implementations alike.
- **T5 — dispatch refusals through `do_export`:** MarkedBlock scope with no mark →
  `do_export(..) == false` AND status `"no marked block — export cancelled"`; origin-mismatch
  twin → `false` + `"buffer changed — export cancelled"`. Both observables are synchronous and
  neither is the default state: **on an implementation that dispatched when it should have
  refused, `do_export` returns `true` (the assertion fails immediately, independent of
  scheduling) and the status is not the refusal text** (dispatch sets no status), so either
  assertion alone catches the wrong path deterministically. No `try_recv`/file-absence probe is
  used (§11 preamble). **Positive control (proves the instrument):** a third case with the mark
  PRESENT and origin intact → `do_export(..) == true` and a BOUNDED `recv_timeout` yields
  exactly one `Msg::ExportDone` (any `result` — on a pandoc-less gate machine it is
  `Err(spawn)`, and `guarded_export` guarantees a spawned worker always sends exactly one
  message), demonstrating that `true`/message-arrival is what dispatch actually looks like —
  the refusal cases' `false` is therefore a discriminating reading, not a value that every
  execution produces. The received message's `scope` field must equal the dispatched
  `MarkedBlock` — this is the carriage map's `do_export → Msg::ExportDone` hand-off (T13a):
  an implementation that constructs the message with a default `WholeDocument` fails here and
  nowhere else.
- **T6 — confirm-boundary scope carriage (the D2 post-confirm moment):** seed
  `pending_export = Some(PendingExport { scope: MarkedBlock, origin, .. })` with the mark then
  CLEARED, fire `PromptAction::OverwriteExport` → **the load-bearing assertion is the status**,
  exactly `"no marked block — export cancelled"`. Synchronous and discriminating: if the
  confirm boundary silently degrades scope to `WholeDocument`, `resolve_export_input` returns
  `Ok(whole text)`, `do_export` dispatches, no refusal status is set — the assertion reads the
  pre-prompt status and fails. No file-absence probe (this path goes through `resolve_prompt`,
  which discards `do_export`'s return, and an immediate absence check is the §11-preamble
  vacuity). Positive-control twin: same seed with the mark PRESENT → bounded `recv_timeout`
  yields an `ExportDone`, proving the confirm path dispatches when it should.
- **T7 — palette invariant:** extend the existing `run_export_with_probe` picker-seed test to
  assert the purpose equals `Export { ext: "html", scope: WholeDocument, origin: active id }`.
- **T8 — Write-Block origin verify, both moments:**
  - *B unmarked:* open ^KW picker on buffer A, switch to a buffer B that has NO mark, commit →
    status exactly `"buffer changed — write block cancelled"`. **The exact status is the SOLE
    discriminator in this fixture** (corrected per round 5): an implementation missing the
    origin verify falls through to the mark re-read, ALSO refuses (`"no marked block"`), and
    ALSO leaves the target absent — so a file-absence assertion here reads the same on correct
    and verify-missing implementations and is corroborative only. B-has-no-mark additionally
    makes the exact status constrain refusal ordering (§5.3 step 1 before step 2): a
    mark-first implementation reads `"no marked block"` and fails the equality.
  - *B MARKED (the write-suppression discriminator):* same flow but B carries its own mark →
    correct implementation still refuses `"buffer changed — write block cancelled"` and the
    target does not exist; **an implementation missing the origin verify passes the mark
    re-read and synchronously writes B's block bytes to the target** — the absence assertion
    fails, and it is sound here because `perform_block_write` writes on the main thread
    (§11 preamble), no scheduling involved.
  - *Confirm twin:* commit on A with an EXISTING target (verify passes; `PendingWriteBlock {
    target, origin: A }` raised), switch to unmarked B, fire `OverwriteWriteBlock` → status
    exactly `"buffer changed — write block cancelled"`. Discriminates a confirm-site
    re-derivation (`origin := active().id`): that implementation passes its own check, hits
    the mark re-read on B, and reads `"no marked block"` — the equality fails. (Re-derivation
    at `PendingWriteBlock` CONSTRUCTION is unconstrained and soundly so — see the carriage
    map: post-verify, `active().id == origin` by construction.)
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
  carries the scope discrimination with exact-match assertions. T10 pins the
  `apply_export_done` ENDPOINT only — carriage INTO it, through the message and both reducers,
  is T13's job (round 5: any of those sites could substitute `WholeDocument` and T10 would
  still pass).
- **T11 — mid-flow edit remap (the reason scope is a flag):** mark a block, apply an edit
  through the funnel that shifts it (insert before `start`), then assert
  `resolve_export_input(e, MarkedBlock, id)` returns the block's CURRENT (post-remap) text —
  the observable is the seam's returned `String`, synchronous, no pandoc involved. This is the
  fixture that fails under the rejected stored-offsets design and passes under re-read.
- **T12 — Export commit-arm carriage (round 5; the hand-off `purpose → commit arm →
  do_export/PendingExport`).** All three cases drive Enter through
  `commit_destination_with_probe(.., || true)` on an Export picker REACHED VIA THE REAL
  REDIRECT from a ^KW flow on buffer A (purpose `Export { ext, scope: MarkedBlock,
  origin: A }`), so the value under test is the carried one, never a hand-seeded one:
  - *(a) scope, no-overwrite path:* CLEAR the mark (`block_clear`), Enter onto a
    non-existing target → status exactly `"no marked block — export cancelled"`. **A commit
    arm that hardcodes `WholeDocument` into the `do_export` call dispatches instead** — no
    refusal status is set, the equality fails. (This is the fixture the round-5 finding named:
    every earlier scope test either called `do_export`/`resolve_export_input` directly or
    seeded `PendingExport` by hand, leaving this arm free to substitute.)
  - *(b) origin, no-overwrite path:* mark intact on A, switch active to an UNMARKED buffer B
    (the Export arm has no commit-time verify — verification is `do_export`'s), Enter onto a
    non-existing target → status exactly `"buffer changed — export cancelled"`. **A commit arm
    that re-derives `origin := active().id` launders the switch:** verification then passes
    against B and the mark re-read refuses with `"no marked block — export cancelled"` — a
    DIFFERENT exact string, so the equality discriminates re-derivation, not just omission.
  - *(c) PendingExport construction, overwrite path:* switch active to B, Enter onto an
    EXISTING target → assert
    `editor.pending_export == Some(PendingExport { ext, target, scope: MarkedBlock, origin: A })`
    by direct struct equality — a construction that substitutes `WholeDocument` or re-derives
    `origin` (which reads B here, because the switch made carried and re-derived differ)
    fails the equality. Then fire `OverwriteExport` → status exactly
    `"buffer changed — export cancelled"` (the confirm leg, complementing T6's seeded side).
    State assertions, not chrome — C5's render-the-screen rule governs user-facing surfaces;
    carriage is state and is asserted as state.
- **T13 — scope carriage through the message layer (round 5; the hand-offs
  `do_export → Msg::ExportDone → reducers → apply_export_done`).**
  - *(a)* T5's positive control asserts the received `Msg::ExportDone.scope ==
    MarkedBlock` — pins the worker-side construction in `do_export`.
  - *(b)* Construct `Msg::ExportDone { scope: MarkedBlock, result: Ok(Bytes(..)), .. }` and
    drive it through the REAL `app::reduce` (the existing `app.rs` ExportDone-test pattern) →
    status exactly `"exported block to {target}"`. **A `reduce_dispatch` arm that forwards a
    default instead of the bound `scope` produces `"exported {target}"`** and fails the
    equality. `WholeDocument` twin → exactly `"exported {target}"` (rules out the opposite
    substitution).
  - *(c)* Same message pair delivered through `prompts::intercept` with a modal prompt open
    (the second delivery site) → same exact-status pair. Either reducer substituting
    `WholeDocument` is caught by its own case; T10's direct calls would catch neither.

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
- **Parameterized, non-interactive export** — a caller naming scope AND destination without a
  picker. Block-scoped export is plugin-reachable TODAY via the registered `block_write`
  command (§9.1); what does not exist — **equally for whole-document export**, whose four
  commands also open a picker — is a picker-free, argument-taking form. That is the post-P
  parameterization law 10 already contemplates; if ever built, it is built for both scopes as
  one deliberate contract-touching act (§9.1's closing paragraph).
