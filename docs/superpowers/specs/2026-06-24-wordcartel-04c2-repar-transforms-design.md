# Wordcartel Effort 4c-2 — `repar` In-Process Transforms (design)

**Status:** design (brainstormed 2026-06-24)
**Sibling efforts:** 4c-1 filter primitive + pandoc export (✅ merged); 4c-3 clipboard sync (later).
**Spec source:** main design §3.5, §14 (repar Integration & Line-Structure Model), §14.1 (in-process transforms), §14.4 (testing discipline); coverage ledger row 4c.

---

## 1. Goal

Add the three `repar` line-structure transforms — **Reflow**, **Unwrap**, **Ventilate** — as
**in-process**, markdown-aware editor commands that reformat the selection (or whole buffer)
as a single undoable edit. `repar` is the author's own MIT prose/markdown reformatter
(`../../par-command/repar`, v0.9.10, I/O-free core, `#![forbid(unsafe_code)]`); Wordcartel is
its interactive front-end. This effort depends on `repar` as a **library** — no subprocess, no
"is the binary installed?" concern.

The three transforms (one per invocation):
- **Reflow** → hard-wrap prose to a target width (the *publish* form). repar's byte-exact default.
- **Unwrap** → one logical line per paragraph (the *soft-wrap-ready* form).
- **Ventilate** → one sentence per line (semantic line breaks; VCS-diff-friendly).

## 2. Architecture

Functional-core/imperative-shell holds: `repar` is the I/O-free formatting core; Wordcartel's
new code (region selection, dispatch, threading, the undoable merge) lives in the **shell crate**
(`wordcartel`, `#![forbid(unsafe_code)]`). `wordcartel-core` is untouched.

- **New file:** `wordcartel/src/transform.rs` — the typed `repar` wrapper, `TransformKind`,
  region snapping, the synchronous engine, and the async-dispatch entry point.
- **New dependency:** `repar = { path = "../../par-command/repar" }` in `wordcartel/Cargo.toml`
  (path is relative to that manifest; verify at implementation time). Not added to
  `wordcartel-core`.
- **Reused from 4c-1 (no changes to their contracts):** `commands::build_range_replace`,
  `Buffer::apply` with `EditKind::Other`, `editor::by_id_mut`, the version-discard merge shape,
  the modal `Prompt` / `PromptAction` / `action_for` infrastructure, and the async-result reducer
  dispatch. **Accurate reducer model (per the real 4c-1 code):** `FilterDone`/`ExportDone` have
  **explicit arms in two places** — the normal match arm and the `editor.prompt.is_some()`
  interception block — while the `editor.minibuffer.is_some()` block intercepts **only key
  events** and lets non-key messages fall through to the normal arm. `Msg::TransformDone` follows
  the same shape (explicit arms in the normal match + the prompt block; minibuffer fall-through),
  NOT a literal third "minibuffer arm."
- **Reused from Effort 3 (block tree):** every buffer already maintains an incrementally-updated
  `document.blocks: BlockTree` (`wordcartel-core::block_tree`); each `Block` has
  `kind: BlockKind` and `span: Range<usize>`, with `BlockTree::top_level()`. 4c-2 reads this for
  markdown-structural region snapping (§3.1) — no new parsing.

## 3. The transform engine (typed `repar` wrapper)

All `repar` stringliness is contained in one function, so the rest of Wordcartel sees a typed
API and a future swap to a typed `repar` builder (§10) touches only this function.

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransformKind { Reflow, Unwrap, Ventilate }

impl TransformKind {
    /// The repar CLI verb that selects this transform.
    fn verb(self) -> &'static str {
        match self {
            TransformKind::Reflow => "--reflow",
            TransformKind::Unwrap => "--unwrap",
            TransformKind::Ventilate => "--ventilate",
        }
    }
}

/// Run a repar transform over `input`, markdown-aware. Pure (no IO); returns the
/// reformatted text or a transform error. This is the ONLY place that touches
/// repar's stringly public API.
pub fn run_transform(kind: TransformKind, input: &str, width: u32)
    -> Result<String, TransformError>
{
    let mut opts = repar::Options::new().width(width);
    // apply_par_args takes &mut self and returns PResult<()> — not chainable.
    opts.apply_par_args([kind.verb()]).map_err(TransformError::from_repar)?;
    opts.apply_fixups("markdown").map_err(TransformError::from_repar)?; // Compat::MARKDOWN
    opts.format(input).map_err(TransformError::from_repar)
}
```

- **Markdown mode is ON by default** (`apply_fixups("markdown")` → `Compat::MARKDOWN`): code
  blocks, tables, and headings pass through **verbatim**; only prose is reflowed. This is the
  correct default for a markdown editor and comes free from `repar`.
- **Width default = 72** (repar's native default). A fixed constant `DEFAULT_REFLOW_WIDTH: u32 = 72`
  for v1; this makes the round-trip / differential laws (§8) byte-exact against `repar`. The
  wrap-ruler column that will eventually override this is **Effort 5** (out of scope here).
- `TransformError` is a `thiserror` enum wrapping `repar::ParError`'s message (errors are values
  at the boundary; the core does not panic).

### 3.1 Region selection & markdown-structural snapping

> **Codex spec-review fix (CRITICAL):** blank-line snapping is **not** markdown-structural. A
> fenced code block can legally contain a blank line, and loose list items / blockquotes span
> blank lines; snapping to blank lines could hand repar a fragment that splits a construct (e.g.
> the closing ```` ``` ```` of a fenced block without its opener), which repar — classifying purely
> on the supplied slice — would mis-handle and corrupt. The region unit is therefore the **block
> tree**, not blank lines.

- **No selection** → the transform runs on the **whole buffer** (repar's native batch mode).
- **A selection** → the range is **snapped outward to whole top-level blocks** using the buffer's
  already-maintained `document.blocks` (§2): take every `top_level()` `Block` whose `span`
  intersects the selection and expand the region to `[min(span.start) … max(span.end)]`. Because
  the unit is a complete block, a selection landing inside a fenced code block, list, blockquote,
  or table pulls in the **whole** construct — repar (in markdown mode) then passes those
  constructs through verbatim and reflows only the prose blocks, with no possibility of splitting
  a construct. The snapped region is what gets replaced.
  - Trailing inter-block whitespace/newlines between the chosen first and last block are included
    by virtue of the contiguous `[start..end]` span; the replaced text is the exact bytes
    repar reformatted, so block separation is preserved.
  - The block tree is already incrementally maintained per keystroke (Effort 3) — snapping is a
    bounded scan over `top_level()`, not a fresh parse.
- The transform replaces exactly the resolved region (whole buffer, or the block-snapped
  selection) with `run_transform`'s output, as a single undoable edit.

## 4. Invocation (UI)

No command palette exists yet, so (as with 4c-1's filter) the feature is bound to a real key.

- **`Ctrl+T`** ("transform") is bound in `input::key_to_command_id` → `CommandId("transform")`
  (Ctrl+T is currently free; the bound Ctrl chords are z/y/Z/c/x/v/s/q/`\`/e). Verify at
  implementation time.
- The `transform` command raises a **modal `Prompt`** (reusing the existing keypress-modal
  infra, like the quit/overwrite prompts — NOT the filter text minibuffer):
  `transform: [r]eflow  [u]nwrap  [v]entilate  ·  Esc cancel`
- `Prompt::transform_chooser()` builds the choices; `action_for(ch)` maps `r`/`u`/`v` to a new
  `PromptAction::Transform(TransformKind)`. `resolve_prompt`'s `Transform(kind)` arm dispatches
  the transform (§5) and clears the prompt. `Esc`/any other key cancels (clears the prompt).
- This is registry-dispatched (the `transform` command id) **and** key-bound, satisfying §14.1's
  "first-class commands"; it is deliberately **not** routed through the external-subprocess
  filter path (running the `repar` binary as a subprocess would defeat the in-process design and
  re-introduce the "is it installed?" problem).

**Precedence while a modal is open (Codex spec-review fix, IMPORTANT):** `Ctrl+T` maps to the
`transform` command **only in normal mode**. While `editor.prompt.is_some()` or
`editor.minibuffer.is_some()`, the reducer's interception blocks consume the keypress **before**
command dispatch, so `Ctrl+T` is naturally swallowed by the open modal and the chooser does not
open (a filter minibuffer, export-overwrite prompt, or quit prompt keeps focus). This is the
intended behavior — one modal at a time. No special-casing is added; the spec states it so the
implementer does not "fix" the swallow.

## 5. Data flow & the undoable merge

Two paths, chosen by the resolved region's byte length; both end in one undoable
`EditKind::Other` edit via `build_range_replace`.

### 5.1 Synchronous path (region < `TRANSFORM_ASYNC_THRESHOLD` = 1 MiB)
repar reflows ~1 MiB in ~5 ms — comfortably sub-frame — so the common case (a selection or a
normal-sized buffer) runs **inline** in `resolve_prompt`'s `Transform` arm:
1. resolve the region (whole buffer or snapped selection);
2. `run_transform(kind, region_text, width)`;
3. on `Ok(out)`: if `out == region_text`, **no edit** (no inode/undo churn; "already …" status
   per §6.2); else `build_range_replace(from, to, &out, doc_len)` →
   `Buffer::apply(.., EditKind::Other, ..)` on the active buffer; then `derive::rebuild` +
   `nav::ensure_visible`; success status per §6.2.
4. on `Err(e)`: error status per §6.2; **buffer untouched**.
   (Borrow-split discipline as in 4c-1: assemble status in a local; end the `by_id_mut` borrow
   before `derive::rebuild(editor)` / setting `editor.status`.)

### 5.2 Asynchronous path (region ≥ 1 MiB)
For a very large whole-buffer transform (5 MiB reflow ≈ 12–28 ms, over one 16 ms frame), keep
typing responsive by running off-thread, mirroring 4c-1's `FilterDone` exactly:
1. capture `(buffer_id, version, range, region snapshot)`; set a one-at-a-time
   `transform_in_flight` guard; status `"reflowing …"`.
2. spawn a thread that materializes the region string from the snapshot and calls
   `run_transform`, then sends `Msg::TransformDone { buffer_id, version, range, result }`
   (`result: Result<String, TransformError>`).
3. **Foreground merge with version-discard:** in `apply_transform_done`, clear the in-flight
   guard; if the buffer's current `document.version` ≠ the message `version` → **discard** with a
   "transform discarded — buffer changed" status (buffer untouched); else apply exactly as the
   sync path (build_range_replace → one `EditKind::Other` edit → rebuild + ensure_visible), or
   surface the error. Merge targets the originating buffer via `by_id_mut(buffer_id)`.
4. `Msg::TransformDone` gets **explicit arms in the normal match and the `prompt` interception
   block**; the `minibuffer` block intercepts only key events, so a `TransformDone` arriving while
   the minibuffer is open **falls through** to the normal arm — never starved by an open modal,
   exactly as `FilterDone`/`ExportDone` are wired today.

The sync and async paths share the same region-resolve + merge helper, so they produce
**identical** results; the only difference is where `run_transform` runs.

## 6. Error handling & edge cases

(Per main design §15: degrade, don't abort; errors are values at the edges; never lose work.)

- **repar error** (`PResult::Err`) → status-line message (§6.2), buffer untouched.
- **Empty buffer / empty selection that snaps to no block** → `nothing to transform` (§6.2), no
  edit.
- **Region that is all code / all-verbatim constructs** → repar returns the input unchanged → no
  edit; `already …` status (§6.2).
- **Output identical to input** → no edit (no undo/inode churn); `already …` status.
- **Stale async result** → version-discarded (never applied to a moved buffer).
- **One transform in flight at a time** (`transform_in_flight`); a second transform dispatched
  while one is running reports a busy status (the async path only; sync completes before
  returning). `transform_in_flight` is cleared on **every** `TransformDone` path (applied,
  discarded, error).

### 6.1 Filter/transform concurrency policy (Codex spec-review fix, IMPORTANT)

A transform and a 4c-1 **filter** (or export) may both be in flight on the same buffer — their
guards (`transform_in_flight` vs `filter_in_flight`) are independent and **we do not proactively
cross-block** (that would require modifying 4c-1's already-merged `dispatch_filter`). Correctness
is guaranteed **reactively by version-discard**: whichever result lands first applies and bumps
`document.version` via `Buffer::apply`; the second result then sees a changed version and is
**discarded** with the "buffer changed" status. No corruption, no lost-but-silent edit — the user
sees exactly one transform/filter applied and a discard notice for the other. (A future unified
"one background mutation at a time" policy is possible but is **not** required for 4c-2.)

### 6.2 Status-message contract

Exact, specific status strings (matching 4c-1's concrete style — no vague "done"):

| Event | Status |
|-------|--------|
| Chooser open | `transform: [r]eflow  [u]nwrap  [v]entilate` (prompt line) |
| Sync success | `reflowed` / `unwrapped` / `ventilated` |
| Async start | `reflowing…` / `unwrapping…` / `ventilating…` |
| Output unchanged | `already reflowed` / `already unwrapped` / `already ventilated` (no edit) |
| Empty / no prose region | `nothing to transform` |
| repar error | `transform failed: <repar message>` |
| Stale async discard | `transform discarded — buffer changed` |
| Busy (transform in flight) | `a transform is already running` |

## 7. Components / module boundaries

| Unit | Responsibility | Depends on |
|------|----------------|------------|
| `transform.rs::run_transform` | typed, pure repar wrapper (the only repar-API site) | `repar` |
| `transform.rs::resolve_region` | whole-buffer vs block-snapped selection range | selection + `document.blocks` (block tree) |
| `transform.rs::dispatch_transform` | sync-vs-async decision; thread spawn; in-flight guard | core merge, `Msg` |
| `app.rs::apply_transform_done` | foreground version-discard merge (async) | `build_range_replace`, `by_id_mut` |
| `prompt.rs::transform_chooser` + `PromptAction::Transform` | the modal chooser | existing Prompt infra |
| `input.rs` Ctrl+T → `CommandId("transform")` | key binding | registry |
| `registry.rs` `transform` command | raise the chooser | `Ctx.editor` |

## 8. Testing (adopts repar's discipline, §14.4)

- **Golden corpus (non-tautological wiring check):** committed `(input, kind) → expected output`
  fixtures with the expected text written by hand / captured from the `repar` **CLI**
  (`repar --unwrap --fixups=markdown -w72`, an *independent* oracle), asserting `run_transform`
  reproduces it. This pins that our argument wiring — verb + markdown fixup + width — matches the
  intended `repar` invocation and stays correct across `repar` upgrades. (Comparing `run_transform`
  to a freshly-constructed `repar::Options` in-process would be tautological — we *are* that call;
  the fixtures must come from an independent source.) Cases cover: prose reflow, a fenced code
  block passed through verbatim, a heading + table untouched, a list item rewrapped.
- **Round-trip law (a 4c-2 obligation to VERIFY, not a pre-confirmed repar invariant — Codex
  spec-review, MINOR):** `reflow(unwrap(p)) == reflow(p)` as a `proptest` property over generated
  prose, with checked-in `proptest-regressions` seeds. repar advertises this round-trip in its CLI
  help, but its committed markdown-mode property tests do not cover `unwrap` composition, so 4c-2
  treats it as a property to **prove for our markdown-mode wiring**. If it does not hold in
  markdown mode for some input class, that is a finding to resolve (narrow the law's domain or
  fix the wiring) — not a silent skip.
- **Paragraph-snap unit tests:** a mid-paragraph selection snaps out to the enclosing blank
  lines; selection at buffer start/end; selection already on blank-line boundaries (no-op snap);
  multi-paragraph selection.
- **Merge tests** (mirroring 4c-1): a transform applies as exactly one undoable edit (one undo
  restores the original); markdown structural passthrough (a region with a fenced code block +
  prose reflows only the prose); identical-output → no edit; async version-discard discards a
  stale result; sync and async paths yield identical text for the same region.
- **No prior test weakened;** `cargo build --workspace` zero warnings; functional-core untouched.

## 9. Non-goals (explicit)

- **Wrap-guide ruler / soft-wrap line-structure UI (§14.2)** → Effort 5.
- **Per-file / config width** and a config system → later; v1 uses the fixed 72 default.
- **"Ventilate as on-disk storage option" save filter (§14.2)** → a later save/export concern,
  not this effort.
- **Routing transforms through the external-filter subprocess path** → never; transforms are
  in-process by design.
- **Width entry in the chooser** → the chooser is single-key (r/u/v); width override waits for
  the ruler/config in Effort 5.

## 10. Future: optional `repar` typed-builder PR

We drive `repar` via its existing **public, stable** entry points (`apply_par_args`,
`apply_fixups`, `format`), kept behind `run_transform`. A **future, optional** enhancement to
`repar` (the author's own crate) would add a typed library API — re-export `Transform` from
`lib.rs` and add `Options::transform(Transform)` + `Options::markdown(bool)` builder methods —
letting `run_transform` call `Options::new().width(w).transform(Transform::Unwrap).markdown(true)`
instead of the stringly verbs. Because all stringliness is isolated in `run_transform`, adopting
that later changes exactly one Wordcartel function and zero call sites. Tracked as a repar PR,
**not** a blocker for 4c-2.
