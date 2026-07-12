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
  validated **edits** (`insert`/`replace`/`set_selection`) — all routing through `submit_transaction`.
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

- **`plugin/host.rs` — `PluginHost`.** Owns the one `mlua::Lua` VM, the `Bridge` (the `Rc<RefCell<Editor>>`
  handle + a `Sender<Msg>` clone for status), and the `pending: VecDeque<PluginCall>` queue. Exposes
  `PluginHost::new(bridge) -> Result<PluginHost, HostError>`, `pump(&mut self)` (the drain — §3), and
  `enqueue(&mut self, PluginCall)`. Carries a `module_budgets.rs` production-line budget from day one.
- **`plugin/api.rs` — the `wc` table.** Builds the global `wc` Lua table WezTerm-style: a **flat
  registration-seam vector** of `fn(&Lua, &Bridge) -> mlua::Result<()>` entries, each installing one
  API area's `create_function`s. Adding an API area is one function + one row — never editing a
  dispatcher (the project's own anti-regrowth rule, which WezTerm independently arrived at).
- **`plugin/load.rs` — the loader.** `load_sources(host, &[(name, src)]) -> Vec<LoadReport>` is the
  filesystem-free testable core (real Lua, string sources). A thin shell-side `discover(dir) ->
  Vec<(String, String)>` does the `read_dir` + bounded read and feeds the core (§6).
- **`Rc<RefCell<Editor>>` confined to `app::run`.** Today `run` owns `let mut editor` (`app.rs:471`)
  and calls `reduce(msg, &mut editor, …)`. P1 wraps it: `let editor = Rc::new(RefCell::new(editor_val));`
  The loop takes exactly one `editor.borrow_mut()` guard per iteration for the `reduce` call (behavior-
  identical to today's `&mut editor`), and **drops that guard before the pump stage** so the pump's
  API closures can take their own short borrows. `reduce` and every helper keep their `&mut Editor`
  signatures unchanged — the `Rc<RefCell>` is a `run()`-local detail, invisible to the rest of the
  shell and to core.
- **`PluginHost` is `Option` in the loop / null when absent.** `--no-plugins`, a load-time VM failure,
  or the no-plugins-dir case leaves `plugin_host: Option<PluginHost> = None`; the pump stage is a
  cheap `if let Some(h) = …` (mirrors the `NullProvider` / boxed-`DiagnosticsProvider` null-object
  discipline from Effort A). e2e journeys that don't exercise plugins construct no host.

Pipeline placement in the loop (`app.rs` run loop, after `reduce` returns at `app.rs:754`), as a new
sibling of the existing post-`reduce` stages (`rebuild_keymap_if_requested`, `rederive_theme_if_requested`,
the settings-save arm): **`reduce` → (drop borrow) → `plugin pump` → keymap/theme/settings arms →
`advance` → `draw`.** The pump runs before `advance`/`draw`, so plugin effects land in the *same frame*
(no Fresh-style one-frame lag).

---

## 3. Host-access mechanism (the crux)

Main-thread-confined VM + a single deferred invocation point (the pump) + a live editor handle with
short per-API-call borrows. This is WezTerm's proven posture (never touch the exclusively-owned struct
from inside a Lua call stack; interior mutability re-fetched per call; `lua.scope` used zero times),
adapted to a single-threaded loop — which lets P1 *improve* on WezTerm's deferred notification queue:
because the safe point is on the same thread in the same frame, API calls borrow the live editor
directly instead of round-tripping oneshot channels.

### a. Reads — live, synchronous, per-call borrow
Each read API function does a short `editor.try_borrow_mut()` (or `try_borrow`) for the duration of that
one call and returns owned data to Lua. Reads are O(requested), never O(document):
- `wc.text(a?, b?)` → the buffer substring (whole buffer if omitted); bounds clamped to char
  boundaries via the buffer's existing `clamp_to_boundary`.
- `wc.selection()` → `{anchor, head}` byte offsets of the primary selection.
- `wc.cursor()` → the primary `head` byte offset.
- `wc.len()` → buffer byte length; `wc.version()` → the document version; `wc.path()` → the active
  buffer's path string or `nil`.

Live reads mean read-after-write inside one callback behaves naturally (insert, then ask the cursor —
it reflects the insert), which a snapshot model cannot do.

### b. Edits — ONLY via `submit_transaction`
There is no raw-state API by construction — the bridge simply never exposes one. Edit functions build a
`ChangeSet` against the *live* buffer length and submit through the proven boundary
(`transact::submit_transaction(editor, txn, clock) -> Result<(), EditError>`):
- `wc.insert(text)` → `ChangeSet::insert(cursor, text, len)` at the primary head.
- `wc.replace(a, b, text)` → a delete-then-insert `ChangeSet` over `[a, b)` (positions clamped to char
  boundaries; a mid-char position that survives clamping yields `EditError::OpBoundary`).
- `wc.set_selection(anchor, head)` → routes through the same selection-snapping `submit_transaction`
  applies (out-of-bounds snaps, never rejects — the existing behavior).

`submit_transaction` is already proptest-hardened (2048 hostile cases: never-panics / on-Err-zero-
mutation / on-Ok-in-bounds-and-char-boundary). The plugin edit boundary **inherits that guarantee for
free** — it is the exact seam whose own header names itself "Effort P's apply(Transaction) seam." On
`Err`, the edit function raises a Lua error carrying the `EditError` (the plugin may `pcall` it);
uncaught, it surfaces via `plugin_error` → status line. Zero mutation on error is guaranteed by
`submit_transaction`, not by plugin discipline.

### c. The `PluginCall` queue + same-frame drain — plugin code never runs inside `reduce`
When the palette / menu / a keybinding dispatches a plugin command, the registry's `Plugin` handler arm
(§4) does **not** call Lua. It pushes `PluginCall { id }` onto the host's pending queue and returns
`CommandResult::Handled`. Lua is invoked only by `PluginHost::pump`, called from the run loop *after*
`reduce` has returned and its editor borrow is dropped. The pump:

1. Drains `pending` FIFO. For each `PluginCall`, looks up the plugin's stored Lua callback (a value in
   Lua's named registry, keyed by the command id string — WezTerm's persistent-callback pattern) and
   invokes it inside `panicx::catch` + the `set_hook` time guard (§7).
2. Each invoked callback may itself call `wc.command("<id>")`. If the target is another **plugin**
   command, it is **enqueued**, not recursed — so callbacks never re-enter Lua under their own call
   stack. If the target is a **builtin**, it is dispatched immediately through the normal registry
   `Ctx` path (builtins are non-re-entrant Rust and hold their own borrow discipline).
3. A **per-pump chain cap** (constant `MAX_PLUGIN_CHAIN = 16`) bounds the total callbacks executed in
   one pump. Exceeding it aborts the remaining queue with a `plugin_error` ("plugin command chain
   exceeded 16 — possible loop") — pre-empting the render→hook→render feedback class (Fresh hit a 13 Hz
   loop; we make it impossible to hang the frame).

Because the pump runs before `advance`/`draw`, effects are visible the same frame. Because it runs with
no editor borrow held, the API closures' `try_borrow_mut` always succeeds in the normal path.

### d. Re-entrancy → "editor busy" (degrade, not panic)
Every API closure uses `try_borrow_mut`, never `borrow_mut`. In the designed control flow the pump holds
no borrow, so this always succeeds. But should any future path invoke Lua while a borrow is live, the
`try_borrow_mut` returns `Err` and the API function raises a Lua error `"editor busy"` → status line —
a graceful degrade, never a `RefCell` double-borrow panic. This is a defensive invariant, tested
directly (§8), not a hoped-for property.

---

## 4. Command & registration seam

Open the *existing* `Registry` (`registry.rs`) — do not build a parallel one. The command-surface
contract's laws hold **by derivation** only if plugin commands live in the same table `palette.rs` and
`menu.rs` already iterate via `reg.commands()`.

Two concrete obstacles in the real code and their minimal resolutions:

- **`Handler = fn(&mut Ctx) -> CommandResult` is a bare fn pointer** (`registry.rs:34`) — it cannot
  carry a plugin closure. Resolution: `CommandEntry.handler` becomes
  `enum HandlerKind { Builtin(Handler), Plugin }`. Dispatching a `Builtin` is exactly today's call;
  dispatching a `Plugin` pushes `PluginCall { id }` onto the host queue and returns
  `CommandResult::Handled`. **No `mlua` type enters `registry.rs`** — the enqueue is done through a thin
  seam (`Ctx` gains a way to reach the pending queue, e.g. via `msg_tx` posting an enqueue message, or a
  dedicated field — resolved in the plan; the invariant is that `registry.rs` stays Lua-free).
- **`CommandId(pub &'static str)` is `Copy`** (`registry.rs:16`) and every consumer relies on it
  (`KeyAction::Id(CommandId)`, palette, menu, hints, `resolve_name`). Plugin names are runtime `String`s.
  Resolution: **intern** each plugin command's namespaced name to `&'static str` once at registration
  (leak-once via a small global interner; process-lifetime; bounded by user-installed plugin count;
  bytes in size). `CommandId` stays `Copy`; every existing consumer is untouched.

New public registry surface:
- `Registry::register_plugin(&mut self, name: &'static str, label: &'static str, menu:
  Option<MenuCategory>) -> Result<(), RegisterError>` — `RegisterError` on a name collision (with a
  builtin or an already-registered plugin command) → surfaced via `plugin_error`. The interned name is
  produced by `register_command` in `api.rs` before this call.
- Names are **namespaced `<plugin>.<command>`**, enforced at registration (the `<plugin>` segment is the
  file/dir stem). Deterministic, collision-resistant, self-documenting in the palette.
- Registration happens only at **plugin load time** (startup in P1), never mid-`reduce`. The registry is
  immutable while dispatch is live — the same between-reduces discipline the keymap swap already uses
  (`keymap_rebuild`). `run()`'s `let reg` (`app.rs:620`) becomes `let mut reg` for the load window only;
  `reduce` keeps `&Registry`.

**Free keybindings — no new binding code.** `build_keymap(km, reg)` resolves every `keymap.patches`
chord through `reg.resolve_name(id_str)` (`keymap.rs:508`, `:561`) and drops unknown ids with a warning.
Plugins load **before** `build_keymap` runs (`app.rs:620–624`), so a user's
`keymap.patches` entry binding `"date.insert"` resolves against the plugin command exactly like a
builtin — zero new code. A preset switch re-runs `build_keymap` (`keymap_rebuild`), so plugin bindings
re-resolve too (contract law 7).

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
  optional string matching a `MenuCategory` variant (`"File"|"Edit"|"Block"|"Format"|"View"|
  "Documents"|"Settings"|"Export"`), validated at registration; `fn` is stored in Lua's named registry
  keyed by the namespaced id. A bad `menu` string or a duplicate `name` → load-report error, plugin
  continues where possible.

**Editor API (callback time):**
- Reads: `wc.text(a?, b?)`, `wc.selection()`, `wc.cursor()`, `wc.len()`, `wc.version()`, `wc.path()`.
- Edits (all via `submit_transaction`): `wc.insert(text)`, `wc.replace(a, b, text)`,
  `wc.set_selection(anchor, head)`.
- Dispatch: `wc.command(id)` — dispatch another command by full id (plugin → enqueued; builtin →
  immediate; §3c chain cap applies).
- Status / errors: `wc.status(msg)` — set `editor.status` (the only user-visible output channel; no
  console — the app owns the alternate screen). Lua `error(msg)` from a callback is caught and routed
  through `plugin_error`.

That is the entire P1 surface. No events, no config table, no timers, no async, no UI-drawing API
(Provider law: plugins supply data/behavior; the host owns UI/layout/focus).

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
- **Config:** a `[plugins]` section added to `Config` following the existing `RawConfig` +
  per-field-merge pattern (`config.rs`). **P1 fields only:** an `enabled: bool` (default true) and an
  optional `disable: Vec<String>` (names to skip). No per-plugin tables (P2). `--no-plugins` overrides
  config to force-off.

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
- **Memory cap (spike-gated).** `Lua::set_memory_limit` (~64 MiB) if §11 confirms it is enforced on
  vendored Lua 5.4; if not, drop the cap with a documented note (do not hack one). A cap WezTerm skips
  but we shouldn't — `O(content)`-plus-baseline is a stated resource law and plugins are untrusted-ish.
- **Degrade, don't crash.** Load failure → skip + report + continue (per-plugin containment). Callback
  failure (error, panic, time abort, chain-cap) → status line; the editor is unaffected and the buffer
  is untouched (edits that never reached `submit_transaction` changed nothing; one that returned `Err`
  changed nothing by proptest guarantee). Repeated-failure auto-disable is deliberately deferred —
  status-line reporting plus `--no-plugins` covers P1.
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
  works; a callback that loops → time-abort fires (driven by a low test budget); a plugin command that
  `wc.command`s itself → chain-cap aborts with the loop message; the `try_borrow_mut` "editor busy" path
  is exercised directly.
- **Edit-boundary test:** a plugin `wc.replace` with a mid-char position → `EditError::OpBoundary`
  surfaces, buffer unchanged (the `submit_transaction` guarantee, re-asserted through the plugin path).
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
  through `submit_transaction` and (via `wc.command`) existing registered commands' setters — never a
  novel raw mutation path.
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

- **`app.rs` stays under budget (818/1000 today; its own test names Effort P the budget most at risk).**
  P1's footprint in `app.rs` is deliberately tiny: the `Rc<RefCell>` wrap of the loop-local editor, one
  `if let Some(h) = plugin_host { h.pump(...) }` stage call, and host construction near the other
  startup wiring. All plugin *logic* lives in `plugin/`. Target: single-digit net line growth in
  `app.rs`.
- **`registry.rs` growth bounded.** The change is the `HandlerKind` enum + the `Plugin` dispatch arm
  (enqueue) + `register_plugin` — a data/table extension, not a new dispatcher. `registry.rs` stays
  Lua-free (no `mlua` import).
- **New-module discipline.** `plugin/host.rs`, `plugin/api.rs`, `plugin/load.rs` — one axis of change
  each (VM+pump+queue / the `wc` surface / discovery+parse). `api.rs` uses the flat registration-seam
  vector so adding an API area never edits a dispatcher.
- **A `module_budgets.rs` budget on `plugin/host.rs`** from day one (sized during the plan; the pump +
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

**Called-out integration risk — the `Rc<RefCell<Editor>>` borrow choreography.** The design's safety
rests on one loop invariant: **the `reduce` borrow is dropped before the pump runs, and the pump holds
no borrow across a Lua call** (each API closure takes and releases its own). Getting this wrong is a
`RefCell` double-borrow — caught by `try_borrow_mut` as "editor busy" (degrade, not crash), but a
regression nonetheless. Mitigations: the `try_borrow_mut`-everywhere rule (never `borrow_mut`), a direct
re-entrancy test (§8), and keeping the wrap strictly `run()`-local so the borrow lifetimes are visible in
one function. This is the single highest-attention item for the plan and the whole-branch Fable gate.
