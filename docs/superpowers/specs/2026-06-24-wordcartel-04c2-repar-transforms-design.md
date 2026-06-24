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
  the modal `Prompt` / `PromptAction` / `action_for` infrastructure, and the three-site reducer
  dispatch (normal arm + `prompt` block + `minibuffer` block).

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

### 3.1 Region selection & paragraph snapping

- **No selection** → the transform runs on the **whole buffer** (repar's native batch mode).
- **A selection** → the selected range is **snapped outward to enclosing blank lines** before
  running repar, so repar always receives complete paragraphs (it reflows by
  blank-line-separated paragraph; a mid-paragraph fragment would be mangled). The **snapped**
  region is what gets replaced:
  - start → scan backward to the byte after the previous blank line (or buffer start);
  - end → scan forward to the byte before the next blank line (or buffer end).
  - "Blank line" = a line that is empty or all-whitespace. Snapping is a cheap byte/line scan;
    it does **not** require the block_tree.
- The transform replaces exactly the resolved region (whole buffer, or the snapped selection)
  with `run_transform`'s output, as a single undoable edit.

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

## 5. Data flow & the undoable merge

Two paths, chosen by the resolved region's byte length; both end in one undoable
`EditKind::Other` edit via `build_range_replace`.

### 5.1 Synchronous path (region < `TRANSFORM_ASYNC_THRESHOLD` = 1 MiB)
repar reflows ~1 MiB in ~5 ms — comfortably sub-frame — so the common case (a selection or a
normal-sized buffer) runs **inline** in `resolve_prompt`'s `Transform` arm:
1. resolve the region (whole buffer or snapped selection);
2. `run_transform(kind, region_text, width)`;
3. on `Ok(out)`: if `out == region_text`, **no edit** (no inode/undo churn, quiet status);
   else `build_range_replace(from, to, &out, doc_len)` → `Buffer::apply(.., EditKind::Other, ..)`
   on the active buffer; then `derive::rebuild` + `nav::ensure_visible`; status `"reflowed"` etc.
4. on `Err(e)`: status line with the error; **buffer untouched**.
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
4. `Msg::TransformDone` is handled in **all three reducer sites** (normal arm, `prompt` block,
   `minibuffer` block) so a result is never starved by an open modal — same rule as `FilterDone`.

The sync and async paths share the same region-resolve + merge helper, so they produce
**identical** results; the only difference is where `run_transform` runs.

## 6. Error handling & edge cases

(Per main design §15: degrade, don't abort; errors are values at the edges; never lose work.)

- **repar error** (`PResult::Err`) → status-line message, buffer untouched.
- **Empty buffer / empty selection / region that is all code or all blank** → repar returns the
  input unchanged → no edit committed (quiet "nothing to reflow" / unchanged status).
- **Output identical to input** → no edit (no undo/inode churn).
- **Stale async result** → version-discarded (never applied to a moved buffer).
- **One transform in flight at a time** (`transform_in_flight`); a second `Ctrl+T` while one is
  running reports a busy status (the async path only; sync completes before returning).

## 7. Components / module boundaries

| Unit | Responsibility | Depends on |
|------|----------------|------------|
| `transform.rs::run_transform` | typed, pure repar wrapper (the only repar-API site) | `repar` |
| `transform.rs::resolve_region` | whole-buffer vs snapped-selection range | buffer text/selection |
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
- **Round-trip law:** `reflow(unwrap(p)) == reflow(p)` as a `proptest` property over generated
  prose, with checked-in `proptest-regressions` seeds (repar's own invariant; also guards our
  wrapper composition across the two verbs).
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
