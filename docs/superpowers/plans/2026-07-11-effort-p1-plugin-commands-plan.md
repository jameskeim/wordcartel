# Effort P1 — implementation plan: in-process Lua plugin commands

**Spec:** `docs/superpowers/specs/2026-07-11-effort-p1-plugin-commands-design.md` (Codex-clean,
human-approved 2026-07-11).
**Branch:** `effort-p1-plugin-commands`.
**Shape:** a **spike GATE** first (prove the load-bearing `mlua` behaviors — a superset of spec §11's
seven — or STOP and revise), then seven integration tasks built so the tree stays green after each:
deps+skeleton → registry seam → registration API + loader core → editor API + pump → loader/config/CLI →
`app::run` wiring (the top integration risk) → the full test suite + `insert_date.lua` demo. The
loader/config/CLI task lands BEFORE the `app::run` wiring so `run()` can reference `discover`/`cfg.plugins`/
`cli.no_plugins` on a green tree. Subagent-driven, TDD per task (failing test → impl → green → commit),
per-task reviewer (spec-compliance + quality), one Fable whole-branch gate + one Codex pre-merge gate.

Anchor on symbol NAMES (lines drift). `cargo` + `grep` are ground truth, never an editor "unused"/
"undefined" hint (subagent edits are the most stale in an analyzer's view).

---

## Global constraints (bind EVERY task — copy into each implementer/reviewer dispatch)

1. **The two design laws are GATES, not guidelines.** (a) *Input-validation LAW* (spec §3): every plugin
   API taking a byte offset/range pre-validates it against the LIVE buffer via `plugin_check_range`
   (in-bounds, `from<=to`, char boundaries) and returns a typed Lua error — no raw plugin offset reaches
   an asserting core primitive (`ChangeSet::from_ops`/`apply`/`insert`/`delete`, `TextBuffer::slice`).
   (b) *Resource-bound LAW* (spec §7): every plugin-supplied string that crosses into a permanent leak or
   a Rust allocation is bounded before the allocation — the load-layer caps (stem/name/label/count, menu
   parse-to-enum, edit-text paste cap, status truncation). A new input-taking API added without both
   guards is a defect.
2. **`#![forbid(unsafe_code)]` holds.** `Rc<RefCell<>>` + owned captures are safe; `mlua`'s `unsafe`
   stays inside the dependency. No `unsafe` in `wordcartel`/`wordcartel-core`. `wordcartel-core` stays
   VM-free (no `mlua` import there — ever).
3. **`registry.rs` stays Lua-free** — no `mlua` import. The `Plugin` dispatch arm pushes a `Copy`
   `CommandId`; nothing Lua-typed enters the registry module.
4. **Module-budget / anti-regrowth (GATE).** `app.rs` stays under **1000 production lines**
   (`wordcartel/tests/module_budgets.rs` — currently 817). New logic lives in `wordcartel/src/plugin/`;
   `app.rs` gains only the `Rc<RefCell>` wrap + per-stage borrows + one pump-stage call + host
   construction. Add a `plugin/host.rs` budget row (Task 6). `clippy::too_many_lines` (threshold 100)
   binds every new fn — the pump loop is a thin delegation, not an inline body.
5. **Command-surface-contract conformance (GATE, spec §9).** Plugin commands register into the EXISTING
   `Registry`; palette/menu derive unchanged, so palette-completeness + menu-subset hold by construction;
   keymap patch resolution (`reg.resolve_name`) gives free bindings. Task 8 re-runs the contract
   invariant tests over a plugin-loaded registry.
6. **House style** (CLAUDE.md): dense hand-formatting, `—` em-dashes never `--`, no emoji, doc-comment
   every public item, snake_case/PascalCase/SCREAMING_SNAKE. Do NOT run `cargo fmt`. Match neighbors.
7. **Errors → status line, never console.** `print_*`/`dbg!` are deny-lints. All plugin errors route
   through the `plugin_error` seam → `editor.status`.
8. **GATES before any merge:** `cargo test` green (both crates); `cargo build` + `cargo test --no-run`
   warning-free for touched crates; `cargo clippy --workspace --all-targets` clean. Run
   `scripts/smoke/run.sh` in the pre-merge report (mandatory-run / advisory-pass).
9. **Commit per task** with project trailers (Co-Authored-By + Claude-Session). Message form:
   `feat(p1): <task summary>` (or `test(p1):`/`chore(p1):` as fits).

---

## Task 1 — mlua spike (GATE: prove the load-bearing behaviors, or STOP)

**This gates everything.** A scratch crate proving the `mlua` assumptions the design rests on — a
**superset of spec §11's seven items** (all seven, plus two useful extras). It is **not** merged — it
lives in `/tmp/claude-*/plugin-spike/` (or a gitignored `spike/` dir) and its findings are recorded in
the effort ledger. **If the spike DISPROVES any load-bearing assumption, STOP and revise the design
before Task 2** — the fallbacks are named inline.

**Model:** standard implementer. **Files:** a throwaway cargo crate (NOT in the workspace).

Prove, each a `fn main`/`#[test]` in the scratch crate. Items 1–7 are spec §11.1–§11.7 (the FULL spec
list); 8–9 are useful extras — this spike is a **superset** of the spec's seven.

1. **(§11.1) `!Send` capture without the `send` feature — THE load-bearing one.** With
   `mlua = { version = "<current>", features = ["vendored", "lua54"] }` (NO `send`/`async`/`serialize`),
   a closure capturing `Rc<RefCell<i64>>` compiles and runs in `lua.create_function`:
   ```rust
   let state = std::rc::Rc::new(std::cell::RefCell::new(0i64));
   let lua = mlua::Lua::new();
   let s = state.clone();
   let f = lua.create_function(move |_, n: i64| { *s.borrow_mut() += n; Ok(()) })?;
   lua.globals().set("bump", f)?;
   lua.load("bump(5)").exec()?;
   assert_eq!(*state.borrow(), 5);
   ```
   *If this fails to compile (mlua requires `Send` captures by default):* STOP. Fallback per spec §11 —
   re-evaluate whether the handle model needs a different cell, or whether a thread-local VM changes the
   capture bound. Do not proceed to integration on a red result here. **(GREEN required.)**
2. **(§11.2) `set_memory_limit` on vendored 5.4.** `lua.set_memory_limit(64 << 20)?`; allocate past it;
   confirm an `Err` (memory error) rather than a process abort. *If unsupported/unenforced:* per spec §7,
   the VM heap cap is DROPPED with a documented note — the always-on registration caps (Task 3/4) remain
   the real bound. **(Allowed documented-red → drop the heap cap.)**
3. **(§11.3) `set_hook` time/instruction abort unwinds cleanly.** Install `lua.set_hook` on an
   instruction-count trigger that returns `Err` after N instructions; run an infinite `while true do end`;
   confirm the `exec()` returns `Err` (not a hang, not an abort), the VM is reusable afterward, and measure
   hook overhead at ~10k-instruction granularity. *If abort does not unwind cleanly:* STOP — the runaway
   guard is mandatory (spec §7); revise before integration. **(GREEN required.)**
4. **(§11.4) Panic→Lua-error conversion.** A `create_function` closure that `panic!`s: confirm the panic
   surfaces as an `Err` at the `exec()`/call site (mlua converts), and that wrapping the whole call in
   `panicx::catch`-equivalent (`catch_unwind`) also catches it — the double defense (spec §7).
   **(GREEN required.)**
5. **(§11.5) Startup cost.** Time `Lua::new()` + loading ~5 small chunks; confirm it is well under the
   startup/splash budget (target < ~10 ms; record the number). **(Recorded.)**
6. **(§11.6) `Lua::scope` status.** One look at whether `lua.scope` still exists / is usable in the pinned
   mlua — the design does NOT use it, so this closes out the rejected alternative honestly.
   **(Documentation-only, never blocking.)**
7. **(§11.7) `cargo deny check`.** Not a scratch-crate probe — a release-checklist check on a branch with
   the `mlua`/vendored-Lua dep added, recording the license/duplicate-dep posture (clean-or-findings; NOT
   a merge gate — mirrors the project's `cargo deny` = release-checklist-not-gate rule). **(Recorded.)**
8. **(extra) Named-registry persistent callbacks.** `lua.set_named_registry_value("k", func)` then
   `lua.named_registry_value::<mlua::Function>("k")?.call(())?` across separate `load().exec()` calls —
   confirm a stored Lua fn survives and re-invokes (the callback-storage mechanism for `register_command`).
   **(GREEN required.)**
9. **(extra) `package.path` prepend + `require`.** Prepend a dir to `package.path` and `require` a module
   from it — confirm the directory-plugin (`<name>/init.lua`) load path works (spec §6). **(GREEN required.)**

**Acceptance:** the load-bearing set **{1, 3, 4, 8, 9} GREEN**; {2, 5, 6, 7} **recorded** (2 may be
documented-red → drop the heap cap; 5/6/7 informational). Findings written to
`$(git rev-parse --git-path sdd)/progress.md` — especially the §11.2 `set_memory_limit` verdict and the
§11.1 `!Send`-capture result. **On any red in {1, 3, 4, 8, 9}: STOP, surface to the human, revise the
spec/plan before Task 2.**

---

## Task 2 — `mlua` dep, `plugin/` module skeleton, `PluginHost::null`, plugin caps in `limits.rs`

Wires the dependency and an inert skeleton. Nothing calls into it yet — the tree stays green and the app
behaves identically (no plugins loaded).

**Model:** standard. **Files:** `wordcartel/Cargo.toml`, `wordcartel/src/lib.rs`,
`wordcartel/src/plugin/mod.rs` (+ `host.rs`, `api.rs`, `load.rs`), `wordcartel/src/limits.rs`.

**TDD first:**
- `plugin::host::tests::null_host_constructs_and_pumps_noop` — `PluginHost::null()` builds, and a pump on
  an empty queue is a no-op (once `pump` exists in Task 5 it is `Option`; here assert the null host exists
  and holds no VM).
- `limits::tests::plugin_caps_are_sane` — the new constants exist with the spec values.

**Implementation:**
- `Cargo.toml`: `mlua = { version = "<pinned by spike>", features = ["vendored", "lua54"] }`.
- `limits.rs` — add (canonical home for quotas):
  ```rust
  /// P1 plugin registration caps (bounded-memory LAW — interned ids/labels are permanent leaks
  /// that `set_memory_limit` does not bound; checked on the raw Lua String BEFORE interning).
  pub const PLUGIN_MAX_COMMANDS_PER_PLUGIN: usize = 256;
  pub const PLUGIN_MAX_STEM_LEN: usize = 64;    // the <plugin> file/dir stem
  pub const PLUGIN_MAX_NAME_LEN: usize = 128;   // the plugin-local command name
  pub const PLUGIN_MAX_LABEL_LEN: usize = 256;  // the menu/palette label
  pub const PLUGIN_MAX_STATUS_LEN: usize = 4096; // wc.status / error(msg) truncation (display-only)
  // Edit text reuses PASTE_MAX_BYTES (above) — plugin edits and user paste share one pre-alloc bound.
  ```
- `lib.rs`: `pub mod plugin;` (grep the existing `pub mod` block; add in logical order).
- `plugin/mod.rs`: `pub mod host; pub mod api; pub mod load;` + a `//!` module doc naming the design.
- `plugin/host.rs`: the `PluginHost` type. In Task 2 it is minimal:
  ```rust
  //! The plugin VM host: owns the one mlua VM + bridge, and the pump (Task 5). Null when no plugins.
  pub struct PluginHost {
      lua: Option<mlua::Lua>,   // None in the null host
      // bridge + pending drain added in Task 5
  }
  impl PluginHost {
      /// A null host — no VM, no plugins. Used for --no-plugins, load failure, and tests
      /// that don't exercise plugins (mirrors NullProvider).
      pub fn null() -> PluginHost { PluginHost { lua: None } }
  }
  ```
- `plugin/api.rs`, `plugin/load.rs`: `//!` docs + stub signatures filled in Tasks 4–6.

**Acceptance:** `cargo build -p wordcartel` warning-free; `cargo test -p wordcartel plugin::` green; app
runs unchanged (`cargo run` opens normally). `module_budgets` unaffected.

---

## Task 3 — registry seam: `HandlerKind`, name/label interner, `register_plugin` (collision-only), `Editor.pending_plugin_calls`

Opens the closed registry to runtime plugin entries — WITHOUT any Lua. Fully unit-testable with a fake
plugin entry. `registry.rs` stays Lua-free.

**Model:** most-capable (this is the load-bearing seam; `CommandId: Copy` + `CommandMeta: Copy`
invariants must survive). **Files:** `wordcartel/src/registry.rs`, `wordcartel/src/editor.rs`,
`wordcartel/src/plugin/mod.rs` (the interner + `PluginCall`).

**TDD first (in `registry.rs` tests):**
- `register_plugin_adds_a_dispatchable_command` — after `reg.register_plugin(id, label, None)`,
  `reg.resolve_name(<id>)` is `Some`, `reg.meta(id).label == <label>`, and it appears in `reg.commands()`.
- `register_plugin_rejects_collision` — registering an id equal to a builtin (`"save"`) or a prior
  plugin id → `Err(RegisterError::Duplicate)`, registry unchanged.
- `plugin_dispatch_enqueues_not_runs` — dispatching a `Plugin` entry pushes one `PluginCall` onto
  `ctx.editor.pending_plugin_calls` and returns `CommandResult::Handled` (no Lua involved).
- `intern_is_stable` — interning the same string twice yields the same `&'static` pointer (or at least
  equal `CommandId`); distinct strings distinct.

**Implementation:**
- `plugin/mod.rs` — the interner + call type:
  ```rust
  use std::collections::HashSet;
  use std::sync::Mutex;
  use crate::registry::CommandId;

  /// A queued plugin-command invocation. `Copy` id only — the pump (Task 5) looks up the Lua
  /// callback by this id. Lives on Editor so both registry dispatch and the pump reach it.
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub struct PluginCall { pub id: CommandId }

  /// Intern a runtime string to `&'static str` (leak-once). PERMANENT — callers MUST cap length
  /// and count on the raw String BEFORE calling this (resource-bound LAW). De-dupes so re-interning
  /// an equal string does not leak twice.
  pub fn intern(s: &str) -> &'static str {
      static POOL: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);
      let mut g = POOL.lock().expect("intern pool");
      let set = g.get_or_insert_with(HashSet::new);
      if let Some(existing) = set.get(s) { return existing; }
      let leaked: &'static str = Box::leak(s.to_owned().into_boxed_str());
      set.insert(leaked);
      leaked
  }
  ```
  (Interner is process-global + de-duping so tests and reload-adjacent paths don't leak unbounded.
  Guarded `.expect` on a poisoned mutex is acceptable — a poisoned intern pool is unrecoverable.)
- `editor.rs` — add the queue field to `Editor` (grep the `// global app state` block near `status:
  String`):
  ```rust
  /// Plugin-command invocations queued by the registry Plugin dispatch arm, drained by the pump
  /// (P1). Default-empty; a settled/idle editor never grows it. VecDeque for FIFO.
  pub pending_plugin_calls: std::collections::VecDeque<crate::plugin::PluginCall>,
  ```
  Initialize `VecDeque::new()` in every `Editor` constructor (grep `Editor {` literal constructions —
  `new_from_text` and any others; the compiler will force each site).
- `registry.rs` — the seam:
  ```rust
  /// A registered command's implementation: a built-in fn pointer, or a plugin (enqueue + pump).
  pub enum HandlerKind { Builtin(Handler), Plugin }

  #[derive(Debug, PartialEq, Eq)]
  pub enum RegisterError { Duplicate }
  ```
  Change `CommandEntry.handler: Handler` → `handler: HandlerKind`; `builtins()`'s `register`/
  `register_stateful` wrap in `HandlerKind::Builtin(...)`. `dispatch` matches:
  ```rust
  match &self.entries[i].handler {
      HandlerKind::Builtin(h) => h(ctx),
      HandlerKind::Plugin => {
          ctx.editor.pending_plugin_calls.push_back(crate::plugin::PluginCall { id });
          CommandResult::Handled
      }
  }
  ```
  Add:
  ```rust
  /// Register a plugin command. Inputs are ALREADY interned `&'static` (the load layer capped +
  /// interned them, Task 4) — so the only failure here is a collision with a builtin or an
  /// earlier plugin command. Never leaks (interning happened upstream).
  pub fn register_plugin(&mut self, id: CommandId, label: &'static str, menu: Option<MenuCategory>)
      -> Result<(), RegisterError> {
      if self.index.contains_key(&id) { return Err(RegisterError::Duplicate); }
      self.index.insert(id, self.entries.len());
      self.entries.push(CommandEntry { id, handler: HandlerKind::Plugin,
          meta: CommandMeta { label, menu, state: None } });
      Ok(())
  }
  ```
  `dispatch` gains `id` in scope (it already has it). Keep `builtins()`'s `#[allow(clippy::too_many_lines)]`.

**Acceptance:** `cargo test -p wordcartel registry::` green; `cargo build`/`test --no-run` warning-free;
clippy clean. Existing registry tests (`commands_iterate_in_registration_order_with_meta`, etc.) still
pass (the `HandlerKind` wrap is transparent to them).

---

## Task 4 — `PluginHost` VM + `wc` registration API (`register_command`) + `load_sources` string-core

The registration half of the API: a real VM, the `wc` table's `register_command`, the load-layer resource
caps (BEFORE interning), menu parse-to-enum, callback storage — and the filesystem-free `load_sources`
core that drives it. Tested entirely on string sources (no disk).

**Model:** most-capable (the mlua boundary + both laws' registration half). **Files:**
`wordcartel/src/plugin/host.rs`, `wordcartel/src/plugin/api.rs`, `wordcartel/src/plugin/load.rs`,
`wordcartel/src/registry.rs` (a `menu_from_str` helper next to `MenuCategory`).

**TDD first (in `plugin/load.rs` tests, string sources):**
- `load_registers_command_into_registry` — `load_sources(&mut reg, &mut host, &[("greet",
  "wc.register_command{ name='hello', label='Hello', fn=function() end }")])` → `reg.resolve_name(
  "greet.hello")` is `Some`, label `"Hello"`, menu `None`.
- `load_namespaces_id_by_stem` — id is `"<stem>.<name>"`.
- `load_rejects_over_length_name` — a `name` > `PLUGIN_MAX_NAME_LEN` → `LoadReport` error, **nothing
  interned** (assert `reg.commands().count()` unchanged), batch continues.
- `load_rejects_over_length_label` / `load_rejects_over_length_stem` / `load_rejects_257th_command` —
  each → error, nothing interned.
- `load_rejects_bad_menu` — `menu='Nonsense'` → error, not interned; `menu='Edit'` → `Some(Edit)`.
- `load_reports_collision` — two plugins registering the same id → the second yields a `Duplicate`
  `LoadReport`, batch continues.
- `load_is_atomic_per_plugin` — a single plugin whose 1st `register_command` is fine and 2nd collides (or
  over-caps) → the WHOLE plugin is skipped: **neither** command is registered (`reg.commands().count()`
  unchanged), a `LoadReport` error is returned.
- `load_parse_error_is_reported_not_fatal` — a plugin with a Lua syntax error → error for that plugin,
  others load.

**Implementation:**
- `registry.rs` — `pub fn menu_from_str(s: &str) -> Option<MenuCategory>` (exhaustive match on the eight
  variants; the parse-to-enum bound). Return `None` for unknown → the caller turns that into a typed error.
- `plugin/host.rs` — real VM + registration sink. Registration cannot hold `&mut Registry` across the Lua
  boundary, so `register_command` appends to a shared **pending sink**; the caller drains it into
  `&mut Registry` after exec (collect-then-apply on the Rust side; the Lua *callback fn* is stored in the
  named registry during exec):
  ```rust
  pub struct PendingReg { pub id: CommandId, pub label: &'static str, pub menu: Option<MenuCategory> }

  pub struct PluginHost {
      lua: Option<mlua::Lua>,
      // Task 5 adds: bridge (Rc<RefCell<Editor>> + Sender<Msg>).
  }
  impl PluginHost {
      pub fn new() -> mlua::Result<PluginHost> {
          let lua = mlua::Lua::new();               // safe ctor (no debug/ffi); never unsafe_new
          #[allow(clippy::let_underscore_untyped)]
          if let Some(cap) = spike_confirmed_mem_cap() { lua.set_memory_limit(cap)?; } // dropped if spike red
          Ok(PluginHost { lua: Some(lua) })
      }
  }
  ```
- `plugin/api.rs` — the registration-seam vector (WezTerm pattern) + `register_command`:
  ```rust
  /// Install the `wc` registration surface for ONE plugin's exec pass. `stem` fixes the namespace;
  /// `sink` collects PendingReg (drained into the Registry after exec); `per_plugin_count` enforces
  /// the per-plugin command cap. All caps are checked on the RAW Lua String BEFORE interning.
  pub(crate) fn install_registration(
      lua: &mlua::Lua, stem: &'static str,
      sink: Rc<RefCell<Vec<PendingReg>>>, count: Rc<Cell<usize>>,
  ) -> mlua::Result<()> {
      let wc: mlua::Table = lua.globals().get("wc").or_else(|_| {
          let t = lua.create_table()?; lua.globals().set("wc", &t)?; Ok::<_, mlua::Error>(t) })?;
      let reg_fn = lua.create_function(move |lua, spec: mlua::Table| {
          let name: String = spec.get("name")?;
          let label: String = spec.get("label")?;
          let menu_s: Option<String> = spec.get("menu")?;
          let func: mlua::Function = spec.get("fn")?;
          // Resource-bound LAW — cap on the RAW String, before interning:
          if count.get() >= crate::limits::PLUGIN_MAX_COMMANDS_PER_PLUGIN {
              return Err(mlua::Error::runtime("plugin: too many commands (max 256)")); }
          if name.len() > crate::limits::PLUGIN_MAX_NAME_LEN {
              return Err(mlua::Error::runtime("plugin: command name too long")); }
          if label.len() > crate::limits::PLUGIN_MAX_LABEL_LEN {
              return Err(mlua::Error::runtime("plugin: label too long")); }
          let menu = match &menu_s {
              None => None,
              Some(m) => Some(crate::registry::menu_from_str(m)
                  .ok_or_else(|| mlua::Error::runtime(format!("plugin: unknown menu '{m}'")))?),
          };
          let full = format!("{stem}.{name}");
          let id = crate::registry::CommandId(crate::plugin::intern(&full));   // ← after caps pass
          let label_s = crate::plugin::intern(&label);
          lua.set_named_registry_value(&format!("wc-cmd-{}", id.0), func)?;    // persistent callback
          count.set(count.get() + 1);
          sink.borrow_mut().push(PendingReg { id, label: label_s, menu });
          Ok(())
      })?;
      wc.set("register_command", reg_fn)?;
      Ok(())
  }
  ```
  (`stem` is interned once at load; caps on `name`/`label` are on the raw `String`; the menu string is
  parse-to-enum; `intern` is only reached AFTER all caps pass — resource-bound LAW satisfied. The
  editor-API functions — reads/edits/status — are added to `wc` in Task 5 via a sibling installer.)
- `plugin/load.rs` — the testable core. **Signature deviation from spec §6, intentional (MINOR):** the
  spec wrote `load_sources(host, &[(name, src)]) -> Vec<LoadReport>`; since registration mutates the
  `Registry`, the real signature threads `reg: &mut Registry` — stated here so it is a deliberate
  refinement, not a drift.
  ```rust
  pub struct LoadReport { pub plugin: String, pub result: Result<usize, String> } // Ok(n_commands)

  /// Filesystem-free load core: exec each (stem, source) into the host VM, collect registrations,
  /// commit into `reg` ATOMICALLY per plugin. Per-plugin failure (parse error, cap, collision) is
  /// isolated AND all-or-nothing — a failing plugin leaves ZERO commands registered (spec §"skip the
  /// whole plugin").
  pub fn load_sources(reg: &mut Registry, host: &PluginHost, sources: &[(String, String)])
      -> Vec<LoadReport> {
      let Some(lua) = host.lua() else { return Vec::new(); };  // null host: nothing
      let mut reports = Vec::new();
      for (stem_raw, src) in sources {
          let report = crate::panicx::catch(|| load_one(reg, lua, stem_raw, src))  // panicx at entry
              .unwrap_or_else(Err);
          reports.push(LoadReport { plugin: stem_raw.clone(), result: report });
      }
      reports
  }
  ```
  `load_one` — **atomic (all-or-nothing) per plugin**:
  1. `stem_raw.len() <= PLUGIN_MAX_STEM_LEN` (else `Err`, nothing done); `intern(stem_raw)`.
  2. Fresh `sink`/`count`; `api::install_registration(lua, stem, sink.clone(), count.clone())?`;
     `lua.load(src).set_name(stem).exec()` — a parse/exec error → `Err(e.to_string())`, and since NOTHING
     has been committed to `reg` yet, the plugin is cleanly skipped. (The Lua callbacks stored in the
     named registry during exec are inert dead keys if we bail — harmless; a future reload clears the VM.)
  3. **Preflight before ANY commit:** with the drained `sink`, check every entry's id against `reg` AND
     against the others in this batch for a collision, and re-confirm the batch is within
     `PLUGIN_MAX_COMMANDS_PER_PLUGIN`. If ANY entry would collide/over-cap → return `Err` having committed
     NONE (the plugin registers zero commands).
  4. Only if the whole preflight passes: `for p in sink { reg.register_plugin(p.id, p.label, p.menu) }`
     — every call now provably `Ok` (preflight already ruled out the sole `Duplicate` failure). Return
     `Ok(count)`.
  This guarantees a half-valid plugin (cmd1 fine, cmd2 duplicate) commits neither — no partial registration.

**Acceptance:** `cargo test -p wordcartel plugin::load` green (all TDD cases, incl. an atomicity test: a
plugin whose 2nd `register_command` collides leaves its 1st command UNregistered); warning-free; clippy
clean. No disk touched by the core.

---

## Task 5 — the `wc.*` editor API (`plugin_check_range`, reads, edits, status) + `PluginHost::pump`

The callback-time half: the editor API honoring BOTH laws, and the two-phase pump that is the only place
Lua runs. Tested pump-as-`InlineExecutor` — construct a host, enqueue a `PluginCall`, call `pump` against
an `Rc<RefCell<Editor>>`, assert on the editor. No threads, no `app::run`.

**Model:** most-capable (both laws' callback half + the borrow-across-Lua invariant). **Files:**
`wordcartel/src/plugin/api.rs`, `wordcartel/src/plugin/host.rs`.

**TDD first (in `plugin/host.rs` tests):**
- `pump_runs_enqueued_plugin_command` — register `date.insert` whose `fn` calls `wc.insert('X')`;
  enqueue `PluginCall{date.insert}`; `pump(&editor)`; buffer gained `X` at the caret.
- `pump_holds_no_borrow_during_lua` — a callback calling `wc.text()` then `wc.insert()` then `wc.cursor()`
  in sequence succeeds (proves each API call takes+drops its own borrow; no "editor busy").
- Input-validation LAW (no panic): `wc.replace(10, 2, 'x')` (reversed), an OOB range, a mid-char offset,
  and `wc.text(10,2)` each → a typed Lua error surfaced to status, **buffer/selection unchanged, no
  panic**.
- Resource LAW: `wc.insert(<text > PASTE_MAX_BYTES>)` → typed error, buffer unchanged, no `Tendril`
  path taken; `wc.status(<len > PLUGIN_MAX_STATUS_LEN>)` → `editor.status` truncated on a char boundary.
- `panicking_callback_is_isolated` — a `fn` that `error()`s / panics → caught, status set, editor intact,
  a subsequent pump of another command still works.
- `editor_busy_on_nested_reentry` — the defensive `try_borrow_mut` `Err` path yields `"editor busy"`,
  not a `RefCell` panic (drive it by holding a borrow while invoking — a white-box test).

**Implementation:**
- `plugin/host.rs` — the bridge + pump:
  ```rust
  pub struct Bridge {
      pub editor: Rc<RefCell<Editor>>,
      pub msg_tx: std::sync::mpsc::Sender<crate::app::Msg>,
      pub clock: /* a Clock handle usable at pump time */,
  }
  impl PluginHost {
      /// Drain-then-invoke, single pass (no wc.command in P1 → no mid-pump growth). Takes the HANDLE,
      /// never &mut Editor, so no borrow is held across Lua.
      pub fn pump(&mut self, editor: &Rc<RefCell<Editor>>, clock: &dyn Clock) {
          let Some(lua) = self.lua.as_ref() else { return; };
          // Phase A — drain under a short borrow that drops immediately:
          let calls: Vec<PluginCall> = {
              let mut e = editor.borrow_mut();
              std::mem::take(&mut e.pending_plugin_calls).into_iter().collect()
          };
          // Phase B — invoke with NO outer borrow held:
          for call in calls {
              let key = format!("wc-cmd-{}", call.id.0);
              let cb: mlua::Result<mlua::Function> = lua.named_registry_value(&key);
              let outcome = crate::panicx::catch(|| {
                  let f = cb?;                       // missing callback → Lua error
                  self.with_time_guard(lua, || f.call::<()>(()))   // set_hook runaway guard (spike #2)
              });
              if let Err(msg) | Ok(Err(msg)) = normalize(outcome) {
                  crate::plugin::plugin_error(editor, call.id.0, &msg);   // → status line
              }
          }
      }
  }
  ```
  (`with_time_guard` installs `set_hook` with the spike-fixed budget around the single call and removes
  it after; `normalize` flattens `Result<Result<_,LuaErr>,PanicMsg>` to one message. `plugin_error` takes
  a short `borrow_mut` to set `editor.status`.)
- `plugin/api.rs` — `plugin_check_range` (the shared chokepoint) + `install_editor_api`:
  ```rust
  /// The input-validation LAW chokepoint (spec §3). Shared by wc.text (read) and wc.replace (edit).
  pub(crate) fn plugin_check_range(buf: &TextBuffer, from: usize, to: usize)
      -> Result<(), PluginRangeError> {
      if from > to { return Err(PluginRangeError::Reversed { from, to }); }
      if to > buf.len() { return Err(PluginRangeError::OutOfBounds { to, len: buf.len() }); }
      if buf.clamp_to_boundary(from) != from { return Err(PluginRangeError::NotBoundary { pos: from }); }
      if buf.clamp_to_boundary(to)   != to   { return Err(PluginRangeError::NotBoundary { pos: to }); }
      Ok(())
  }
  ```
  `install_editor_api(lua, bridge)` adds to `wc`, each closure taking a short `try_borrow_mut` via the
  bridge's `Rc` clone (never `borrow_mut`; `Err` → `"editor busy"` Lua error):
  - `wc.text(a?, b?)` → default `a=0`, `b=len`; `plugin_check_range` → on `Err` a typed Lua error;
    else `TextBuffer::slice(a..b)` → owned `String` to Lua.
  - `wc.selection`/`wc.cursor`/`wc.len`/`wc.version`/`wc.path` → owned reads, no offset input.
  - `wc.insert(text)` → `text.len() > PASTE_MAX_BYTES` → typed error (resource LAW, before any alloc);
    else cursor from live selection, `plugin_check_range(buf, cur, cur)`, `ChangeSet::insert(cur, text,
    len)`, `submit_transaction(editor, Transaction::new(cs), clock)`; `EditError` → typed Lua error.
  - `wc.replace(a, b, text)` → `text.len()` cap; `plugin_check_range(buf, a, b)`;
    `commands::build_range_replace(a, b, text, doc_len)` → `submit_transaction`.
  - `wc.set_selection(anchor, head)` → an **identity `ChangeSet`** (no text edit) + `Transaction`
    carrying the selection; `submit_transaction` snaps it (out-of-bounds clamps, never rejects), so no
    pre-check. `submit_transaction` requires a `Transaction { changes: ChangeSet, .. }` (transact.rs,
    history.rs), so the changes must be a validated identity over the live `doc_len`:
    ```rust
    let doc_len = editor.borrow().active().document.buffer.len();
    let ident = wordcartel_core::change::ChangeSet::from_ops(
        vec![wordcartel_core::change::Op::Retain(doc_len)], doc_len);  // retain-all == identity
    let txn = wordcartel_core::history::Transaction::new(ident)
        .with_selection(wordcartel_core::selection::Selection::range(anchor, head));
    crate::transact::submit_transaction(&mut editor.borrow_mut(), txn, clock)?;  // snaps the selection
    ```
    (`Retain(doc_len)` sums to `len_before` so `from_ops`'s consumption assert holds; `submit_transaction`
    clamps `anchor`/`head` to char boundaries in `[0, doc_len]` — the existing snap-not-reject behavior.
    Do NOT invent a raw selection mutation or an unchecked `from_ops`.)
  - `wc.status(msg)` → truncate `msg` to `PLUGIN_MAX_STATUS_LEN` on a char boundary → `editor.status`.
  Register both installers via the flat seam vector (registration installer from Task 4 + this one).

**Acceptance:** `cargo test -p wordcartel plugin::host` green (every TDD case, especially the no-panic +
resource + isolation cases); warning-free; clippy clean.

---

## Task 6 — loader `discover` (+ skipped-report) + `[plugins]` config + `--no-plugins` + module budget

The filesystem + config + CLI surface around the tested core, plus the `plugin/host.rs` budget row.
**Ordered BEFORE the `app::run` wiring (Task 7)** so `run()` can reference `discover`, `cfg.plugins`, and
`cli.no_plugins` and every task compiles on a green tree.

**Model:** standard. **Files:** `wordcartel/src/plugin/load.rs`, `wordcartel/src/config.rs`,
`wordcartel/tests/module_budgets.rs`.

**TDD first:**
- `load::tests::discover_reads_single_file_and_dir` — a tempdir with `a.lua` and `b/init.lua` → two
  sources, lexicographic order `["a", "b"]`, stems correct.
- `discover_skips_disabled` — a name in `disable` is skipped.
- `discover_reports_skipped_oversize` — a file over the bounded-read cap is **skipped AND named in the
  returned report** (not silently dropped), others load.
- `config::tests::plugins_section_parses` — `[plugins] enabled=false, disable=["x"]` folds into
  `Config.plugins`; default is `enabled=true, disable=[]`.
- `config::tests::parse_cli_no_plugins_flag` — `--no-plugins` sets `cli.no_plugins`.

**Implementation:**
- `config.rs` — add `pub no_plugins: bool` to `Cli` (grep `pub struct Cli`; add near `no_splash`) and the
  `"--no-plugins" => cli.no_plugins = true,` arm in `parse_cli`. Add `pub plugins: PluginsConfig` to
  `Config` (grep `pub struct Config`), a `PluginsConfig { enabled: bool (default true), disable:
  Vec<String> }`, and the `RawConfig` + per-field-merge following the existing pattern (grep an existing
  section like `ClipboardConfig`/`DiagnosticsConfig` for the raw-parse + merge shape).
- `plugin/load.rs` — `discover` must **surface skipped files** (spec §"Load failure → skip + report"), so
  it returns the sources AND a skipped-report, not a bare `Vec`:
  ```rust
  /// The outcome of scanning the plugins dir: loadable (stem, source) pairs + a report of files
  /// skipped (oversize / unreadable) so the caller can surface them to the status line.
  pub struct Discovered {
      pub sources: Vec<(String, String)>,     // (stem, source), lexicographic by stem
      pub skipped: Vec<LoadReport>,            // reuse LoadReport { plugin, result: Err(reason) }
  }
  pub fn discover(dir: &Path, disable: &[String]) -> Discovered { … }
  ```
  `read_dir`, collect `*.lua` files (stem = filename minus `.lua`) and `*/init.lua` dirs (stem = dir
  name), skip `disable`, sort lexicographically by stem, bounded-read each (the `bounded_read_opt`/
  generous ~1 MiB pattern — plugin files are user code, not documents); an over-size/unreadable file goes
  to `skipped` (a `LoadReport` with `Err(reason)`), never into `sources`. (Does NOT touch `Fs` — that
  trait is write-only; discover reads directly, and the load logic is in `load_sources`.)
- `module_budgets.rs` — add:
  ```rust
  #[test]
  fn plugin_host_stays_bounded() {
      // plugin/host.rs — the VM+pump+bridge hub. Budget with headroom over the P1 size.
      assert_hub_budget("src/plugin/host.rs", 400);   // tune to the merged size + headroom
  }
  ```

**Acceptance:** `cargo test -p wordcartel plugin::load config::` green; warning-free; clippy clean.

---

## Task 7 — `app::run` integration: `Rc<RefCell<Editor>>`, the borrow choreography, the pump stage (TOP RISK)

Wire the host into the real loop. **This is the highest-attention task** (the borrow choreography — spec
§11). Sequence carefully; every existing loop stage becomes its own short borrow scope, `msg_tx` moves
before the plugin-load phase, and the pump runs between `reduce` and the keymap/theme arms holding NO
outer borrow. Depends on Task 6 (`discover`/`cfg.plugins`/`cli.no_plugins`).

**Model:** most-capable. **Files:** `wordcartel/src/app.rs` (the `run` fn only; `reduce` and helpers keep
`&mut Editor`), **`wordcartel/src/e2e.rs`** (the `Harness` needs a `PluginHost` field + a pump slot in
`step` so the e2e test actually drives the pump — see below).

**TDD first:**
- **`e2e.rs` harness change (prerequisite for the test):** the real `Harness` (`struct Harness`, e2e.rs)
  has no `PluginHost` and `Harness::step` (the shared `snapshot → reduce → surface_undo_eviction →
  advance → render` sequence) has no pump slot — so a plugin test cannot reach the pump as-is. Add an
  `Option<PluginHost>` (+ the `Rc<RefCell<Editor>>` handle or an equivalent) to `Harness`, and insert the
  pump call in `step` at the SAME point `run` uses it (after `reduce`, before the keymap/advance stages).
  A test constructor loads plugin sources into the harness's registry+host.
- `e2e::tests::plugin_command_dispatches_via_palette` — a harness loaded with an `insert_date`-style
  plugin: dispatch its command through the registry/palette path, `step`, assert the buffer changed via
  the pump. (First test exercising the real pump stage in a loop-shaped sequence.)
- A borrow-safety regression: a `step` that dispatches a plugin command does not panic (no double
  `borrow`), asserted by the harness completing.

**Implementation (in `run`, anchored by symbol/behavior, not line):**
1. **Move `msg_tx` creation earlier** — the `let (msg_tx, msg_rx) = channel()` currently sits ~after
   `Registry::builtins()`. Move it **above** the plugin-load phase so the bridge can capture
   `msg_tx.clone()`. (Mechanical; the channel has no dependency on the intervening lines — verify by
   compile.)
2. **Plugin-load phase** — between `Registry::builtins()` and `build_keymap` (so plugin commands are in
   the registry before keymap resolution → free bindings). Load needs only `&mut reg` + the VM (NOT the
   editor), so it runs here; the bridge is attached after the editor is wrapped (step 3):
   ```rust
   let mut plugin_host = if cli.no_plugins || !cfg.plugins.enabled {
       crate::plugin::host::PluginHost::null()
   } else {
       match crate::plugin::host::PluginHost::new() {
           Ok(h) => {
               let disc = crate::plugin::load::discover(&plugins_dir, &cfg.plugins.disable);
               for r in disc.skipped { warns.push(format!("plugin {} skipped: {}", r.plugin,
                   r.result.err().unwrap_or_default())); }              // surface skipped (Task 6)
               for r in crate::plugin::load::load_sources(&mut reg, &h, &disc.sources) {
                   if let Err(e) = &r.result { warns.push(format!("plugin {}: {e}", r.plugin)); }
               }
               h
           }
           Err(e) => { warns.push(format!("plugins disabled: {e}")); PluginHost::null() }
       }
   };
   let reg = reg; // freeze (was `let mut reg` for the load window)
   ```
3. **Wrap the editor, THEN attach the bridge.** Keep all pre-loop editor mutation (session restore,
   splash, `first_frame_settle`, first draw) on the plain `&mut editor`; convert to
   `let editor = Rc::new(RefCell::new(editor));` JUST before the `loop {`. Immediately after,
   `plugin_host.attach_bridge(editor.clone(), msg_tx.clone(), /* clock handle */)` — this installs the
   editor-API closures (reads/edits/status, Task 5) now that the handle exists. Registration (Task 4)
   already happened in step 2 and never touched the editor, so this ordering is sound.
4. **Per-stage borrow choreography** — every loop-body stage that touches editor takes its own short
   scope (spec §2). Each `stage(&mut editor, …)` becomes `stage(&mut editor.borrow_mut(), …)` (or an
   explicit `{ let mut e = editor.borrow_mut(); … }`); the immutable `next_wake` uses `&editor.borrow()`.
   Order: `timers::next_wake` (`&borrow`), `timers::pre_recv` (`borrow_mut`), `reduce` (`borrow_mut`
   scope), **`plugin_host.pump(&editor, &clock)`** ← NEW, holds NO borrow, `rebuild_keymap_if_requested`,
   `rederive_theme_if_requested`, settings-save arm, `surface_undo_eviction`, `drain_clipboard_intents`,
   `reconcile_mouse_capture`, `advance`, `render::render`, session-persist check/use — each its own
   `borrow`/`borrow_mut` scope. The pump slots directly after `reduce`'s borrow scope closes and before
   the keymap arm.
5. **Post-loop shutdown — audit each use for the CORRECT mutability** (Codex CRITICAL 2): after the loop,
   - `editor.diag_provider.shutdown()` needs **`&mut self`** (`DiagnosticsProvider::shutdown`,
     `diag_provider.rs`) → `editor.borrow_mut().diag_provider.shutdown();`.
   - `recovery::dump_all_dirty(&editor, &dir)` on `ExitReason::InputLost` reads → `&editor.borrow()`.
   - `session_restore::persist_session(&mut session, &editor, &cfg, seq)` reads editor → `&editor.borrow()`.
   - `drop(guard)` is terminal-only, no editor borrow.
   Give each its own short scope; do NOT hold one borrow across several.

**Acceptance:** `cargo test -p wordcartel` green (incl. the new e2e); the app runs, loads a plugin from
the config dir, and its command works via palette + keybinding; `--no-plugins` cleanly skips;
`module_budgets` app.rs still < 1000 (report the number). Warning-free; clippy clean.

---

## Task 8 — full §8 test suite + `insert_date.lua` e2e demo + guardrails + contract invariants

Consolidate the acceptance evidence: the guardrails and contract tests the spec §8 names that aren't
already covered task-locally, plus the end-to-end success demo.

**Model:** standard. **Files:** `wordcartel/src/plugin/` test modules, `wordcartel/src/e2e.rs`,
`wordcartel/tests/` (a contract-invariant test if not already present), a committed `insert_date.lua`
fixture under `wordcartel/tests/fixtures/plugins/` (or an inline string in the e2e test).

**Tests:**
- **Loaded-but-idle guardrail** (extends the swap SSD-wear family): load a plugin, drive idle
  `Msg::Tick`s through `reduce`, assert **zero** plugin callback invocations (a host-side counter) AND
  `timers::next_wake` unchanged (plugins arm no deadline in P1). Proves "loaded ≠ background work."
- **Contract-invariant tests** (spec §9): the palette-completeness and menu-subset invariants re-run over
  a registry that has plugin entries (a plugin command tagged `menu=Some(Edit)` appears in both palette
  and Edit menu; a `menu=None` one is palette-only). A patch-bound plugin command resolves in
  `build_keymap` and survives a CUA↔WordStar preset switch (law 7).
- **`insert_date.lua` e2e demo** (the success criterion): a fixture plugin registering `date.insert` →
  "Insert Date"; the e2e harness loads it, dispatches via the palette, and asserts the buffer gained a
  date string through `submit_transaction`; a `keymap.patches` binding of `"date.insert"` resolves and
  fires the same command.
- **No-panic property test** (spec §8): drive random `(a, b, text)` from Lua across the range-taking
  surface (`wc.replace`, `wc.text`) → assert **no panic** and buffer coherence.

**Acceptance:** `cargo test -p wordcartel` fully green; `cargo clippy --workspace --all-targets` clean;
`scripts/smoke/run.sh` run and its one-line summary quoted in the report; `module_budgets` green (app.rs
< 1000, plugin/host.rs under its row). The `insert_date.lua` demo passes.

---

## Final gates (after Task 8)

1. **Fable whole-branch review** — cross-task invariants: the borrow-across-Lua invariant holds on every
   loop path (Task 7); both design laws are honored at EVERY input-taking API with no gap (compile probes
   against the branch); no `mlua` type leaked into `registry.rs`/`wordcartel-core`; idle does no plugin
   work. Fable compiles scratch probes against the real branch.
2. **Codex pre-merge gate** — independent GO/NO-GO: spec conformance, the resource/panic laws' completeness
   audit re-checked against the merged code, module budgets, clippy, contract invariants.
Re-run each after fixes until clean/GO. Then merge `--no-ff` to the trunk, verify tests on the merged
result, delete the branch. Push only when asked.

---

## Ledger

Track in `$(git rev-parse --git-path sdd)/progress.md`: one line per completed task + commit range. After
any compaction, trust the ledger + `git log` over recollection; never re-dispatch a task it marks done.
Task 1 (spike) records its findings there (spec §11's seven + two extras) — especially the
`set_memory_limit` verdict (drives whether the VM heap cap ships) and the `!Send`-capture result (the
design's load-bearing assumption).
