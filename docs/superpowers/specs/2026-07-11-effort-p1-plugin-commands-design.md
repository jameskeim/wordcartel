# Effort P1 — Plugin commands (in-process Lua): commands + reads + validated edits + isolation

**Status:** SPEC (2026-07-11). Effort **P** (the in-process Lua plugin system — the 1.0 capstone),
**Phase 1 of 3**. The 3-phase decomposition and the main-thread-pump host-access model were adopted
from the independent architecture proposal (grounding: `docs/design/effort-p-grounding.md`); the
earlier P0→P3 brainstorm and thin-P0 spec are **retired**. P2 (events/config/reload) and P3
(async/timers/parameterized commands) are out of scope here — see the explicit NOT-in-P1 list.

Binding constraint sources (authoritative, unchanged): `CLAUDE.md` (project law) and
`docs/design/command-surface-contract.md`. Real code surface verified against the live tree
2026-07-11 (`registry.rs`, `transact.rs`, `panicx.rs`, `jobs.rs`, `timers.rs`, `keymap.rs`, `app.rs`).

---

## 1. Goal & scope

**Goal.** Ship the *minimum lovable* plugin system: a user drops a Lua file into their config's
`plugins/` directory; it registers editor commands that appear in the command palette, optionally in
a menu category, and are bindable through the existing keymap-patch mechanism — with full panic /
runaway / error isolation so a buggy plugin degrades to a status-line message and never hangs typing
or crashes the editor. A plugin command can **read** editor state and **edit** the buffer, but only
through the already-proven validated write boundary (`submit_transaction`).

This is the smallest shippable unit that retires the bulk of Effort P's risk: it drives one complete
vertical slice through the crux (VM embed, loader, opened registry, the pump, isolation), so P2/P3
build breadth on a proven spine.

**Success demo.** An `insert_date.lua` in the plugins dir registers `"date.insert"` → "Insert Date";
it appears in the palette, is bindable via `keymap.patches`, and inserting at the caret goes through
`submit_transaction` (valid-by-construction, zero-mutation-on-error).

### In scope (P1)
- One embedded `mlua` VM (PUC Lua 5.4, vendored), main-thread-confined, one per process.
- A loader: single-file `<name>.lua` and directory `<name>/init.lua`, eager lexicographic load at
  startup, with a filesystem-free `load_sources` core.
- The `wc.*` registration + editor API: `register_command`, `status`, buffer/selection **reads**, and
  validated **edits** (`insert`/`replace`/`set_selection`) — plugin offsets pre-validated against the
  live buffer (§3b), then routed through `submit_transaction`. (No `wc.command` in P1 — deferred to P2.)
- The opened command registry: plugin `CommandId→Plugin` entries in the *same* registry the palette
  and menu derive from; `<plugin>.<command>` namespacing; optional `MenuCategory`.
- The **pump**: the single post-`reduce` pipeline stage that is the *only* place Lua ever runs, with
  no editor borrow held.
- Isolation: `panicx` at every host→Lua entry, a `set_hook` runaway-time abort, a (spike-gated)
  memory cap, and the `plugin_error` seam → status line.
- `--no-plugins` safe-mode flag; a `[plugins]` config section (enable/disable list only in P1).
- Tests: pump-as-InlineExecutor unit tests, string-source loader tests, the loaded-but-idle guardrail,
  and the command-surface-contract invariants extended over plugin entries.

### NOT in P1 (deferred to P2/P3) — explicit
- **`wc.command(id)`** (a plugin dispatching another command) → **P2**. It needs the full dispatch
  context (`Ctx` + `&Registry`) the pump doesn't carry, plus the enqueue-route re-drain loop and chain
  cap; it lands with events, which need the same context. **Scope trim vs. the original proposal — flag
  at spec review.** (P1 plugins register/read/edit/status without it.)
- **Events / hooks** (`on_save`/`on_open`/`on_buffer_close`, any pub/sub) → **P2**.
- **Per-plugin config tables** (`[plugins.<name>]` handed to the plugin) → **P2**. P1's `[plugins]`
  is host-level enable/disable only.
- **Reload** (`plugins_reload`, VM teardown/rebuild) and `plugin_list` command → **P2**.
- **Plugin timers / periodic work** (the `timers.rs` `SUBSYSTEMS` → `Vec` upgrade) → **P3**.
- **Plugin async** (`spawn_process`-style; host-side slow work on the job substrate) → **P3**.
- **Parameterized commands** (the contract's rule-10 set-value collapse) → **P3**.
- **Plugin-contributed dynamic menu *sections*** (the `DYNAMIC_SECTIONS` second consumer) → **P3**.
  P1 plugins may only tag an existing `MenuCategory`; no new top-level menus.
- **Per-plugin side-effect tracking / compensating teardown** (Fresh's model) — unneeded until reload
  (P2), and P2 uses whole-VM rebuild, not per-effect bookkeeping.
- **Sandboxing / capability restriction.** P1 is trusted-user-installed code (§7 trust posture).
- **Hot-path hooks** (`on_key`, synchronous per-keystroke Lua) — architecturally excluded, not merely
  deferred: plugin code never runs inside `reduce` (§3c).

---

## 2. Architecture & components

A new module family under `wordcartel/src/plugin/`. Nothing plugin-specific leaks into `wordcartel-core`
(which stays VM-free and `#![forbid(unsafe_code)]`). The shell's `#![forbid(unsafe_code)]` also holds —
`Rc<RefCell<>>` + owned captures are safe Rust; `mlua`'s `unsafe` stays inside the dependency.

- **`plugin/host.rs` — `PluginHost`.** Owns the one `mlua::Lua` VM and the `Bridge` (the
  `Rc<RefCell<Editor>>` handle + a `Sender<Msg>` clone for status). The `PluginCall` **queue lives on
  `Editor`** (`Editor.pending_plugin_calls: VecDeque<PluginCall>` — §3c), because both the registry
  dispatch arm and the pump reach `Editor` but neither reaches the host. Exposes
  `PluginHost::new(bridge) -> Result<PluginHost, HostError>` and — **critically** —
  `pump(&mut self, editor: &Rc<RefCell<Editor>>)`: the pump takes the **handle**, NOT `&mut Editor`, so
  it holds no `RefMut` across Lua (§3c). Carries a `wordcartel/tests/module_budgets.rs` production-line
  budget from day one.
- **`plugin/api.rs` — the `wc` table.** Builds the global `wc` Lua table WezTerm-style: a **flat
  registration-seam vector** of `fn(&Lua, &Bridge) -> mlua::Result<()>` entries, each installing one
  API area's `create_function`s. Adding an API area is one function + one row — never editing a
  dispatcher (the project's own anti-regrowth rule, which WezTerm independently arrived at).
- **`plugin/load.rs` — the loader.** `load_sources(host, &[(name, src)]) -> Vec<LoadReport>` is the
  filesystem-free testable core (real Lua, string sources). A thin shell-side `discover(dir) ->
  Vec<(String, String)>` does the `read_dir` + bounded read and feeds the core (§6).
- **`Rc<RefCell<Editor>>` confined to `app::run` — multiple short borrow scopes per iteration.** Today
  `run` owns `let mut editor` (`app.rs:471`) and every loop stage takes editor in sequence:
  `timers::next_wake(&editor, now)` (~`app.rs:740`, an **immutable `borrow()`**), `timers::pre_recv`
  (~`736`), `reduce` (~`754`), `rebuild_keymap_if_requested`/`rederive_theme_if_requested`
  (~`755`/`760`), the settings-save arm (~`761`), `surface_undo_eviction` (~`770`),
  `drain_clipboard_intents` (~`771`), `reconcile_mouse_capture` (~`772`), `advance` (~`773`),
  `render::render` (~`774`, and `render` itself takes `&mut Editor` — `render.rs:216`), and the
  post-render session-persist check + use (~`776`–`779`: reads `saved_version`, then borrows for
  `persist_session`). P1 wraps the loop-local editor as
  `let editor = Rc::new(RefCell::new(editor_val));` and **each stage above takes its own short
  `editor.borrow()`/`borrow_mut()` scope that drops immediately** — i.e. many short borrows per
  iteration, NOT one long borrow spanning the loop body. Every existing `stage(&mut editor, …)` call
  becomes `stage(&mut editor.borrow_mut(), …)` (or an explicit
  `{ let mut e = editor.borrow_mut(); stage(&mut e, …); }` scope where the borrow must end before the
  next stage; `next_wake` uses `&editor.borrow()`). `reduce` and every helper keep their `&mut Editor`
  signatures unchanged — the `Rc<RefCell>` is a `run()`-local detail, invisible to the rest of the shell
  and to core.
- **The pump runs in its OWN stage holding NO outer borrow — it takes the handle, not `&mut Editor`.**
  Inserted between `reduce` and the keymap/theme arms, it is called as `host.pump(&editor)` (the
  `Rc<RefCell<Editor>>` handle by reference) — deliberately NOT `host.pump(&mut editor.borrow_mut())`,
  which would hold a `RefMut` across the whole pump body and make every `wc.*` re-borrow hit "editor
  busy." Internally the pump drains the queue under a short `borrow_mut` that drops, then runs callbacks
  with nothing held (§3c), so each API closure's per-call `try_borrow_mut` (§3a) succeeds. The
  post-`reduce` sequence is therefore:
  **`reduce` (borrow scope A) → `plugin pump` (takes the handle; no outer borrow; each API call and the
  internal drain take+drop their own borrow) → keymap/theme/settings arms (borrow scope B…) → `advance`
  → `render` (borrow scope N) → session-persist.** The pump runs before `advance`/`render`, so plugin
  effects land in the *same frame* (no Fresh-style one-frame lag).
- **`PluginHost` is `Option` in the loop / null when absent.** `--no-plugins`, a load-time VM failure,
  or the no-plugins-dir case leaves `plugin_host: Option<PluginHost> = None`; the pump stage is a
  cheap `if let Some(h) = …` (mirrors the `NullProvider` / boxed-`DiagnosticsProvider` null-object
  discipline from Effort A). e2e journeys that don't exercise plugins construct no host.
- **Implementation ordering note — the `Sender<Msg>` must be created before plugin load.** The plugin
  load phase is planned between `Registry::builtins()` (`app.rs:620`) and `build_keymap` (`app.rs:622`),
  but the `let (msg_tx, msg_rx) = channel()` currently lives *later* (`app.rs:~631`). If the `Bridge`
  needs a `Sender<Msg>` clone (for `wc.status` / future async), the plan **must move channel creation
  earlier in `run()`** — ahead of the plugin-load phase — so the bridge can capture `msg_tx.clone()` at
  host construction. A mechanical reorder (the channel has no dependency on the intervening lines); the
  plan calls it out as a required migration step.

---

## 3. Host-access mechanism (the crux)

Main-thread-confined VM + a single deferred invocation point (the pump) + a live editor handle with
short per-API-call borrows. This is WezTerm's proven posture (never touch the exclusively-owned struct
from inside a Lua call stack; interior mutability re-fetched per call; `lua.scope` used zero times),
adapted to a single-threaded loop — which lets P1 *improve* on WezTerm's deferred notification queue:
because the safe point is on the same thread in the same frame, API calls borrow the live editor
directly instead of round-tripping oneshot channels.

### Design law — offset/range pre-validation (cross-cutting; binding on every phase)
The core edit/text primitives are **trusted-caller** APIs that *assert* on bad input rather than
returning errors: `ChangeSet::from_ops` release-asserts its consumption sum (`change.rs:127`),
`ChangeSet::apply` asserts on apply (`change.rs:94`), `ChangeSet::insert`/`delete` assume in-range
boundary offsets, and `TextBuffer::slice` (`buffer.rs:125`) char-boundary-checks then hands the range to
`rope.byte_slice` as-given — and `clamp_to_boundary` only snaps a *single* offset, it does **not**
enforce `a <= b` or in-bounds (`buffer.rs:48–53`, `:125–136`). A raw plugin-supplied offset reaching any
of them is a panic, not a degrade — the class behind the round-3 (edits) and round-4 (`wc.text`)
Criticals. Therefore:

> **LAW.** Every plugin API that accepts a byte offset or range MUST pre-validate it against the **live
> buffer** — in-bounds (`to <= len`), ordered (`from <= to`), and both endpoints on char boundaries
> (`clamp_to_boundary(p) == p`) — via the shared `plugin_check_range` helper (§3b), and return a **typed
> Lua error (degrade)** on failure. **No raw plugin-supplied offset ever reaches an asserting core
> primitive** (`ChangeSet::from_ops`/`apply`/`insert`/`delete`, `TextBuffer::slice`).

**P1 `wc.*` surface audit against the LAW** (every input-taking API is covered, so future phases inherit
the discipline): `wc.text(a, b)` — range → **guarded** (§3a); `wc.replace(a, b, text)` — range →
**guarded** (§3b); `wc.insert(text)` — single offset (live cursor) → **guarded** (§3b); `wc.set_selection`
— snaps via `submit_transaction`, never asserts → **safe by routing**; `wc.selection`/`wc.cursor`/
`wc.len`/`wc.version`/`wc.path` — take **no** offset input → **safe** (nothing to validate);
`wc.status`/`wc.register_command` — no offsets → **safe**. `plugin_check_range` is the one chokepoint.

### a. Reads — live, synchronous, per-call borrow (offset inputs pre-validated — the LAW)
Each read API function does a short `editor.try_borrow_mut()` (or `try_borrow`) for the duration of that
one call and returns owned data to Lua. Reads are O(requested), never O(document):
- `wc.text(a?, b?)` → the buffer substring (whole buffer if both omitted). When a range is given it is
  **pre-validated via `plugin_check_range` against the live buffer** (`a <= b`, `b <= len`, both on char
  boundaries) — bad input → a **typed Lua error**, nothing sliced — BEFORE calling `TextBuffer::slice`.
  This closes the same panic class as the edit fold: `slice` → `rope.byte_slice(range)` panics on a
  reversed or out-of-bounds range, and `clamp_to_boundary` clamps only one endpoint, so the range check
  is on us, not the primitive. A missing endpoint defaults to `0` / `len` (both valid boundaries).
- `wc.selection()` → `{anchor, head}` byte offsets of the primary selection.
- `wc.cursor()` → the primary `head` byte offset.
- `wc.len()` → buffer byte length; `wc.version()` → the document version; `wc.path()` → the active
  buffer's path string or `nil`.

Live reads mean read-after-write inside one callback behaves naturally (insert, then ask the cursor —
it reflects the insert), which a snapshot model cannot do.

### b. Edits — ONLY via `submit_transaction`, and NEVER hand raw plugin offsets to a trusting constructor
There is no raw-state API by construction — the bridge simply never exposes one. Edit functions build a
`ChangeSet` against the *live* buffer length and submit through the proven boundary
(`transact::submit_transaction(editor, txn, clock) -> Result<(), EditError>`).

**Pre-validation is on us — the construction step panics on garbage before the boundary ever runs.**
`ChangeSet::from_ops` (`wordcartel-core/src/change.rs:118`) is a **trusted-caller** constructor that
**release-asserts** `retain + delete == len_before` (`change.rs:127`); `ChangeSet::apply` release-asserts
`buf.len() == len_before` (`change.rs:95`); and the char-boundary asserts fire in `TextBuffer::insert`/
`delete` (`buffer.rs:72`/`:95`) on mid-char offsets. `submit_transaction`'s `validate_against` catches a
STALE *length*, but a plugin passing out-of-bounds, reversed (`from > to`), or non-char-boundary offsets
would **panic during construction**, before validation — defeating degrade-don't-crash / no-data-loss.
So the plugin edit API MUST pre-validate the plugin-provided offsets against the **live buffer** and
return a **typed Lua error** (degrade, plugin may `pcall`) BEFORE constructing any `ChangeSet`. The
single shared chokepoint — used by BOTH edits and the `wc.text` read (§3a) — is
`plugin_check_range(buf, from, to) -> Result<(), PluginRangeError>`, which enforces, in order:
`from <= to`; `to <= buf.len()`; `from` and `to` each on a char boundary
(`buf.clamp_to_boundary(p) == p`). Only offsets that pass reach a core primitive (this is the §3 LAW).
A single-offset API (`wc.insert`) checks `plugin_check_range(buf, off, off)`.

- `wc.insert(text)` → pre-checks `cursor` is a valid boundary (it always is — it comes from the live
  selection — but the check is uniform), then `ChangeSet::insert(cursor, text, len)`
  (`wordcartel-core/src/change.rs:63`) at the primary head.
- `wc.replace(a, b, text)` → **pre-validate `(a, b)` via `plugin_check_range`** (bad input → typed Lua
  error, nothing constructed), THEN build the `ChangeSet` via the **existing public**
  `build_range_replace(from, to, text, doc_len)` (`commands.rs:185` — already `pub`, already used by the
  filter merge; it wraps the private `replace_changeset` + the matching `Edit`, so no visibility change
  and no logic duplication). Because the range was pre-checked against the live buffer, the underlying
  `ChangeSet::from_ops` consumption assert cannot trip. `submit_transaction` then re-validates length (a
  concurrent-edit `StaleLength` → `Err`, zero mutation) — the pre-check and the boundary are
  complementary: pre-check guards *construction*, `validate_against` guards *staleness*.
- `wc.set_selection(anchor, head)` → routes through the same selection-snapping `submit_transaction`
  applies (out-of-bounds snaps, never rejects — the existing behavior; selection needs no pre-check).

`submit_transaction` is already proptest-hardened (2048 hostile cases: never-panics / on-Err-zero-
mutation / on-Ok-in-bounds-and-char-boundary). The plugin edit boundary **inherits that guarantee for
free** — it is the exact seam whose own header names itself "Effort P's apply(Transaction) seam." On
`Err`, the edit function raises a Lua error carrying the `EditError` (the plugin may `pcall` it);
uncaught, it surfaces via `plugin_error` → status line. Zero mutation on error is guaranteed by
`submit_transaction`, not by plugin discipline.

### c. The `PluginCall` queue + same-frame drain — plugin code never runs inside `reduce`
**Enqueue transport (Codex-flagged gap — specified concretely).** `Ctx` carries only
`editor/clock/executor/msg_tx` (`registry.rs:26`) — no host, no queue — so the `Plugin` dispatch arm has
nowhere to reach a host-owned queue, and both dispatch and the pump must reach the same place. The one
thing both already touch is the **`Editor`**. So the queue lives on `Editor`:

- A new field `Editor.pending_plugin_calls: VecDeque<PluginCall>` (`PluginCall { id: CommandId }` — a
  `Copy` id; `VecDeque` for FIFO). Trivial budget/footprint impact — one field, `Default`-empty, no new
  `Msg` variant, no signature change to `reduce`/`Ctx`.
- The registry's `HandlerKind::Plugin` dispatch arm (§4) pushes `PluginCall { id }` onto
  `ctx.editor.pending_plugin_calls` and returns `CommandResult::Handled` — it does **not** call Lua and
  imports no `mlua` type.
- `PluginHost::pump(&mut self, editor: &Rc<RefCell<Editor>>)` takes the **handle**, not `&mut Editor`,
  so it never holds a borrow across Lua. It is a **single drain-then-invoke pass** (P1 has no
  `wc.command` — §5/§1 — so no callback can grow the queue mid-pump; a re-drain loop and a chain cap are
  P2 concerns, deferred with the machinery that would need them):
  - **Phase A — drain (short `borrow_mut` scope, drops immediately).** `{ let mut e =
    editor.borrow_mut(); std::mem::take(&mut e.pending_plugin_calls) }` pops the queued `PluginCall`s
    into a local `Vec`; the `RefMut` is released before Phase B.
  - **Phase B — invoke (NO outer borrow held).** For each drained `PluginCall`, look up the plugin's
    stored Lua callback (a value in Lua's named registry, keyed by the command id string — WezTerm's
    persistent-callback pattern) and invoke it inside `panicx::catch` + the `set_hook` time guard (§7).
    Each `wc.*`/editor API closure re-borrows via its **own captured `Rc<RefCell<Editor>>` clone** with a
    short `try_borrow_mut` (§3a) — which succeeds precisely because Phase A's borrow is gone and the pump
    holds nothing. A single callback's own runaway is bounded by the `set_hook` time guard (§7) — the
    real per-callback hang protection, independent of any chain cap.

Because the pump runs before `advance`/`draw`, effects are visible the same frame. Because it runs with
no editor borrow held, the API closures' `try_borrow_mut` always succeeds in the normal path.

### d. Re-entrancy → "editor busy" (degrade, not panic) — the defensive backstop, NOT the normal path
Every API closure uses `try_borrow_mut`, never `borrow_mut`. In the normal pump path this **always
succeeds**: the pump holds no borrow (§3c takes the handle, not `&mut Editor`), and each API call takes
and releases its own short borrow, so consecutive `wc.*` calls in one callback never overlap. The
"editor busy" degrade is therefore reserved for **genuine nested re-entry** — a callback that somehow
triggers a *second* live borrow of the same `RefCell` before its first has dropped (e.g. a future API
that re-enters Lua mid-borrow). In that case `try_borrow_mut` returns `Err` and the API function raises a
Lua error `"editor busy"` → status line — a graceful degrade, never a `RefCell` double-borrow panic. It
is a defensive invariant, tested directly (§8), not something the normal path exercises.

---

## 4. Command & registration seam

Open the *existing* `Registry` (`registry.rs`) — do not build a parallel one. The command-surface
contract's laws hold **by derivation** only if plugin commands live in the same table `palette.rs` and
`menu.rs` already iterate via `reg.commands()`.

Two concrete obstacles in the real code and their minimal resolutions:

- **`Handler = fn(&mut Ctx) -> CommandResult` is a bare fn pointer** (`registry.rs:34`) — it cannot
  carry a plugin closure. Resolution: `CommandEntry.handler` becomes
  `enum HandlerKind { Builtin(Handler), Plugin }`. Dispatching a `Builtin` is exactly today's call;
  dispatching a `Plugin` pushes `PluginCall { id }` onto `ctx.editor.pending_plugin_calls` (§3c — the
  queue lives on `Editor`, the one place both dispatch and the pump reach) and returns
  `CommandResult::Handled`. **No `mlua` type enters `registry.rs`** — the arm only pushes a `Copy`
  `CommandId`; `registry.rs` stays Lua-free.
- **Both `CommandId(pub &'static str)` AND `CommandMeta.label` are `&'static str`** (`registry.rs:16`,
  `:52`), and every consumer relies on it (`KeyAction::Id(CommandId)`, palette, menu, hints,
  `resolve_name`). Plugin names AND labels are runtime `String`s from Lua. Resolution: **intern both the
  namespaced name and the label** to `&'static str` once at registration (leak-once via a small global
  interner; process-lifetime). This keeps `CommandId` `Copy` and `CommandMeta` `Copy`-of-`&'static`
  entirely — the registry metadata never holds an owned `String`, so every existing consumer is
  untouched. **The leak makes registration a permanent allocation — hence the hard caps below (§5/§7):**
  interned strings are NOT reclaimed and Lua's `set_memory_limit` does not bound them, so registration
  must be count- and length-capped to keep the bounded-memory invariant. **The caps are enforced at the
  `api.rs` load layer on the RAW Lua-supplied `String`s, BEFORE interning** (§5/§7) — never inside
  `register_plugin`, which by then has already received `&'static` (too late to reject an oversized
  leak). Both the plugin **stem** and the plugin-local **name** are length-checked, and the total
  namespaced id length is bounded, so the *full* leaked `<plugin>.<command>` id is capped, not just its
  `name` segment.

New public registry surface:
- `Registry::register_plugin(&mut self, name: &'static str, label: &'static str, menu:
  Option<MenuCategory>) -> Result<(), RegisterError>` — receives already-interned `&'static` inputs, so
  its only failure is a **collision** (`RegisterError::Duplicate` — the id already names a builtin or an
  earlier plugin command) → surfaced via `plugin_error`. All length/count caps are checked upstream in
  `api.rs` on the raw `String`s; a rejected registration never reaches `register_plugin` and never
  interns.
- Names are **namespaced `<plugin>.<command>`**, enforced at registration (the `<plugin>` segment is the
  file/dir stem). Deterministic, collision-resistant, self-documenting in the palette.
- Registration happens only at **plugin load time** (startup in P1), never mid-`reduce`. The registry is
  immutable while dispatch is live — the same between-reduces discipline the keymap swap already uses
  (`keymap_rebuild`). `run()`'s `let reg` (`app.rs:620`) becomes `let mut reg` for the load window only;
  `reduce` keeps `&Registry`.

**Free keybindings — no new binding code.** `build_keymap(km, reg)` resolves every `keymap.patches`
chord through `reg.resolve_name(id_str)` (`keymap.rs:508`, `:561`) and drops unknown ids with a warning.
The plugin-load phase is **planned for insertion between `Registry::builtins()` (`app.rs:620`) and the
`build_keymap` call (`app.rs:622`)** — so at build time the registry already contains the plugin
commands, and a user's `keymap.patches` entry binding `"date.insert"` resolves against the plugin
command exactly like a builtin, zero new code. A preset switch re-runs `build_keymap` (`keymap_rebuild`),
so plugin bindings re-resolve too (contract law 7).

Palette + menu: a `Plugin` entry with `menu: None` is palette-only; with `menu: Some(cat)` it also
appears in that existing menu category. Both surfaces derive from `reg.commands()` — no palette/menu
code changes.

---

## 5. The `wc.*` + editor API surface (exact P1 surface)

One global table `wc`. Registration functions are callable only during load (they mutate the registry,
which is frozen after load); the editor functions are callable during a command callback (pump time).
Calling a registration function outside load, or an editor function outside a callback, raises a Lua
error (degrade, not panic).

**Registration (load time):**
- `wc.register_command{ name = "<command>", label = "Label", menu = "Edit"|nil, fn = function() … end }`
  — `name` is the plugin-local segment (namespaced to `<plugin>.<name>` by the host); `menu` is an
  optional string **parsed to the fixed `MenuCategory` enum** (`"File"|"Edit"|"Block"|"Format"|"View"|
  "Documents"|"Settings"|"Export"` — `registry.rs:38–43`) — an unknown/oversized value is **rejected with
  a typed Lua error and never interned or leaked** (parse-to-enum is its own bound; §7 audit); `fn` is
  stored in Lua's named registry keyed by the namespaced id. A bad `menu` value or a duplicate `name` →
  typed error → status line, plugin continues where possible.
- **Registration resource caps (bounded-memory invariant — stem/name/label are interned/leaked, §4, so
  these bound a permanent allocation `set_memory_limit` cannot).** Enforced in `api.rs` **on the raw
  Lua-supplied `String`s, BEFORE interning** (never inside `register_plugin`, which sees only `&'static`);
  an over-cap `register_command` → typed error → status line (degrade, plugin continues where possible),
  nothing interned:
  - `PLUGIN_MAX_COMMANDS_PER_PLUGIN = 256` — the 257th `register_command` from one plugin is rejected.
  - `PLUGIN_MAX_STEM_LEN = 64` bytes (the `<plugin>` file/dir stem, checked once at load);
    `PLUGIN_MAX_NAME_LEN = 128` bytes (the plugin-local `name`); the full namespaced
    `<plugin>.<command>` id is thereby bounded (≤ ~193 bytes) — the *whole* leaked id is capped, not just
    the `name` segment. `PLUGIN_MAX_LABEL_LEN = 256` bytes. Any over-length → rejected, nothing interned.
  These also cap what flows into the palette label list (`palette.rs:73`) and the menu width scans
  (`menu.rs:62`/`:114`), so the caps protect layout as well as memory.

**Editor API (callback time):**
- Reads: `wc.text(a?, b?)`, `wc.selection()`, `wc.cursor()`, `wc.len()`, `wc.version()`, `wc.path()`.
- Edits (all pre-validated then via `submit_transaction` — §3b): `wc.insert(text)`,
  `wc.replace(a, b, text)` (offsets pre-checked against the live buffer; bad input → typed Lua error,
  nothing constructed), `wc.set_selection(anchor, head)`. **Edit `text` is length-checked against the
  SAME pre-allocation cap as user paste** — `clipboard::PASTE_MAX_BYTES` (the guard at
  `jobs_apply.rs:320`, `text.len() > PASTE_MAX_BYTES` → skip, that user paste already uses before
  `build_range_replace`). The plugin edit API applies that identical check at the `api.rs` layer BEFORE
  constructing any `ChangeSet` (before `Tendril::from(text)` allocates); over-cap → typed Lua error
  (degrade). Plugin edits and user paste thus share one bound (§7 audit).
- Status / errors: `wc.status(msg)` — set `editor.status`, the only user-visible output channel (no
  console — the app owns the alternate screen). `msg` is a plugin `String` copied into owned
  `Editor.status: String` (`editor.rs:395`), so it is **hard-capped/truncated to `PLUGIN_MAX_STATUS_LEN =
  4096` bytes** (display-only; a longer message is clamped on a char boundary). Lua `error(msg)` from a
  callback is caught and routed through `plugin_error` (same truncation).

**No `wc.command` in P1 — deferred to P2 (deliberate scope trim).** Dispatching another command from a
plugin needs a full `Ctx{editor,clock,executor,msg_tx}` + `&Registry` (`registry.rs:26–31`, `:649`)
that `pump(&editor)` does not supply; wiring the pump with that context (and making `wc.command`
enqueue-and-route, with the re-drain loop + chain cap that then become necessary) is P2-shaped
plumbing. A P1 plugin can already register, read, edit, and set status without invoking other commands —
so `wc.command` is dropped from P1 for leanness and lands in P2 alongside events (which need the same
dispatch context). **Flagged for the human at spec review as a scope trim vs. the original proposal.**

That is the entire P1 surface. No `wc.command`, no events, no config table, no timers, no async, no
UI-drawing API (Provider law: plugins supply data/behavior; the host owns UI/layout/focus).

---

## 6. Loading model

- **On disk:** `dirs::config_dir()/wordcartel/plugins/`. Two shapes: `<name>.lua` (single file) and
  `<name>/init.lua` (directory — the dir is prepended to that load's `package.path` so `require` grows
  naturally; no git-clone/distribution machinery in P1). Plugin name = file/dir stem.
- **Discovery & ordering:** one `read_dir` at startup, sorted **lexicographically** — deterministic and
  explainable. No manifest, no dependency graph (a plugin needing another's Lua module `require`s it via
  the shared path).
- **Eager at startup:** entry files load eagerly (they must — the palette must be complete and
  `build_keymap` runs once from the loaded registry). Neovim discipline: entry files are small
  registration stubs; heavy logic loads lazily via `require`. `--no-plugins` skips the whole phase
  (safe mode — cheap, vital for support).
- **Testable core vs the write-only `Fs` trait.** `Fs` (`fsx.rs`) is write/atomic-replace only and
  cannot back a loader — so P1 does **not** grow it. The loader core is
  `load_sources(host, &[(name, src)]) -> Vec<LoadReport>` — filesystem-free, unit-tested with string
  Lua. The shell-side `discover(dir)` does `read_dir` + a **bounded read** (the `bounded_read_opt`
  pattern; generous cap — plugin files are user code, not documents, so ~1 MiB) and feeds the core. A
  load failure (parse error, oversize, bad `register_command`) skips that plugin with a `LoadReport`
  error surfaced to the status line; other plugins proceed. Order of load into the registry follows the
  lexicographic discovery order.
- **Config & CLI (both net-new additions).** A `[plugins]` section is **added** to `Config`
  (`config.rs:42`) following the existing `RawConfig` + per-field-merge pattern. **P1 fields only:** an
  `enabled: bool` (default true) and an optional `disable: Vec<String>` (names to skip). No per-plugin
  tables (P2). A `--no-plugins` flag is **added** to `Cli` (`config.rs:7`, alongside `no_config` /
  `no_splash`) and its arg-parse arm; it overrides config to force-off (safe mode).

---

## 7. Isolation, limits, failure, trust

- **Trust posture: trusted user-installed code** — the Neovim posture, not Fresh's sandbox. The bounds
  below defend against *accidents* (infinite loop, runaway allocation), not malice. `Lua::new()`'s safe
  default stdlib (excludes `debug`/`ffi`; keeps `io`/`os` — plugins legitimately touch files) plus
  documentation carry the rest. No manual stdlib whitelist; no capability system in P1.
- **Panic isolation at every host→Lua entry.** The pump invocation and each load `pcall`/exec run inside
  `panicx::catch` (`panicx.rs:27` — its doc already names "(later) plugin call-sites" as an intended
  consumer). mlua callback errors arrive as `mlua::Result`, never an unwind. **FFI-error hazard** (a
  Rust panic inside a `create_function` closure crossing the C boundary — the class that kills Fresh's
  editor): defended twice — (1) our API closures are panic-free by construction (they call
  `Result`-returning validated APIs and `try_borrow`), and (2) mlua's Rust-panic→Lua-error conversion is
  **spike-verified** (§11), not assumed.
- **Runaway execution abort — IN P1, day one** (the posture Fresh proves an in-process design cannot
  skip). An `mlua` instruction-count hook (`Lua::set_hook`, every ~10k instructions) checks elapsed wall
  time against a budget (~100–250 ms; final value set by the §11 spike) and raises a Lua error to abort a
  callback that exceeds it → `"plugin <name>: exceeded time budget"` on the status line. This is the
  line between "plugin bug" and "editor hang" — a direct no-silent-UI-waits requirement.
- **Resource-bound completeness law (cross-cutting; mirrors the §3 input-validation law).** Because
  `set_memory_limit` is spike-gated AND bounds only the Lua heap — never a Rust-side allocation or a
  permanent leak — every plugin-supplied string crossing into Rust must be *deliberately* bounded:

  > **LAW.** Every plugin-supplied string that crosses into a PERMANENT leak (interned ids/labels) or a
  > Rust allocation MUST be bounded — either by a hard plugin-layer cap or by an existing buffer-memory
  > bound — checked/justified BEFORE the allocation.

  **P1 plugin-supplied-string audit against the LAW** (every string in the surface, how it is bounded):
  - **Plugin stem `<plugin>`** (leaked into every interned id) → `PLUGIN_MAX_STEM_LEN = 64` B, checked at
    load before interning (§5).
  - **Command `name` segment** (leaked in the interned id) → `PLUGIN_MAX_NAME_LEN = 128` B (§5).
  - **Full namespaced `<plugin>.<command>` id** (the permanent `CommandId`) → bounded by stem+name caps
    (≤ ~193 B) — the *whole* leaked id, not just `name`.
  - **Label** (leaked into `CommandMeta.label: &'static str`) → `PLUGIN_MAX_LABEL_LEN = 256` B (§5).
  - **Command count per plugin** (each adds a leaked id+label + a registry entry) →
    `PLUGIN_MAX_COMMANDS_PER_PLUGIN = 256` (§5).
  - **`menu` value** (`wc.register_command{menu=...}`) → **parse-to-enum** against the fixed
    `MenuCategory` (`registry.rs:38–43`); an unknown/oversized string is rejected with a typed error and
    **never interned or leaked** — no cap needed, the enum IS the bound (§5).
  - **`wc.insert`/`wc.replace` text** (allocates via `ChangeSet::insert`→`Tendril::from`, `change.rs:66–72`
    / `build_range_replace`→`replace_changeset`, `commands.rs:110–130`) → length-checked at the `api.rs`
    layer against the **same pre-allocation cap as user paste** — `clipboard::PASTE_MAX_BYTES`, the guard
    at `jobs_apply.rs:319–333` that user paste already applies before `build_range_replace` — BEFORE the
    `Tendril` allocation; over-cap → typed error. (NOT "bounded by M5": M5 evicts *undo history* AFTER the
    `Tendril` is already allocated, and paste has this separate pre-alloc cap that plugin edits would
    otherwise bypass. The plan reuses that mechanism/constant so plugin edits and user paste share one
    bound.)
  - **`wc.status` / `error(msg)` text** (copied into owned `Editor.status: String`, `editor.rs:395`) →
    hard-capped/truncated to `PLUGIN_MAX_STATUS_LEN = 4096` B on a char boundary (§5; display-only, not
    leaked, so a modest clamp suffices).

  These caps are the ALWAYS-ON bounded-memory guard; the VM heap cap below is a separate, Lua-only layer.
- **VM heap cap (spike-gated, Lua-side).** `Lua::set_memory_limit` (~64 MiB) if §11 confirms it is
  enforced on vendored Lua 5.4; if not, drop the cap with a documented note (do not hack one). A cap
  WezTerm skips but we shouldn't — `O(content)`-plus-baseline is a stated resource law and plugins are
  untrusted-ish. Note this bounds only the *Lua* heap, not the leaked Rust strings or buffer content —
  those are the plugin-layer caps' / M5's job (the LAW above).
- **Degrade, don't crash.** Load failure → skip + report + continue (per-plugin containment). Callback
  failure (Lua error, panic, time abort, or a rejected edit offset — §3b) → status line; the editor is
  unaffected and the buffer is untouched (a rejected edit never reached a constructor; edits that never
  reached `submit_transaction` changed nothing; one that returned `Err` changed nothing by proptest
  guarantee). Repeated-failure auto-disable is deliberately deferred — status-line reporting plus
  `--no-plugins` covers P1.
- **`plugin_error(editor, name, err)` seam** — the single formatting/routing point for all plugin
  errors (the analog of WezTerm's injectable `show_error` callback), writing to `editor.status`. Never a
  console; `print_*`/`dbg!` remain deny-lints.

---

## 8. Testing & success criteria

- **The pump is the `InlineExecutor` of plugins.** Invocation is a plain method at a known pipeline
  point, so tests build a `PluginHost` from string sources, `enqueue` a `PluginCall`, call `pump`
  directly against a real `Rc<RefCell<Editor>>`, and assert on the editor — no threads, no timing, real
  Lua (mlua is the test double *and* the production engine; deterministic and in-process, honoring the
  `InlineExecutor` discipline).
- **Loader core** — `load_sources` unit-tested on string sources: a good plugin registers its command;
  a parse error / duplicate name / bad `menu` yields a `LoadReport` error and does not abort the batch;
  lexicographic order is asserted. `discover` is tested against a tempdir at the shell layer only.
- **Loaded-but-idle guardrail** (extends the swap SSD-wear guardrail family): load a plugin, drive idle
  `Msg::Tick`s, assert **zero** callback invocations *and* `timers::next_wake` unchanged (P1 plugins arm
  no deadline — nothing to gate). Proves "loaded ≠ background work."
- **Isolation tests:** a panicking callback → caught, status set, editor intact, next command still
  works; a callback that loops → time-abort fires (driven by a low test budget); the `try_borrow_mut`
  "editor busy" path is exercised directly.
- **Offset/range pre-validation tests (guards the §3 LAW — no core primitive ever panics on plugin
  garbage; the round-3 edit + round-4 `wc.text` Criticals):** for each **range-taking** API — `wc.replace`
  and the `wc.text` **read** — an out-of-bounds offset, a reversed range (`from > to`), and a mid-char
  offset each → a **typed Lua error, buffer/selection unchanged, no panic** (the `plugin_check_range`
  pre-check rejects before any `ChangeSet` is constructed or `TextBuffer::slice` is called — proves
  garbage never reaches `from_ops`'s consumption assert or `rope.byte_slice`). (`wc.insert` takes NO
  plugin-supplied offset — it inserts at the live selection head — so it has no reversed/OOB row; it gets
  its own text/no-panic test, incl. multibyte insert text.) Plus a concurrent-`StaleLength` edit path: a
  valid-at-check-time range that a racing edit invalidates → the `submit_transaction` `validate_against`
  `Err`, zero mutation. A fuzz/property-style test driving random `(a, b, text)` from Lua across the whole
  range-taking surface asserts **no panic**.
- **Resource-cap tests (guards the §7 resource-bound LAW — no unbounded leak/alloc from plugin input):**
  an over-length stem/name/label and the 257th `register_command` each → typed error, **nothing interned**
  (verified via a stable id/label count before/after); an invalid/oversized `menu` value → typed error,
  not interned; an over-length `wc.status` → truncated to `PLUGIN_MAX_STATUS_LEN` on a char boundary; a
  `wc.insert`/`wc.replace` with text exceeding `clipboard::PASTE_MAX_BYTES` → typed error, buffer
  unchanged, **no `Tendril` allocated** (shares the user-paste cap; asserted at the `api.rs` layer).
  Registration enforcement is asserted to happen on the raw `String` (a rejected registration never
  reaches `register_plugin`).
- **Contract-invariant tests** (see §9): palette-completeness and menu-subset re-run over a registry
  containing plugin entries; a patch-bound plugin command resolves in `build_keymap` and survives a
  preset switch (law 7).
- **Success criterion:** the `insert_date.lua` demo — dropped into the plugins dir, "Insert Date"
  appears in the palette, binds via `keymap.patches`, and inserts through `submit_transaction`.

---

## 9. Command-surface-contract conformance (REQUIRED)

P1 touches commands, the palette, the menu, and keybinding hints — it MUST conform, and does so **by
derivation** (the single-registry design makes conformance structural, not vigilant):

- **Law 1 — registry is the single source of truth.** Plugin commands are registered *into the existing
  `Registry`*; there is no parallel command store. Plugin edits mutate command-reachable state only
  through the validated `submit_transaction` boundary (P1 exposes no raw-state API and no `wc.command`,
  so a plugin has literally no other mutation path). (`wc.command`, routing through existing commands'
  shared setters, arrives in P2 and keeps this discipline.)
- **Law 3 — palette exhaustive.** `palette.rs` iterates `reg.commands()`; plugin entries are ordinary
  entries, so they appear automatically. The palette-completeness test is re-run over a plugin-loaded
  registry.
- **Law 4 — menu ⊆ palette.** A plugin entry tagged `menu: Some(cat)` is a registered command, hence in
  the palette; the menu-subset invariant holds. P1 plugins cannot create new menus or dynamic sections
  (that is P3's `DYNAMIC_SECTIONS` consumer).
- **Law 7 — hints track the active keymap.** Hints come from `build_keymap`'s resolution of
  `keymap.patches` against `reg.resolve_name`; plugins load before that resolution and re-resolve on a
  preset switch — so a user's binding of a plugin command surfaces in palette/menu hints identically to
  a builtin's.
- **Laws 2 & 6 (every option is a command / one setter) — N/A in P1.** P1 adds no user-settable
  *options*; plugin *commands* are nullary verbs (rule 10's parameterized set-value commands are P3).
  The `[plugins]` config is host-level enable/disable, not a `SettingsSnapshot` option.

No amendment to the contract is required — P1 uses the seam the contract already anticipates for
plugins ("the fourth actor … plugins route through the registry spine").

---

## 10. Anti-regrowth / module structure

- **`app.rs` stays under budget (817/1000 *production* lines — the cap in
  `wordcartel/tests/module_budgets.rs:48–52` counts lines before `mod tests` at `app.rs:818`, i.e. 817).
  Its own budget comment names Effort P the budget most at risk.** P1's footprint in `app.rs` is
  deliberately tiny: the `Rc<RefCell>` wrap of the loop-local editor + the per-stage `borrow_mut` scopes
  (§2 — mechanical, near-zero net lines), one `if let Some(h) = &mut plugin_host { h.pump(&editor) }`
  stage call,
  and host construction near the other startup wiring. All plugin *logic* lives in `plugin/`. Target:
  single-digit net line growth in `app.rs`.
- **`registry.rs` growth bounded.** The change is the `HandlerKind` enum + the `Plugin` dispatch arm
  (enqueue) + `register_plugin` — a data/table extension, not a new dispatcher. `registry.rs` stays
  Lua-free (no `mlua` import).
- **New-module discipline.** `plugin/host.rs`, `plugin/api.rs`, `plugin/load.rs` — one axis of change
  each (VM+pump+queue / the `wc` surface / discovery+parse). `api.rs` uses the flat registration-seam
  vector so adding an API area never edits a dispatcher.
- **A `wordcartel/tests/module_budgets.rs` budget on `plugin/host.rs`** from day one (sized during the plan; the pump +
  queue + VM ownership are the core, everything else delegates), so the plugin hub cannot become the new
  god-object. The `clippy::too_many_lines` (threshold 100) gate applies to every new function; the pump
  loop stays a thin delegation, not an inline body.

---

## 11. Risks + the pre-P1 spike list (a gate before implementation)

A short scratch-crate spike settles the few `mlua` behaviors that cannot be verified by reading. It is
the **first step**, gating the plan; none of its outcomes change the architecture — only parameters
(and one drop-if-unsupported). Run all seven, record results in the plan:

1. **Feature set + `!Send` capture.** Recommend `mlua = { features = ["vendored", "lua54"] }` —
   deliberately *without* WezTerm's `async`/`send`/`serialize` (P1 is sync, main-thread-confined).
   Verify a `!Send` `Rc<RefCell<Editor>>` captured in a `create_function` closure compiles without the
   `send` feature. **This is the load-bearing assumption of §2–§3.**
2. **`set_memory_limit` on vendored Lua 5.4** — supported and enforced? Sets whether §7's cap ships.
3. **`set_hook` abort** — confirm raising an error from an instruction-count hook cleanly unwinds the
   in-flight Lua call in mlua; measure hook overhead at ~10k-instruction granularity; fix the time
   budget.
4. **Panic conversion at the callback boundary** — probe mlua's Rust-panic→Lua-error behavior (incl. the
   panic-during-error-handling case) to confirm §7's double defense against the FFI-error hazard is real.
5. **Startup cost** — `Lua::new()` + loading ~5 small plugins, measured against the startup/splash
   budget (must not regress perceived launch).
6. **`Lua::scope` status in current mlua** — not used by this design; one look to honestly close out the
   rejected alternative in the plan.
7. **`cargo deny`** — mlua + vendored-Lua license and duplicate-dep posture (release-checklist item per
   project law; record clean-or-findings, not a merge gate).

**Called-out integration risk — the `Rc<RefCell<Editor>>` borrow choreography (§2/§3).** Converting the
loop-local editor from `&mut editor` to `Rc<RefCell<Editor>>` touches **every** loop stage
(`pre_recv`, `reduce`, keymap/theme arms, settings-save, undo-eviction, clipboard drain, mouse
reconcile, `advance`, `render`), each of which must become its own short `borrow_mut` scope that drops
before the next. The design's safety rests on one invariant: **no `borrow_mut` is held when the pump
enters Lua** — the pump's own drain-borrow drops before callbacks run, and each API closure takes and
releases its own short borrow. Getting this wrong is a `RefCell` double-borrow — caught by
`try_borrow_mut` as "editor busy" (degrade, not crash), but a regression nonetheless. Mitigations: the
`try_borrow_mut`-everywhere rule in `api.rs` (never `borrow_mut`), a direct re-entrancy test (§8), and
keeping the wrap strictly `run()`-local so all borrow lifetimes are visible in one function. This is the
single highest-attention item for the plan and the whole-branch Fable gate.
