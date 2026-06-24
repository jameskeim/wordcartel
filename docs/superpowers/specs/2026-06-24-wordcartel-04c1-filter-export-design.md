# Wordcartel Effort 4c-1 — Filter Primitive & Pandoc Export (Design Spec)

**Date:** 2026-06-24
**Status:** Design approved (brainstorm) — pending spec review → plan
**Parent spec:** `docs/superpowers/specs/2026-06-21-wordcartel-design.md` (§3.5 filter, §13.6, §15.4 subprocess, §3.5 export disposition, §18.5 security)
**Predecessors:** 4a (sync shell), 4b-1/4b-2 (async substrate + crash safety), 4r (Buffer extraction) — all merged.
**Coverage ledger:** `docs/superpowers/plans/2026-06-22-wordcartel-coverage-ledger.md`

---

## 1. Goal

Turn Wordcartel into a Unix-pipe platform: pipe the selection/buffer through an
external tool and route the result back, off the keystroke path, with a hard
security + robustness model. Ship the **subprocess engine**, the **preset
mechanism** the rest of 4c rides on, a minimal **filter-command minibuffer** for
arbitrary typed filters, and **pandoc export** presets.

This is **4c-1**, the first slice of the IO platform layer (parent §4c). Its
siblings are split out and brainstormed separately: **4c-2** repar transforms
(Reflow/Unwrap/Ventilate — in-process `repar` calls + filter presets) and **4c-3**
system-clipboard sync (`arboard`/OSC 52). See §9 Non-Goals.

## 2. Scope (decided)

- **In:** the `filter` subprocess engine (Filter / Insert / Export dispositions);
  the `FilterSpec` preset model; a minimal single-line filter-command minibuffer;
  pandoc export presets; the security + robustness model; off-hot-path execution on
  a dedicated filter thread with Esc-cancel + timeout; version-discard staleness.
- **Out (sibling 4c efforts):** repar transforms (4c-2); clipboard sync (4c-3).
- **Out (Effort 5):** the command palette / hideable menu / richer minibuffer
  (history, completion, fuzzy match); arbitrary **shell-pipeline** typed filters
  (`a | b`); save-as / arbitrary output-path entry for export.
- **Out (backlog):** streaming filters; concurrent multiple filters; filter output
  diff preview.

## 3. Global Constraints (inherited; bind every task)

- **Responsiveness #1** (§3.9): the foreground never blocks on the subprocess. A
  filter dispatches, shows `running <cmd> ⏳`, and the loop stays live. Esc cancels.
- **Functional core / imperative shell** (§10): all subprocess/thread/IO work lives
  in the `wordcartel` shell crate; `wordcartel-core` stays IO/thread-free.
- **Single mutation channel** (§10.1): a filter's document change happens **only**
  via the target buffer's `apply` — `editor.by_id_mut(buffer_id)`'s `Buffer::apply`
  (a single `Transaction`/`ChangeSet` replacing the captured range, `EditKind::Other`
  so it is **one undoable edit** and never coalesces with typing). It routes by
  `buffer_id` — **never** `Editor::apply()`/`active_mut()`, which would target
  whatever buffer is active at merge time, not the one the filter ran on (Codex
  review). A public validated range-replace helper is added (the existing
  `replace_changeset` builder in `commands.rs` is private; expose a checked variant).
- **Foreground owns all `Editor` mutation** (§10): the filter thread **never**
  touches `Editor`. It owns only the `Popen` (for kill) and sends a result. The
  `filter_in_flight` handle on `Editor` is set/cleared **only on the foreground** —
  in the dispatch, Esc, and `FilterDone` reducer arms (Codex review).
- **Reconcile = discard, not rebase** (§10.3): a filter result whose `(buffer_id,
  version)` no longer matches the live buffer is **discarded**, never applied over
  the user's intervening edit.
- **Security default = argv, no implicit shell** (§3.5, §18.5): argv arrays by
  default; `shell = true` is an explicit per-preset opt-in.
- **Never crash / never lose work** (§15): the buffer is replaced **only** after a
  successful, fully-collected, UTF-8-valid, under-cap result; failures surface
  non-blockingly; the source file is never touched by a filter; never `unwrap` on a
  subprocess path.
- **Multi-buffer ready** (4r): a filter targets the active buffer **at dispatch**
  and its result routes back by `buffer_id` (reuses 4r's `by_id_mut` routing +
  version staleness).
- **Workspace facts:** `cargo test` from repo root; binary `wcartel`. **New
  dependency `subprocess`** — added to `wordcartel/Cargo.toml` as an explicit plan
  step (it is not present today). The three new modules (`filter.rs`,
  `minibuffer.rs`, `export.rs`) do not exist yet.

---

## 4. Architecture

### 4.1 Modules (new, shell crate)

- `filter.rs` — the engine: `FilterSpec`, `FilterOutcome`, `run_filter` (spawns the
  dedicated thread, drives the subprocess, enforces timeout/cancel/size-cap/UTF-8).
- `minibuffer.rs` — `Minibuffer { prompt, text, cursor }`, a single-line text-input
  mode distinct from the keypress modal `prompt`.
- `export.rs` — pandoc presets + the startup `pandoc` probe.
- Touched: `editor.rs` (`minibuffer: Option<Minibuffer>`; a filter-in-flight
  handle), `app.rs` (`reduce` minibuffer key routing + a `Msg::FilterDone` arm),
  `registry.rs` (`filter` / `export_*` command-ids), `render.rs` (paint the
  minibuffer / `running ⏳`), `commands.rs`/`jobs.rs` as needed.

### 4.2 `FilterSpec` (the invocation/preset model)

```rust
pub struct FilterSpec {
    pub argv: Vec<String>,          // program + args; never split by a shell
    pub shell: bool,                // opt-in (presets only): run via `sh -c <joined>`
    pub disposition: Disposition,
    pub input: Input,
    pub timeout: std::time::Duration, // default 10s; Esc cancels regardless
    pub max_output: usize,          // byte cap on collected stdout
}

pub enum Disposition {
    Filter,                         // stdout replaces the input range (undoable)
    Insert,                         // stdout inserted at the cursor (undoable)
    Export(ExportSink),             // read-only; child produces a file
}
pub enum ExportSink {
    Capture { ext: String },        // we read child stdout, write <source-stem>.<ext>
    WritesOutput { ext: String },   // child writes its own file (e.g. pandoc -o); we pass the path
}
pub enum Input { SelectionElseBuffer, None, WholeBuffer }
```

- **Presets** are named `FilterSpec`s registered behind command-ids (`export_docx`,
  …); 4c-2 registers Reflow/Unwrap/Ventilate the same way.
- **Typed filters** (minibuffer): the entered line is **argv-split on ASCII
  whitespace** → `FilterSpec { argv, shell: false, disposition: Filter, input:
  SelectionElseBuffer, .. }`. **Typed input can NEVER set `shell: true`** (only
  presets can). **Known 4c-1 limitation (Codex review):** whitespace-splitting has
  **no quoting** — an arg with spaces (`prettier --parser=markdown` is fine;
  `sed 's/a b/c/'` or a path with a space is not). Pipelines and quoted/space args
  need a `shell=true` **preset** or Effort 5's richer command parsing; documented and
  accepted for 4c-1.
- **Shell presets** that interpolate dynamic values (a buffer path, a width) must
  pass them as **argv entries**, never spliced into the `sh -c` string (injection).
  4c-1 ships no path-interpolating shell preset; the rule binds 4c-2/5 presets.

### 4.3 Execution flow

1. **Invoke.** `filter` command opens the minibuffer (`> `). Enter builds a
   `FilterSpec` and dispatches. Presets (export) build a `FilterSpec` directly,
   no minibuffer.
2. **Capture at dispatch (foreground, O(1)).** Capture `(buffer_id, version,
   input_range, rope_snapshot)`: `input_range` = the primary selection if non-empty
   else the whole buffer (`Filter`), the empty range at the cursor (`Insert`), or the
   whole buffer (`Export`); `rope_snapshot` is the buffer's O(1) `snapshot()`. The
   foreground does **only** the O(1) clone + range capture — **slicing the range to a
   `String` and materializing stdin happens on the filter thread** (off the keystroke
   path). Set status `running <cmd> ⏳`. Set `editor.filter_in_flight = Some(handle)`
   (foreground-only). Spawn the **dedicated filter thread**, which owns the
   `subprocess::Popen` (the kill target). **One filter at a time** — a second
   invocation while `filter_in_flight.is_some()` is rejected with a status.
3. **Run (filter thread).** **Cancellation kill scope (plan-discovered):** the shell
   crate is `#![forbid(unsafe_code)]`, so `libc::kill(-pgid, …)` (process-group kill)
   is unavailable without a safe wrapper. **4c-1 kills the child only**
   (`terminate()` then `kill()` if it lingers) — sufficient because 4c-1 ships only
   **argv** filters + pandoc (no shell-pipeline grandchildren). Group-kill (via
   `nix::killpg`, a safe wrapper, with `PopenConfig { setpgid: true }`) is **deferred
   to when shell-pipeline presets ship** (4c-2/5). Drive
   `communicate_start(Some(stdin_bytes)).limit_time(timeout).limit_size(max_output).read()`
   — the `Communicator` form gives deadlock-safe concurrent stdin/stdout **plus**
   the timeout and size cap (the plain string `communicate()` lacks both and
   *silently replaces* invalid UTF-8, so it cannot reject binary). Collect **bytes**,
   then **`String::from_utf8` strictly** — reject (binary) on error. **Esc-cancel:**
   the foreground sets a shared atomic flag the thread observes (or the foreground
   kills the group directly via the handle); either way the child's group is killed.
   On completion the thread sends `Msg::FilterDone { buffer_id, version, range,
   outcome }` into the unified `Msg` channel. (It does **not** touch `Editor`.)
4. **Merge (foreground, version-checked).** `reduce`'s `FilterDone` arm clears
   `filter_in_flight`, then:
   - **Stale:** `by_id(buffer_id)` missing OR its `document.version != version` →
     **discard**, status `filter discarded — buffer changed`. (First real consumer
     of 4b's version-discard staleness.) The `input_range` is byte-valid against the
     live buffer precisely when `version` is unchanged (no edit ⇒ no byte shift —
     cursor-only motion doesn't bump `version`), so a fresh result's range is always
     in-bounds; a stale one is dropped before any apply, so no out-of-range apply or
     panic is possible.
   - **Success + fresh:** route to the target buffer via
     `editor.by_id_mut(buffer_id)` (NEVER `active_mut()`):
     - `Filter` → `b.apply(<one Transaction replacing input_range with output>,
       …, EditKind::Other, clock)` — one undoable edit; selection collapses to the
       replaced range's end.
     - `Insert` → `b.apply(<insert output at the captured cursor>, …, Other, clock)`.
     - `Export` → no document change; status `exported <path>`.
   - **Failure** (non-zero exit / timeout / cancelled / oversized / non-UTF-8):
     buffer + selection untouched; status shows the child's **stderr (truncated) +
     exit code**; stderr never enters the buffer.

`FilterOutcome` (what the thread returns): `Ok(String)` (collected stdout) for
Filter/Insert; `Ok(())`/written-path for Export; `Err(FilterError)` for the failure
modes above (each carrying a user-facing message).

### 4.4 Why a dedicated thread (not the 4b FIFO worker)

A filter can be slow and is user-cancellable; running it on the shared single
worker would starve swap-cadence and save jobs until it finished, and the blocked
worker couldn't be interrupted. A dedicated per-invocation thread keeps the
housekeeping worker free, owns the `Popen` for `kill`, and still delivers its result
through the same unified `Msg` channel the loop already drains — so the merge path
is uniform with `JobDone`. The "one filter at a time" rule keeps this to a single
extra thread.

## 5. Minibuffer (§4.1 `minibuffer.rs`)

```rust
pub struct Minibuffer { pub prompt: String, pub text: String, pub cursor: usize }
```

- `Editor.minibuffer: Option<Minibuffer>` — a **separate input mode** from the
  keypress modal `prompt` (which takes single-key choices). **Hard invariant: at
  most one of `prompt`/`minibuffer` is `Some` at any time** — every site that opens
  one asserts the other is `None` (not just a comment; enforced at the
  `Prompt::*`/minibuffer-open call sites). The minibuffer is for free text.
- **Reducer order (Codex review):** in `reduce`, the prompt-interception block runs
  first (unchanged); then, if `minibuffer.is_some()`, the minibuffer intercepts
  **only `Msg::Input` key events** — `Msg::FilterDone`, `Msg::JobDone`, and
  `Msg::Tick` are **never** intercepted and run their normal arms (the §4b lesson:
  background results must not be starved by an open input UI). Key routing when the
  minibuffer is active: printable → insert at `cursor`; Backspace → delete before
  `cursor`; Left/Right → move `cursor`; Enter → submit (consume the text, clear the
  minibuffer, dispatch the filter); Esc → cancel (clear, no-op).
- Rendered on the status row as `> <text>` with the caret at `cursor`.
- **Deliberately minimal** (no history, completion, or fuzzy match) so Effort 5's
  palette generalizes it (palette = minibuffer + results list + matcher).

## 6. Pandoc export (§4.1 `export.rs`)

- **Probe at startup:** attempt to detect `pandoc` (spawn / `which`); on `ENOENT`
  the export command-ids register but are **disabled with a one-time notice**;
  core editing is unaffected (§15.4). The probe result lives on the workspace.
- **Presets:** `export_docx` / `export_html` / `export_pdf` etc. as `FilterSpec {
  disposition: Export(WritesOutput { ext } | Capture { ext }), input: WholeBuffer,
  shell: false, argv: [pandoc, …] }`.
- **Output path is derived** from the active buffer's source path
  (`notes.md → notes.<ext>` beside it). A scratch/unnamed buffer cannot export (the
  command reports "save the file first"). Arbitrary output-path entry is **Effort 5**.
- **Overwrite confirm (Codex review):** if the target exists, raise the modal — but
  **NOT** the existing `PromptAction::Overwrite`, which already means "overwrite
  *save*" (it calls `save::overwrite_save`). Add a dedicated
  `PromptAction::OverwriteExport` (+ a `Prompt::export_overwrite(path)` constructor)
  whose resolver re-runs the export; misrouting export through the save-overwrite
  action would dispatch a save.
- **Atomic output (Codex review):** both sinks write to a **temp path then rename**
  on success, so a failed/partial child never clobbers an existing target.
  `WritesOutput`: pass `-o <tmp>` as argv (capture stderr/exit); on success rename
  `<tmp>` → `<path>`. `Capture`: read child stdout, write `<tmp>`, rename on success
  (reuse the `file`/`swap` atomic-write discipline). Export never mutates the buffer
  or the source `.md`.

## 7. Error handling (§15.4)

| Case | Behavior |
|---|---|
| Binary not found (`ENOENT`) | status notice; editing unaffected; export presets pre-disabled by the probe |
| Non-zero exit | buffer untouched, selection preserved; stderr (truncated) + exit code in the status line; stderr never inserted |
| Hung / slow | `running <cmd> ⏳`; **Esc** kills the child; per-spec `timeout` (default 10s) kills it too |
| Oversized stdout | abort past `max_output`; status `filter output too large`; buffer untouched |
| Non-UTF-8 stdout | rejected; status `filter produced non-text output`; buffer untouched |
| Buffer changed during run | result discarded (§4.3 stale); status `filter discarded — buffer changed` |
| Export-target write failure | status; buffer + source untouched |
| Second filter while one runs | rejected; status `a filter is already running` |

**Status scope (Codex review):** filter status messages use the single global
`Editor.status` line. Because filters are **one-at-a-time** and **version-discard**,
the only cross-buffer case is: dispatch on buffer A, switch to B, the A-filter
completes — its result is applied to A (by `buffer_id`, correct) but the *status* is
global. For 4c-1 that global status is acceptable (and a stale A-filter shows the
informative `filter discarded — buffer changed`). The multi-buffer status-routing
rule (prefix the buffer name when `result.buffer_id != active`) is the Effort-6
multi-buffer spec's job; 4c-1 does not implement per-buffer status.

## 8. Testing strategy

- **Engine (deterministic, real commands on the test box):** `cat` = identity
  `Filter` (output == input, one undo step); `tr a-z A-Z` = transform; a
  `shell=true` preset `sh -c 'exit 3'` = non-zero exit (buffer untouched, stderr in
  status); an oversized producer vs a tiny `max_output` = size-cap reject; a
  non-UTF-8 producer (`printf '\xff'`) = UTF-8 reject; `Insert` via `printf` at the
  cursor. Drive the **merge** by synthesizing `Msg::FilterDone` in `reduce` tests;
  cover the live thread with **one** deterministic-handshake integration test (no
  timing asserts), mirroring 4b's `ThreadExecutor` test.
- **Staleness:** dispatch → bump the buffer's version → deliver `FilterDone` →
  assert discard + status + buffer unchanged.
- **Undo:** a successful `Filter` replacement is a single undo step restoring the
  original range.
- **Minibuffer:** key routing through `reduce` (type / Backspace / Left-Right /
  Enter-submits-the-spec / Esc-cancels); `JobDone` not starved under an active
  minibuffer.
- **Export:** derived path for a named buffer; scratch buffer refused;
  overwrite-confirm modal; `Capture` vs `WritesOutput` using a stub "pandoc"
  (`cat`/a script) injected as the `FilterSpec` argv; pandoc-absent graceful-disable
  (probe returns absent → command reports disabled).
- **Determinism (§11.3)** + parallel-isolation: unique temp paths for any
  file-writing test (4b-2 discipline); injected `Clock`; no real sleeps except the
  one cancel/timeout handshake.

## 9. Non-Goals (explicit)

- **repar transforms** (Reflow/Unwrap/Ventilate) — sibling **4c-2** (in-process
  `repar` dep + filter presets registered through this engine).
- **System-clipboard sync** (`arboard`/OSC 52) — sibling **4c-3**.
- **Command palette / hideable menu / richer minibuffer** (history, completion,
  fuzzy) — **Effort 5**; this minibuffer is the minimal seed it generalizes.
- **Typed shell pipelines** (`a | b` in the minibuffer) — Effort 5 / a `shell=true`
  preset; the typed default is argv-only for safety.
- **Arbitrary export output-path entry / save-as** — Effort 5.
- **Streaming / multiple concurrent filters / output diff preview** — backlog.

## 10. Spec → parent-section traceability

| This spec | Parent § | Item |
|---|---|---|
| §4.2, §4.3 | 3.5 | filter primitive: Filter/Insert/Export dispositions, argv-default |
| §4.3, §4.4 | 3.9, 10.3, 15.4 | off-hot-path execution, Esc-cancel, deadlock-safe I/O |
| §4.3 merge | 10.1, 10.3 | filter result via `apply` (undoable); version-discard staleness |
| §3 security | 3.5, 18.5 | argv default, shell opt-in, UTF-8 validate, size cap, timeout |
| §5 | 12.2 | minibuffer as the minimal seed for Effort 5's palette |
| §6 | 3.5 export, 15.4 | pandoc presets, capture vs writes_output, startup probe, graceful disable |
| §7 | 15.4 | subprocess error handling (ENOENT, non-zero, hung, write-fail) |
