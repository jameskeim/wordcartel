# Effort P — grounding package (facts for independent architecture)

**Purpose.** This is the **factual** grounding for designing wordcartel's in-process Lua plugin system
(Effort P — a **significant** effort, but **not** the 1.0 capstone; scope it as solid, incrementally
valuable work, not a do-everything endgame). It deliberately contains **no decomposition and no design
decisions** —
only (1) the goal, (2) the binding constraints, (3) the real current code surface, and (4) three
prior-art reference models read at source. It exists so an independent author can propose the
architecture and phasing **from grounding**, not from prior conclusions. (Our earlier tentative
brainstorm — a P0→P3 slicing and specific forks — lives separately in
`docs/design/effort-p-plugin-system-design-space.md` and the `effort-p0` spec; those are held for
*reconciliation after* an independent proposal, not as premises.)

---

## 1. Goal

An **in-process Lua plugin system** for wordcartel (a markdown-first Rust terminal word processor;
functional-core `wordcartel-core` + imperative-shell `wordcartel`, binary `wcartel`; ratatui 0.30).
Plugins are user-installed Lua that extends the editor. It is a **significant effort but NOT the 1.0
capstone** — scope it as solid, incrementally shippable work, not the do-everything endgame; prefer a
lean, valuable first phase over maximal completeness. It is the spine for automation. Reference intent:
adopt the *lifecycle/mechanics* of a Neovim-style embedded-Lua editor,
but **mediate** all host access (no raw internals) so the editor's valid-by-construction guarantees
hold.

## 2. Binding constraints (non-negotiable; authoritative sources in-repo)

Sources: `CLAUDE.md` (project law) and `docs/design/command-surface-contract.md`. Summary:

- **Instant typing, no data loss, no silent UI waits.** Per-keystroke work stays `O(visible)+O(edited)`,
  never `O(document)`. The input loop must never block. **Idle is free** — with no input/animation the
  loop BLOCKS; background work is **edge-triggered by a real content/state change, never level-triggered
  off wall-clock**. Any plugin hook touching the hot path must be bounded / time-sliced / dispatched to
  the job substrate — never "run arbitrary Lua synchronously on every keystroke."
- **Valid-by-construction / no data loss.** Buffers/selection are private + validated; edits go through
  a single validated transaction boundary (see code map §submit_transaction). Plugins must never mutate
  raw internal state.
- **`#![forbid(unsafe_code)]`** in all first-party crates (`wordcartel-core`, `wordcartel`, `main.rs`).
  A native VM's `unsafe` must stay encapsulated inside the dependency; `wordcartel-core` stays VM-free.
- **Command-surface contract** (authoritative doc): the **command registry is the single source of
  truth**; the palette is exhaustive; the menu ⊆ palette; every user-settable option IS a command with
  one shared setter; keybinding hints track the active keymap. Plugin-registered commands/options must
  conform. (One pre-existing exception: a dynamic non-registry "Documents" menu section.)
- **Module structure — dispatchers delegate, don't implement.** New behavior enters through a
  **registration seam** (a data-table row, an exhaustive-enum variant, a feature module), never by
  growing a central `match`/loop. Enforced by `clippy::too_many_lines` (fn threshold 100) and
  `wordcartel/tests/module_budgets.rs` (hub file budgets). Effort P **must** register into seams, not
  grow `reduce`/`run`.
- **Errors → status line, never console.** The app owns the alternate screen; `print_*`/`dbg!` are
  deny-lints. Typed errors surface to `editor.status`.
- **Resource behavior.** Memory ≈ fixed baseline + `O(content)` + bounded undo; disk writes track
  saves/edits, never idle duration. Guardrail tests assert idle/settled states do no background work.

## 3. Real current code surface (mapped 2026-07-11 against the live tree)

**Already built and reusable:**
- **Validated write boundary — DONE.** `wordcartel/src/transact.rs::submit_transaction(editor, txn,
  clock) -> Result<(), EditError>` — its own header calls itself *"Effort P's apply(Transaction)
  seam."* Validates via `ChangeSet::validate_against` (zero mutation on Err), snaps selection, applies
  once via trusted `editor.apply`. proptest-hardened (2048 hostile cases: never-panics / on-Err-zero-
  mutation / on-Ok-in-bounds+char-boundary). Constructors a plugin API would wrap:
  `ChangeSet::insert/delete/from_ops/validate_against` (`change.rs`), `Selection::single/range`
  (`selection.rs`), `Transaction::new/with_selection` (`history.rs`). `EditError = StaleLength{expected,
  actual} | OpBoundary{pos}`.
- **Panic isolation — DONE.** `panicx::catch<T>(f) -> Result<T, String>` — `catch_unwind` + thread-local
  re-entrancy guard; already used by the job executor; its doc names "(later) plugin call-sites."
- **Job substrate.** `jobs.rs`: `trait Executor { dispatch(Job); drain() -> Vec<JobOutcome> }`.
  `ThreadExecutor` = one named worker thread (`std::thread` + `mpsc`), each job run inside
  `panicx::catch`; `InlineExecutor` = deterministic test double. Results post back via `Msg::JobDone`;
  `is_stale` version-discards. **Single FIFO worker today**, not a pool.
- **Error surfacing.** `Editor.status: String`; per-domain `describe_*` fns assign to it; painted each
  frame. No console channel.
- **Command registry (palette/automation spine).** `registry.rs`: `CommandId(pub &'static str)` (`Copy`);
  `Ctx<'a> { editor: &'a mut Editor, clock: &'a dyn Clock, executor: &'a dyn Executor, msg_tx:
  Sender<Msg> }`; `Handler = fn(&mut Ctx) -> CommandResult` (bare fn pointer); `CommandResult = Handled
  | Noop | Quit` (`commands.rs`); `CommandMeta { label: &'static str, menu: Option<MenuCategory>, state:
  Option<fn(&Editor)->MenuMark> }`. **Registration is a CLOSED compile-time table** — `Registry {
  entries, index }` (private fields), built once by `Registry::builtins()` via private `register`/
  `register_stateful`. **No runtime registration API.** `dispatch(&self, id, ctx) -> CommandResult`
  (HashMap lookup; unknown → `editor.status="unknown command"` + `Noop`). Palette (`palette.rs`) and menu
  (`menu.rs`) DERIVE from `reg.commands()` — single source of truth (except the dynamic Documents
  section). The header comment states the intent: *"Plugins (Effort P) register CommandId→Handler here
  without touching the enum."*
- **`Ctx` construction sites (production):** `input.rs:41`, `app.rs:188` (overlay helper), `timers.rs:177`,
  `prompts.rs:169/216/228/240/246`, `jobs_apply.rs:185` (+ many `#[cfg(test)]` sites).
- **DiagnosticsProvider seam (Effort A — the newest Open-Closed extension point).** `diag_provider.rs`:
  `trait DiagnosticsProvider { name; availability; ensure_running; configure; notify_change(..)->
  Accepted; notify_close; reload_dictionary; shutdown }`. `Editor` holds one `Box<dyn
  DiagnosticsProvider>` (single-slot box-swap, defaults to `NullProvider`), wired imperatively at
  run-loop start; async results via `Msg::DiagnosticsDone`/`Msg::DiagProviderEvent`. A `#[cfg(test)]
  RecordingProvider` exists. Model: trait behind a boxed field, async results as new `Msg` variants,
  hermetic no-op default.

**NOT built (new work required):**
- **The registry is closed** (above) — opening it to runtime registration is new.
- **No hook/event seam of any kind.** The `reduce` dispatch is a **hardcoded 11-stage `intercept`
  chain** in `app.rs` (splash→marks→menu→palette→theme_picker→file_browser→prompts→minibuffer→
  search_ui→diag_overlay→outline_overlay), NOT a data table. Adding an interceptor edits `app.rs`
  directly. There is **no** pub/sub, no `on_save`/`on_open`/`on_edit`, no listener registry
  (grep-confirmed). Contrast `timers.rs`: `SUBSYSTEMS: &[TimedSubsystem]` IS a static table whose header
  says it "upgrades to a `Vec` when Effort P needs dynamic (plugin) timer registration" — the one
  forward-declared plugin seam. The intercept chain has no such table yet.
- **`app.rs` module budget = ≤1000 production lines** (`module_budgets.rs`); currently **818** (≈182
  headroom). Its test comment: *"Effort P wires plugins in HERE, so this is the budget most at risk:
  plugin arms/hooks must register into a seam, not grow reduce/run."* Sibling budgets: `render.rs`≤900,
  `timers.rs`≤400.
- **`mlua`/Lua absent** from all manifests. **No plugin-dir config** (`Config` has 9 fields:
  keymap/state/mouse/view/diagnostics/theme/export/menu/clipboard; adding a section follows the
  `RawConfig` + per-field-merge pattern; config dir convention is `dirs::config_dir()/wordcartel/`).
  `DiagnosticsConfig.linters: Option<Vec<String>>` exists but is dead (belongs to a future diagnostics-
  provider selector).
- **The `Fs` trait (`fsx.rs`) is write/atomic-replace only** (`create_excl`/`existing_mode`/`rename`/
  `sync_dir`/`remove_file`) — **no read-dir/read-file**; it cannot back a plugin loader.
- **`Editor` is owned locally in `app::run`** (`app.rs:471`); `Registry` local after
  (`app.rs:620`); `reduce` called with `(msg, editor, reg, keymap, executor, clock, msg_tx)`.

---

## 4. Reference model — Neovim (in-process Lua; the canonical model)

Embeds LuaJIT in its C core (no RPC), giving Lua synchronous access to editor state. Three mechanics:
(1) **`runtimepath`** directory scan; (2) **eager `plugin/`** sourcing (tiny files that register
commands/keymaps) vs **lazy `lua/`** loading (a custom `package.searchers` entry resolves `require('x')`
on demand); (3) a global **`vim`** bridge — `vim.api` (direct C buffer/window manipulation), `vim.fn`,
`vim.cmd`. **`vim.api` hands Lua first-class synchronous access to internal memory structures** — the
part a valid-by-construction editor should NOT replicate raw. All of this ports to `mlua` (embed +
custom `package.searchers`). Hooks are the **autocmd/event** system (on save/open/etc.); Lua runs
synchronously on the main loop — a slow autocmd janks it (the hot-path risk our constraints forbid).

## 5. Reference model — Fresh (`sinelaw/fresh`; out-of-process sandboxed JS — the counter-model)

A mature Rust/ratatui editor whose plugins are **sandboxed TypeScript in a QuickJS VM on a dedicated OS
thread**, talking to the editor only by **async message-passing**. Most of its traits are downstream of
that boundary:
- Because plugin code runs off the edit thread, a slow/infinite plugin **can't stall typing** — and
  Fresh exploits this with **zero execution bounds** ("a plugin can infinite-loop"). *An in-process VM
  inverts this*: a slow in-process hook WOULD stall typing, so an in-process design cannot copy the
  no-bounds posture.
- The serialize-across-a-thread boundary forces a **~205-variant `PluginCommand` enum funneled through
  one giant `match`** — a textbook dispatch-attractor god-object. In-process (direct Rust host calls)
  avoids this entirely; do NOT recreate it.
- **One-frame async lag**: plugin effects land next render, never the current. JS before-hooks are
  **observational — cannot veto a keystroke** (the real veto is a native-Rust hook).
- Worth stealing regardless of runtime: the **Provider law** (plugin supplies *data*; host owns UI /
  layout / navigation / focus — their retro: plugins that drew their own UI reimplemented navigation and
  shipped keybinding/i18n bugs); **generated typed API** from the Rust source; **per-plugin side-effect
  tracking → compensating teardown on unload**; classify plugin outputs visual/non-visual to kill a
  render→hook→ack→render feedback loop (they hit a 13 Hz one). Crash isolation asymmetry: JS exceptions
  caught, but a Rust panic in an FFI callback re-panics onto the main thread and kills the editor.

## 6. Reference model — WezTerm (`wezterm/wezterm`; Rust host + mlua — the closest analog)

Mature Rust terminal embedding `mlua = 0.9` with features `["vendored","lua54","async","send",
"serialize"]` (PUC **Lua 5.4**, vendored). Read at source 2026-07-11. Directly relevant because it is a
Rust-host-embeds-mlua system at scale.

**Host-state access — the crux (all file-grounded):**
- **`lua.scope` is used ZERO times** across the entire tree (exhaustive grep). `set_app_data`/
  `app_data_ref` also **zero**. A mature codebase on the identical mlua feature set never reaches for
  scoped lending.
- **Dominant pattern: global singleton + interior mutability, re-fetched per call.** `Mux::get() ->
  Arc<Mux>` from a `lazy_static Mutex<Option<Arc<Mux>>>`; every `create_function` closure that needs
  host state calls it fresh (`lua-api-crates/mux/src/lib.rs`). Nothing is borrowed across the boundary.
- **ID-based `UserData` handles:** `MuxPane(PaneId)` / `MuxWindow(WindowId)` — `Copy` newtypes wrapping
  only an id, no lifetime, freely stashable in Lua; methods `resolve()` the live object from the global
  registry per call (grabbing a short-lived `RwLock` guard for that one call).
- **Deferred notification queue for the exclusively-owned big struct.** `TermWindow` (the closest analog
  to our `Editor`) is **never** touched inside a Lua callback. Lua posts `TermWindowNotif` messages
  (with oneshot-channel round-trips for reads); the owning event loop applies them to its own `&mut
  self` on its own turn — including a generic `Apply(Box<dyn FnOnce(&mut TermWindow)+Send+Sync>)`. This
  is how they sidestep "callback needs state that's already exclusively borrowed" — the state is never
  entered from the callback's call stack.
- **Thread confinement:** one long-lived `Rc<mlua::Lua>` in a thread-local, all calls serialized onto
  the main thread (`with_lua_config_on_main_thread` panics if off-thread); the `send` feature is only for
  moving the VM across the async executor's threads, never concurrent `&mut` host access.

**API construction / loading / errors:**
- **API object = a single plain `Table`** (`get_or_create_module`), populated with `create_function`s,
  assembled via a **registration-seam vector**: each `lua-api-crates/*` crate exposes `pub fn register(&
  Lua) -> Result<()>`; `env-bootstrap` collects them in a flat array (`for func in [.., ..] {
  add_context_setup_func(func) }`). "Add an API surface = one crate + one line, never edit a dispatcher."
  **WezTerm independently arrived at wordcartel's own registration-seam rule.** `UserData` is reserved
  for stateful **handle** objects, not the namespace.
- **Persistent callbacks = Lua named-registry values** (`set_named_registry_value` keyed by string,
  e.g. `"wezterm-event-{name}"`) — a growable Lua table of fns per event; **not** Rust-side `RegistryKey`
  handles, not app-data. Registration happens **directly during `exec()`** (interior-mutable `&Lua`) — no
  collect-then-apply staging on the Lua side.
- **VM = `Lua::new()`** (safe ctor, never `unsafe_new`); **no `unsafe` feature**, no manual stdlib
  whitelist, **no `set_memory_limit`** — relies on mlua's safe default (excludes `debug`/`ffi`).
- **No `catch_unwind` around Lua calls (a real gap)** — a Rust panic in a callns closure aborts via the
  process panic hook. *Wordcartel already does better (M4/`panicx`); keep panic isolation at the plugin
  boundary; consider a memory cap WezTerm skips, since untrusted plugins matter more here.*
- **Error surfacing = an injectable callback seam:** the config crate defines
  `assign_error_callback`/`show_error`; the GUI wires a real UI sink (CLI mode degrades to `eprintln!`).
  Analog for us: a plugin-error seam the shell wires to the **status line**.
- **Loading:** `package.path` prepended with config/plugin dirs; plugins found by ordinary `require`
  (lazy for modules; eager `eval` for the main config once). A **wrap-not-replace** `package.searchers[2]`
  hook adds a reload-watch list without changing load semantics. Distribution = git-repo plugins cloned
  into a data dir, resolved via `package.path` (scale beyond a single-file model).

---

## 7. What the independent proposal must produce (dimensions, not answers)

Propose, grounded in §2–§6:
1. **Decomposition** — how to slice Effort P into independently shippable, reviewable phases (each a
   coherent milestone a user could see), with the ordering rationale and what each phase de-risks.
2. **Host-access mechanism** — how a plugin's Lua reaches editor state given the instant-typing +
   valid-by-construction + single-validated-write-boundary constraints and the WezTerm evidence
   (scope vs interior-mutability-handle vs deferred-queue vs hybrid). Address reads, edits, and the
   hot-path/hook execution model.
3. **Command / registration seam** — how plugin-registered commands (and later options/hooks) integrate
   with the closed registry + the command-surface contract + the module-budget/anti-regrowth rule,
   without growing `app.rs`/`reduce`.
4. **Loading model** — what a plugin is on disk, discovery, ordering, eager/lazy, and the growth path.
5. **Isolation / limits / failure** — panic isolation, resource bounds, the FFI-error hazard, degrade-
   don't-crash, and the trust posture (trusted user-installed vs sandboxed).
6. **Testing/determinism** — how the design stays unit-testable (the `InlineExecutor` discipline; the
   `Fs` trait is write-only so a loader needs a string-source testable core) and how "loaded-but-idle
   does no background work" is guarded.
7. **Where it is genuinely uncertain** — call out any decision that rests on an unverified `mlua`
   behavior and should be settled by a spike before implementation.
