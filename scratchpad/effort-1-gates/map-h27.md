# H27 map — `#[allow(clippy::too_many_arguments)]` on seven dispatch functions

Method note: no rust-analyzer LSP tool is exposed in this environment (checked via
ToolSearch; none found), so this map was built by `grep -rn` across
`wordcartel/src/**/*.rs` plus manual reading of each call site's enclosing
function, cross-checked by re-running the greps with different anchors (bare
name, `crate::`-qualified name, definition-line exclusion). No rust-analyzer
`findReferences` cross-check was possible — this is a grep-only map. Where a
`grep "reduce("` naively also matched test-function names ending
`..._via_reduce()`, those were identified and excluded by inspection (noted
below).

---

## 1–4. The seven functions

### 1. `input::handle_key`
File: `wordcartel/src/input.rs`. Symbol: `handle_key`.

```rust
#[allow(clippy::too_many_arguments)] // C5 T5: +fs threads the seam through every dispatch site
pub(crate) fn handle_key(
    k: crossterm::event::KeyEvent,
    editor: &mut crate::editor::Editor,
    reg: &crate::registry::Registry,
    keymap: &crate::keymap::KeyTrie,
    ex: &dyn crate::jobs::Executor,
    clock: &dyn wordcartel_core::history::Clock,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>,
    fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
) {
```

**Call sites: 1 total — 1 production, 0 test.**
- `wordcartel/src/app.rs`, inside `reduce_dispatch` (production):
  `crate::input::handle_key(k, editor, reg, keymap, ex, clock, msg_tx, fs)`.

**Constructs a bundle internally:** yes — `registry::Ctx` (not `DispatchCtx`):
```rust
let mut ctx = crate::registry::Ctx { editor, clock, executor: ex, msg_tx: msg_tx.clone(),
    fs: std::sync::Arc::clone(fs) };
reg.dispatch(id, &mut ctx);
crate::app::hydrate_overlays(editor, reg, keymap);
```

**Parameter use:** all 8 are read directly in the body — `k` (code match), `editor`
(mutated throughout, also moved into `Ctx`), `reg` (`reg.dispatch`, then
`hydrate_overlays(editor, reg, keymap)`), `keymap` (`keymap.resolve`, then
`hydrate_overlays`), `ex`/`clock`/`msg_tx`/`fs` (moved/cloned into the local
`registry::Ctx`; `clock` is additionally passed straight to
`commands::run(..., editor, clock)` in the printable-fallthrough arm). None are
unread pass-throughs — every one is consumed to build `Ctx` or read directly.

---

### 2. `app::reduce`
File: `wordcartel/src/app.rs`. Symbol: `reduce`.

```rust
#[allow(clippy::too_many_arguments)] // C5 T5: +fs threads the seam through every dispatch site
pub fn reduce(
    msg: Msg,
    editor: &mut Editor,
    reg: &Registry,
    keymap: &crate::keymap::KeyTrie,
    ex: &dyn Executor,
    clock: &dyn Clock,
    msg_tx: &std::sync::mpsc::Sender<Msg>,
    fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
) -> bool {
```

**Call sites: 149 total — 1 production, 148 test.**
- Production (1): `wordcartel/src/app.rs`, inside `run()` —
  `let keep = { reduce(msg, &mut editor.borrow_mut(), &reg, &keymap, &executor, &clock, &msg_tx, &fs) };`
- Test (148): `wordcartel/src/app.rs` inside `mod tests` (everything from the
  `#[cfg(test)] mod tests {` opener to EOF — ~131 call sites, one per `#[test]`
  fn body); `wordcartel/src/mouse.rs` inside its `mod tests`; `wordcartel/src/overlays.rs`
  inside its `mod tests` (2 sites: `splash_intercept_precedes_marks`,
  `no_dwell_arming_while_splash_is_up`); `wordcartel/src/file_browser.rs` inside its
  `mod tests`; `wordcartel/src/e2e.rs` (3 sites — `e2e.rs` is gated
  `#[cfg(test)] mod e2e;` in `lib.rs`, i.e. wholly test-only).
- Grep/rust-analyzer disagreement note: a naive `grep "reduce("` also hits 9
  test-function *names* that happen to end `..._via_reduce()` (e.g.
  `active_edit_in_review_arms_via_reduce`, in `app.rs`, and
  `active_buffer_merge_eviction_surfaces_via_reduce` in `editor.rs`) — these are
  fn *definitions*, not calls, and are excluded from the 149. No rust-analyzer
  was available to cross-check independently; this exclusion was done by manual
  inspection of each hit.

**Constructs a bundle internally:** no. `reduce`'s body only calls
`reduce_dispatch(msg, editor, reg, keymap, ex, clock, msg_tx, fs)` and, on the
one return path, `arm_if_edited(editor, before_id, before_version, clock)`.

**Parameter use:** `msg` and `editor` are read directly (debug-assertions smoke
-panic match, `editor.active()` snapshot). `reg`, `keymap`, `ex`, `msg_tx`,
`fs` are pure pass-throughs to `reduce_dispatch` — not otherwise touched in
`reduce`'s own body. `clock` is passed to `reduce_dispatch` **and** used again
directly in the `arm_if_edited(...)` call on the same exit path.

---

### 3. `app::reduce_dispatch`
File: `wordcartel/src/app.rs`. Symbol: `reduce_dispatch` (private, `fn`, not `pub`).

```rust
#[allow(clippy::too_many_arguments)] // C5 T5: +fs threads the seam through every dispatch site
fn reduce_dispatch(
    msg: Msg,
    editor: &mut Editor,
    reg: &Registry,
    keymap: &crate::keymap::KeyTrie,
    ex: &dyn Executor,
    clock: &dyn Clock,
    msg_tx: &std::sync::mpsc::Sender<Msg>,
    fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
) -> bool {
```

**Call sites: 1 total — 1 production, 0 test.**
- `wordcartel/src/app.rs`, inside `reduce` itself:
  `let keep = reduce_dispatch(msg, editor, reg, keymap, ex, clock, msg_tx, fs);`

**Constructs a bundle internally:** yes — `DispatchCtx` itself, first statement
of the body:
```rust
let ctx = crate::overlays::DispatchCtx { reg, keymap, ex, clock, msg_tx, fs };
```

**Parameter use:** all 8 used, and used *twice* in most cases — once to build
`ctx` (which is then threaded through the whole 11-row overlay intercept chain
and the `marks` pre-stage), and again directly later in the same function body:
`reg`/`keymap` again in `hydrate_overlays(editor, reg, keymap)` (twice, once
per branch that opens an overlay); `ex`/`clock`/`msg_tx`/`fs` again in
`crate::jobs_apply::apply_job_outcome(o, editor, ex, clock, msg_tx, fs)` (two
call sites — `Msg::JobDone` arm and the tail-drain loop), `crate::timers::on_tick(editor, ex, clock, msg_tx, fs)`,
`crate::input::handle_key(k, editor, reg, keymap, ex, clock, msg_tx, fs)`, and
`crate::mouse::handle(editor, ev, reg, keymap, ex, clock, msg_tx, fs)`. `fs`
alone is also read a third way, re-derefed: `crate::jobs_apply::apply_export_done(editor, target, result, overwrite_confirmed, &**fs)`.
`editor` and `msg` are used pervasively and are NOT part of `ctx` (see §7).

---

### 4. `app::dispatch_overlay_command`
File: `wordcartel/src/app.rs`. Symbol: `dispatch_overlay_command`.

```rust
#[allow(clippy::too_many_arguments)] // C5 T5: +fs threads the seam through every dispatch site
pub(crate) fn dispatch_overlay_command(
    editor: &mut Editor,
    reg: &crate::registry::Registry,
    keymap: &crate::keymap::KeyTrie,
    ex: &dyn crate::jobs::Executor,
    clock: &dyn wordcartel_core::history::Clock,
    msg_tx: &std::sync::mpsc::Sender<Msg>,
    id: crate::registry::CommandId,
    fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
) {
```

**Call sites: 4 total — 3 production, 1 test.**
- Production: `wordcartel/src/palette.rs`, inside `intercept` (the Enter-on-a-
  command-row arm); `wordcartel/src/menu.rs`, inside `dispatch_row_action`
  (function #5 below, `Command(id)` arm); `wordcartel/src/mouse.rs`, inside
  `mouse_palette` (Down(Left)-on-a-command-row arm).
- Test: `wordcartel/src/app.rs`, inside `pub fn menu_select_for_test` — a
  standalone `#[cfg(test)]`-annotated helper fn (not inside `mod tests`, but
  gated the same way).

**Constructs a bundle internally:** yes — `registry::Ctx` (not `DispatchCtx`):
```rust
let mut ctx = crate::registry::Ctx { editor, clock, executor: ex, msg_tx: msg_tx.clone(), fs: std::sync::Arc::clone(fs) };
```

**Parameter use:** `editor` used directly (`close_all(editor)`, moved into
`ctx`) and again in the drain loop and `hydrate_overlays`. `reg` used directly
(`reg.dispatch(id, &mut ctx)`) — note `reg` itself is NOT a `registry::Ctx`
field, it's the receiver. `keymap` used only once, in the trailing
`hydrate_overlays(editor, reg, keymap)` call. `ex` used twice: moved into
`ctx.executor` AND directly in `ex.drain()`. `clock`, `msg_tx`, `fs` each used
twice: once into `ctx`, once again directly in the drain loop's
`apply_job_outcome(o, editor, ex, clock, msg_tx, fs)`. `id` used once
(`reg.dispatch(id, ...)`).

---

### 5. `menu::dispatch_row_action`
File: `wordcartel/src/menu.rs`. Symbol: `dispatch_row_action`.

```rust
#[allow(clippy::too_many_arguments)] // C5 T5: +fs threads the seam through every dispatch site
pub(crate) fn dispatch_row_action(
    editor: &mut crate::editor::Editor,
    reg: &crate::registry::Registry,
    keymap: &crate::keymap::KeyTrie,
    ex: &dyn crate::jobs::Executor,
    clock: &dyn wordcartel_core::history::Clock,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>,
    action: MenuRowAction,
    fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
) {
```

**Call sites: 2 total — 2 production, 0 test.**
- `wordcartel/src/menu.rs`, inside `intercept` (keyboard Enter-on-a-menu-row arm).
- `wordcartel/src/mouse.rs`, inside `mouse_menu` (Down(Left)-on-a-dropdown-row arm).

**Constructs a bundle internally:** no.

**Parameter use:** `editor` and `action` are used directly (matched, and
`editor.menu = None` / `workspace::switch_to` in the `SwitchBuffer` arm).
`reg`, `keymap`, `ex`, `clock`, `msg_tx`, `fs` are used ONLY in the
`MenuRowAction::Command(id)` arm, where all six are forwarded verbatim,
unread, straight into `dispatch_overlay_command(editor, reg, keymap, ex,
clock, msg_tx, id, fs)`. In the `SwitchBuffer` arm none of the six are
touched at all. This is the purest forwarder of the seven.

---

### 6. `mouse::handle`
File: `wordcartel/src/mouse.rs`. Symbol: `handle`.

```rust
#[allow(clippy::too_many_lines)] // mouse event dispatch — one branch per screen region
#[allow(clippy::too_many_arguments)] // C5 T5: +fs threads the seam through every dispatch site
pub fn handle(
    editor: &mut Editor,
    ev: MouseEvent,
    reg: &crate::registry::Registry,
    keymap: &crate::keymap::KeyTrie,
    ex: &dyn crate::jobs::Executor,
    clock: &dyn wordcartel_core::history::Clock,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>,
    fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
) {
```

**Call sites: 4 total — 1 production, 3 test.**
- Production: `wordcartel/src/app.rs`, inside `reduce_dispatch`
  (`Msg::Input(Event::Mouse(ev))` arm): `crate::mouse::handle(editor, ev, reg, keymap, ex, clock, msg_tx, fs);`
- Test: `wordcartel/src/chrome.rs` inside its `mod tests`; `wordcartel/src/overlays.rs`
  inside its `mod tests` (2 sites: `splash_intercept_precedes_marks` uses
  `reduce`, but `no_dwell_arming_while_splash_is_up` and
  `click_under_overlay_does_not_move_caret` call `mouse::handle` directly).

**Constructs a bundle internally:** yes, but conditionally — only inside the
`if !no_overlay_open(editor)` branch:
```rust
let ctx = crate::overlays::DispatchCtx { reg, keymap, ex, clock, msg_tx, fs };
route_overlay(editor, ev, area, &ctx);
return;
```

**Parameter use — the sharpest split of the seven:** `editor`, `ev`, `clock`
are used throughout the whole function (dwell timers, click/drag/scroll
gesture handling, all in the `no_overlay_open(editor)` branch below the early
return). `reg`, `keymap`, `ex`, `msg_tx`, `fs` are read EXACTLY ONCE each, all
on the single `DispatchCtx { reg, keymap, ex, clock, msg_tx, fs }` line — a
grep of the function body below that line for `reg\b|keymap\b|\bex\b|msg_tx|\bfs\b`
returns nothing. When an overlay is open, all five are packaged into `ctx` and
handed to `route_overlay`; when no overlay is open (the dwell/gesture path,
which is most of the function's ~220 lines), none of the five are touched
again.

---

### 7. `plugin::pump::drain_one_dispatch`
File: `wordcartel/src/plugin/pump.rs`. Symbol: `PluginHost::drain_one_dispatch`
(private inherent method, `impl PluginHost { ... }`, not a trait impl).

```rust
#[allow(clippy::too_many_arguments)] // C5 T5: +fs threads the seam through every dispatch site
fn drain_one_dispatch(
    &self,
    editor: &Rc<RefCell<Editor>>,
    reg: &crate::registry::Registry,
    ex: &dyn crate::jobs::Executor,
    clock: &Clock,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>,
    d: &crate::plugin::PluginDispatch,
    fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
) {
```
(`clock: &dyn Clock` — `Clock` is `wordcartel_core::history::Clock`, imported by name in this file.)

**Call sites: 1 total — 1 production, 0 test.**
- `wordcartel/src/plugin/pump.rs`, inside `PluginHost::pump`'s dispatch-drain
  loop (`for d in dispatches { ... self.drain_one_dispatch(editor, reg, ex, clock, msg_tx, &d, fs); }`).

**Constructs a bundle internally:** yes — `registry::Ctx` (not `DispatchCtx`;
note `editor` is dereffed through a short `borrow_mut()`, unlike the other six
which take `&mut Editor` directly):
```rust
let mut e = editor.borrow_mut();
let mut ctx = crate::registry::Ctx { editor: &mut e, clock, executor: ex, msg_tx: msg_tx.clone(),
    fs: std::sync::Arc::clone(fs) };
reg.dispatch_with_arg(id, &mut ctx, d.arg.clone());
```

**Parameter use:** `editor` used to `.borrow_mut()` and (on the unknown-name
early-return) passed to `plugin_error`. `reg` used TWICE directly:
`reg.resolve_name(&d.name)` and `reg.dispatch_with_arg(id, &mut ctx, ...)` —
`reg` itself is not a `Ctx` field. `ex`, `clock`, `msg_tx`, `fs` are each used
once, moved/cloned into `ctx`. `d` used three ways (`d.name`, `d.origin`,
`d.arg.clone()`).

---

## 5. `DispatchCtx` definition
File: `wordcartel/src/overlays.rs`. Symbol: `DispatchCtx<'a>`.

```rust
/// The non-editor dispatch context, bundled so every overlay `intercept` (and later `mouse`)
/// fn shares ONE signature. The editor is passed SEPARATELY as `&mut Editor` — deliberately
/// EXCLUDED here to avoid a `&mut` aliasing tangle in the table loop (contrast
/// `registry::Ctx`, which OWNS `editor: &mut Editor` and holds `msg_tx` by VALUE for a
/// `'static` spawned thread; `DispatchCtx` borrows `msg_tx` — it never outlives the loop).
pub(crate) struct DispatchCtx<'a> {
    pub(crate) reg: &'a crate::registry::Registry,
    pub(crate) keymap: &'a crate::keymap::KeyTrie,
    pub(crate) ex: &'a dyn crate::jobs::Executor,
    pub(crate) clock: &'a dyn wordcartel_core::history::Clock,
    pub(crate) msg_tx: &'a std::sync::mpsc::Sender<Msg>,
    /// The filesystem seam (owned handle — the listing thread clones it in).
    pub(crate) fs: &'a std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
}
```

- Single lifetime `'a` shared by all six fields.
- `&mut Editor` is **deliberately excluded** as a field, not merely omitted by
  oversight. The doc comment on the struct states the reason explicitly:
  "The editor is passed SEPARATELY as `&mut Editor` — deliberately EXCLUDED
  here to avoid a `&mut` aliasing tangle in the table loop" — i.e. `reduce_dispatch`'s
  overlay-intercept loop calls `(id.row().intercept)(msg, editor, &ctx)` on
  every row in `OverlayId::ALL`, re-borrowing `editor` as `&mut` fresh each
  iteration; if `editor` were also inside `ctx` (itself borrowed `&ctx` across
  the loop), the loop could not simultaneously hold `&mut editor` and `&ctx`
  containing another borrow of `editor`.
- The same comment explicitly contrasts `DispatchCtx` with `registry::Ctx`
  (`wordcartel/src/registry.rs`), which DOES own `editor: &mut Editor` as a
  field, and holds `msg_tx: std::sync::mpsc::Sender<Msg>` **by value** (not a
  reference) because `dispatch_filter` moves a clone of it into a `'static`
  spawned thread. `DispatchCtx.msg_tx` is a borrow (`&'a Sender<Msg>`)
  because, per the comment, "it never outlives the loop."
- `fs`'s own doc comment: "The filesystem seam (owned handle — the listing
  thread clones it in)" — `DispatchCtx.fs` is `&'a Arc<...>` (a borrow of an
  owned handle), mirroring `registry::Ctx.fs: Arc<...>` (owned) one level up.

---

## 6. Every other place `DispatchCtx` is constructed or consumed

**Constructed** (`DispatchCtx { ... }` literal), beyond `reduce_dispatch` and
`mouse::handle` above:
- `wordcartel/src/splash.rs` (test), `wordcartel/src/prompts.rs` (2 sites, test),
  `wordcartel/src/minibuffer.rs` (2 sites, test), `wordcartel/src/menu.rs` (test),
  `wordcartel/src/overlays.rs` (2 sites, test), `wordcartel/src/plugin/host.rs`
  (test) — all inside `mod tests` blocks, building a throwaway `ctx` to call an
  `intercept`/`mouse` fn directly.
- `wordcartel/src/file_browser_listing.rs` (1 site) and
  `wordcartel/src/file_browser_commit.rs` (5 sites) — need per-site check for
  test-gating, all found inside functions in files whose only `DispatchCtx`
  users are test helpers/`#[test]` fns per the surrounding code shape (same
  pattern as above: build `ctx`, call an intercept/commit helper).
- `wordcartel/src/test_support.rs`, inside `press_key_fb` (a `pub(crate)` test
  helper, not under `#[cfg(test)]` itself but only reachable from tests) —
  builds `ctx` and calls `file_browser_intercept::intercept` directly.

**Consumed** (fn parameter `ctx: &crate::overlays::DispatchCtx` / `_ctx: &...`):
every per-overlay `intercept` fn (`splash::intercept` [`_ctx`, unused],
`palette::intercept`, `menu::intercept`, `theme_picker::intercept`,
`cursor_picker::intercept`, `file_browser_intercept::intercept`,
`prompts::intercept`, `minibuffer::intercept`, `search_ui::intercept`,
`diag_overlay::intercept`, `outline_overlay::intercept`, `marks::intercept`)
and every per-overlay `mouse` fn in `wordcartel/src/mouse.rs` (`mouse_palette`,
`mouse_menu`, `mouse_theme_picker` [`_ctx`, unused], `mouse_cursor_picker`
[`_ctx`, unused], `mouse_file_browser`, `mouse_outline` [`_ctx`, unused],
`mouse_diag`, `mouse_prompt`, `mouse_minibuffer` [`_ctx`, unused],
`mouse_search` [`_ctx`, unused]) and `mouse::route_overlay`. These are exactly
the fn-pointer table entries in `overlays.rs`'s `OverlayRow`/`OVERLAYS` (see §8).

---

## 7. Borrow / aliasing structure

- **`DispatchCtx` vs `&mut Editor` is the central split-borrow pattern.** In
  `reduce_dispatch`, `let ctx = DispatchCtx { reg, keymap, ex, clock, msg_tx, fs };`
  is built once, then the overlay-intercept loop does
  `(id.row().intercept)(msg, editor, &ctx)` for each row, re-taking `&mut
  editor` fresh every call while `&ctx` (an immutable borrow of `reg`/`keymap`/
  `ex`/`clock`/`msg_tx`/`fs`) stays alive across the whole loop. This is
  exactly what the struct's own doc comment (quoted in §5) says the
  `editor`-exclusion is FOR.
- Same pattern in `mouse::handle`: `let ctx = DispatchCtx { reg, keymap, ex,
  clock, msg_tx, fs }; route_overlay(editor, ev, area, &ctx);` — `editor` is
  passed as a separate `&mut Editor` argument alongside `&ctx`.
- `registry::Ctx` (`wordcartel/src/registry.rs`) is the aliasing-relevant
  sibling bundle: it OWNS `editor: &mut Editor` as a field (so it can't be
  built while any other borrow of `editor` is outstanding), owns `msg_tx` BY
  VALUE (`Sender<Msg>`, not `&Sender<Msg>`) with the comment "because
  `dispatch_filter` moves a clone into a `'static` spawned thread," and owns
  `fs: Arc<...>` (not `&Arc<...>`) with the comment "because `jobs::Job::run`
  is `Box<dyn FnOnce() -> JobResult + Send>` — a job closure must be able to
  clone this in." Four of the seven functions (`handle_key`,
  `dispatch_overlay_command`, `drain_one_dispatch`, and transitively via
  `dispatch_overlay_command`, `dispatch_row_action`) build a `registry::Ctx`
  from their loose `editor`/`clock`/`ex`/`msg_tx`/`fs` params specifically so
  `reg.dispatch(id, &mut ctx)` can hand a job-spawning builtin an owned,
  `'static`-clonable `msg_tx`/`fs` — collapsing these to hold a borrowed
  `DispatchCtx` instead would not by itself remove this second, differently-
  shaped bundle (`registry::Ctx`) that three of the seven still need to build.
- `drain_one_dispatch` additionally takes `editor: &Rc<RefCell<Editor>>`
  (not `&mut Editor` — plugin dispatch runs the editor through a `RefCell`)
  and does `let mut e = editor.borrow_mut(); let mut ctx = registry::Ctx {
  editor: &mut e, ... };` — a short-lived interior-mutable borrow, distinct
  from the other six functions' direct `&mut Editor` parameter.
- `wordcartel/src/fsx.rs:100-101` doc comment, about why `Fs` trait is `pub`:
  "Task 5 threads `dyn Fs` into `Ctx`/`DispatchCtx` and several `pub`
  functions (`app::reduce`, `mouse::handle`,
  `plugin::pump::PluginHost::pump`, …), exactly like ..." — names `reduce`
  and `mouse::handle` by name as part of the seam this `#[allow]` marks.

---

## 8. Trait impls / fn pointers / closures / thread boundaries

- **None of the seven functions is itself stored as a fn pointer, a trait
  method, or a closure field.** A grep for each name used as a bare value
  (not immediately followed by `(`) turns up only doc-comment/prose mentions,
  never an assignment, struct-literal field, or `Fn`-bound argument.
- **`drain_one_dispatch` is a private inherent method** on `PluginHost`
  (`impl PluginHost { ... }`), not a trait impl.
- **Two of the seven are called FROM inside functions that ARE stored as fn
  pointers**, which constrains their reachability more indirectly:
  `overlays.rs`'s `OverlayRow` struct has `intercept: fn(Msg, &mut Editor, &DispatchCtx) -> Handled`
  and `mouse: fn(&mut Editor, MouseEvent, Rect, &DispatchCtx)` fields, populated
  in the static table `OVERLAYS: &[OverlayRow]` with entries like
  `intercept: crate::menu::intercept, ... mouse: crate::mouse::mouse_menu` and
  `intercept: crate::palette::intercept, ... mouse: crate::mouse::mouse_palette`.
  - `dispatch_row_action`'s two call sites are inside `menu::intercept`
    (table's `intercept` fn pointer for the `Menu` row) and `mouse::mouse_menu`
    (table's `mouse` fn pointer for the `Menu` row).
  - `dispatch_overlay_command`'s call sites at `palette.rs`/`mouse.rs` are
    likewise inside `palette::intercept` and `mouse::mouse_palette`, both
    table fn pointers; its third production call site is inside
    `dispatch_row_action` itself (one level further from the table).
  - `reduce_dispatch` itself drives the table: `(id.row().intercept)(msg,
    editor, &ctx)` for each `OverlayId` in `ALL[1..]`, plus a direct,
    non-table call to `crate::marks::intercept`. This is the mechanism by
    which `handle_key` and `mouse::handle` (called directly, NOT via the
    table) end up racing/interleaving with the table-driven overlay
    interceptors inside the same function.
- **No closures capture any of the seven as values**; the `OVERLAYS` table's
  `is_active`/`close` fields ARE non-capturing closures (`|e| e.splash.is_some()`
  etc.) coerced to fn pointers, but none of the seven functions fill those
  particular fields.
- **No thread boundary directly touches the seven's own signatures.** The
  thread-crossing concern in this code lives one level down, in
  `registry::Ctx` (owned `msg_tx: Sender<Msg>`, owned `fs: Arc<...>`,
  documented as needed because `dispatch_filter` spawns a `'static` thread and
  `Job::run` closures must clone `fs` in) — three of the seven
  (`handle_key`, `dispatch_overlay_command`, `drain_one_dispatch`) construct
  exactly this owned `registry::Ctx` from their own borrowed params before
  calling `reg.dispatch(...)`, which is how a borrowed `&Sender`/`&Arc<dyn Fs>`
  in one of the seven's signatures ends up cloned into a spawned thread two
  calls downstream.
- `file_browser.rs`'s doc comment (near `start_listing`, not one of the seven)
  independently confirms an `Fs` handle is cloned across a listing thread —
  consistent with the `DispatchCtx.fs` field comment "the listing thread
  clones it in" — but no such handle crosses into the *dispatch* path from
  inside the seven functions themselves.
