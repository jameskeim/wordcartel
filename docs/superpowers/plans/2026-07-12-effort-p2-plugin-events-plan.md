# Effort P2 — implementation plan: plugin events + per-plugin config + reload + wc.command

**Spec:** `docs/superpowers/specs/2026-07-12-effort-p2-plugin-events-design.md` (Codex-clean after 5
rounds, 2026-07-12).
**Branch:** `effort-p2-plugin-events` (already cut).
**Shape:** **No spike** — P1 proved the `mlua` params (`!Send` capture, `set_memory_limit`, `set_hook`
abort, panic→error, named-registry callbacks, `mlua::String::as_bytes` borrowed length). P2 is nine
integration tasks built so the tree stays green after each, ordered with **zero forward dependencies**
(a task depends only on earlier tasks). Subagent-driven, TDD per task (failing test → impl → green →
commit), a per-task reviewer (spec-compliance + quality), then one Fable whole-branch gate + one Codex
pre-merge gate.

Anchor on symbol NAMES (lines drift). `cargo` + `grep` are ground truth, never an editor "unused"/
"undefined" hint (subagent edits are the most stale in an analyzer's view — the controller's diagnostics
about a subagent-touched file lag; verify with `cargo build`/`test` before treating one as real).

**Reconciliation with spec §15 (spec is authoritative).** The spec's task sketch and this plan agree
except: this plan **lands `Registry::retain_builtins` + the run-loop un-freeze FIRST** (spec §15 folds
`retain_builtins` into the reload task but flags in its dependency note that "task 2's fatal path calls
`retain_builtins`, so land it with/before task 2" — this plan resolves that by making it Task 1), and
follows the spec's own ordering rule **config BEFORE events** (spec §15: "4 before 5 — `load_one`'s
signature changes once") rather than the looser suggested spine. Everything else matches the spec.

---

## Global constraints (bind EVERY task — copy into each implementer/reviewer dispatch)

1. **Binding constraints (spec §2 — settled law, NOT open to re-litigate):**
   - **Hooks are OBSERVER-ONLY.** `on_save`/`on_open`/`on_buffer_close` may READ editor state and emit
     status; they may **NOT edit** the document or mutate editor state, and may **NOT call
     `wc.command`**. Enforced mechanically by `InvokeState.observer` (Task 6), not by trust.
   - **Hooks NEVER abort or delay the operation.** The save/open/close has ALREADY happened by the time
     the hook runs (events drain from a queue written after the op). A hook error → status line; a
     runaway hook → time-guard-killed; the op proceeds regardless.
   - **`on_save` fires AFTER a successful write** (`Ok(Saved|Unchanged)`), never on save failure.
   - **Reload = whole-VM teardown.** Plugin Lua state does not persist across `plugins_reload`.
   - **Per-plugin config = opaque Lua table.** The host imposes no schema; the plugin self-validates.
   - **`wc.command`** routes through the SAME `Registry::dispatch` the palette/menu/keys use — never a
     side channel — bounded by the pump chain cap.
2. **The two design LAWS are GATES, not guidelines.**
   - **(a) Input-validation LAW** (spec §3): every plugin API taking a byte offset/range pre-validates it
     against the LIVE buffer via `plugin_check_range` and returns a typed Lua error — no raw plugin
     offset reaches an asserting core primitive. **P2 adds NO offset/range API** (event names, config,
     `wc.command` targets are strings/tables), so this LAW is inherited unchanged; the observer check
     added in front of the edit APIs (Task 6) is strictly additive — it rejects more, never less.
   - **(b) Resource-bound LAW** (spec §7): every plugin-supplied string crossing into a permanent leak or
     a Rust/Lua allocation is bounded BEFORE the allocation. **Concrete pattern — borrowed-length-check-
     then-convert:** extract as `mlua::String` (borrows Lua bytes, no Rust alloc), check
     `.as_bytes().len()` against the cap FIRST, convert only on pass. Applies to every new P2 input:
     `wc.on` event name (parse-to-enum via `event_from_str`), `wc.command` target
     (`PLUGIN_MAX_COMMAND_REF` + queue cap), config string values AND keys
     (`PLUGIN_MAX_CONFIG_STR = 64 KiB`, checked before `lua.create_string`), event payload
     (`PLUGIN_MAX_EVENT_PAYLOAD` via `cap_status`).
3. **`#![forbid(unsafe_code)]` holds.** `Rc<RefCell<>>` + owned captures are safe; `mlua`'s `unsafe`
   stays in the dependency. `wordcartel-core` stays VM-free (no `mlua` import there — ever).
4. **`registry.rs` stays Lua-free** — no `mlua` import. `retain_builtins` is pure Rust; the two new
   builtins (`plugins_reload`/`plugin_list`) are flag-setting / inventory-formatting handlers.
5. **Module-budget / anti-regrowth (GATE).** The budgets count **PRODUCTION lines only** (the lines
   before `#[cfg(test)] mod tests`, per `tests/module_budgets.rs`'s `assert_hub_budget` — NOT raw file
   length; the raw files are much larger incl. tests, e.g. `app.rs` ~4516 / `host.rs` ~605 raw — do not
   confuse the two in review). `app.rs` ≤ **1000** production lines (currently **886**). `plugin/host.rs`
   ≤ **400** production lines (currently **203**). New logic lives in
   `wordcartel/src/plugin/`; hooks/reload enter through **seams** (`fire_event`, `perform_reload`, the
   pump, a `load_phase` extraction), NOT bulk in `reduce`/`run`. `app.rs` net delta is a **target ≤ 0**,
   ENFORCED by the gate — achieved by extracting the inline startup load block into
   `plugin/reload.rs::load_phase` (Task 8) and keeping the reload seam a guarded CALL. If `plugin/host.rs`
   would exceed 400, extract `plugin/events.rs` rather than bump the budget. `clippy::too_many_lines`
   (threshold 100) binds every new fn — the pump re-drain loop is a thin loop over delegate methods.
6. **Command-surface-contract conformance (GATE, spec §9).** See the dedicated section below; each
   relevant task restates how it conforms.
7. **House style** (CLAUDE.md): dense hand-formatting, `—` em-dashes never `--`, no emoji, doc-comment
   every public item, snake_case/PascalCase/SCREAMING_SNAKE. **Do NOT run `cargo fmt`.** Match neighbors.
8. **Errors → status line, never console.** `print_*`/`dbg!` are deny-lints. All plugin errors route
   through the `plugin::plugin_error` seam → `editor.status`.
9. **GATES before merge:** `cargo test` green (both crates); `cargo build` + `cargo test --no-run`
   warning-free for touched crates; `cargo clippy --workspace --all-targets` clean. Run
   `scripts/smoke/run.sh` in the pre-merge report (mandatory-run / advisory-pass, quote its one-line
   summary). `cargo deny check` at release-checklist time (not a merge gate).
10. **Commit per task** with project trailers (Co-Authored-By: Claude Opus 4.8 + Claude-Session).
    Message form `feat(p2): …` / `test(p2):` / `chore(p2):` / `refactor(p2):` as fits.
11. **The two ⚠OPEN flags are DECIDED A/A — bake in, do NOT re-open:** (1) `on_open` does NOT synthesize
    a startup event — it fires only for in-session opens; (2) `plugins_reload` re-reads the `[plugins]`
    config section (only that section) at each reload.

---

## Command-surface-contract conformance (spec §9 — the plan honors it, task by task)

- **Law 1 (registry = single source of truth).** `plugins_reload`/`plugin_list` are ordinary
  `Registry::builtins()` entries (Task 8). `wc.command` resolves via `reg.resolve_name` + dispatches via
  `reg.dispatch` — the identical path palette/menu/keys use, no call-time name-set snapshot (Task 7).
  Registry mutation (`retain_builtins` + reload rebuild) happens ONLY at the between-reduces reload seam
  (Task 8), never while dispatch is live.
- **Law 2 (every user-settable option is a command).** `[plugins].dir`/`disable`/`enabled`/
  `[plugins.config.<name>]` are config-layer load machinery (the `keymap.patches`/`theme.file` class),
  NOT `SettingsSnapshot` runtime options — so the law-2 recurrence-guard test is unaffected; the runtime
  verb over them is `plugins_reload` (a real command that re-reads `[plugins]`).
- **Law 3 (palette exhaustive).** The two new builtins appear in the palette by derivation
  (`palette.rs` iterates `reg.commands()`); Task 9 re-runs palette-completeness over a post-reload
  registry.
- **Law 4 (menu ⊆ palette).** `plugins_reload`/`plugin_list` are tagged `MenuCategory::Settings`
  (browse-for plugin management); both are palette entries, so the subset holds by derivation. No dynamic
  sections in P2.
- **Laws 5/6.** Mouse path falls out of menu placement; reload's state changes flow through existing
  shared flags (`keymap_rebuild`, `editor.status`) + `register_plugin`/`retain_builtins` — no bypass.
- **Law 7 (hints track the active keymap).** After reload sets `keymap_rebuild`, the existing
  `rebuild_keymap_if_requested` arm re-resolves patch-bound plugin commands (Task 8); Task 9 adds a
  reload hints-re-resolution case.
- **No contract amendment required** — events/config/`wc.config` are host↔plugin data flow, not
  command-surface actors.

---

## Task 1 — foundation: `Registry::retain_builtins` + un-freeze `reg` in the run loop

The reload machinery's registry half, landed first (spec §15 dependency note: Task 2's fatal path calls
`retain_builtins`). Pure Rust, fully unit-testable; no Lua, no behavior change at runtime (nothing calls
`retain_builtins` yet).

**Model:** most-capable (the index-rebuild invariant is load-bearing). **Files:**
`wordcartel/src/registry.rs`, `wordcartel/src/app.rs` (the `let reg = reg;` freeze line only).

**TDD first (in `registry.rs` tests):**
- `retain_builtins_keeps_builtins_and_drops_plugins` — build `Registry::builtins()`, `register_plugin`
  two plugin ids, `retain_builtins()`, then: both plugin ids `resolve_name` → `None`; a known builtin
  (`"save"`) still resolves and `dispatch`es; `commands().count()` equals the pre-plugin builtin count.
- `retain_builtins_reindexes_so_a_reregister_succeeds` — after `retain_builtins`, `register_plugin` the
  SAME id that was dropped → `Ok(())` (no ghost `index` entry), and it resolves + dispatches.
- `retain_builtins_preserves_builtin_order` — the `commands()` id sequence for builtins is identical
  before any plugin registration and after `register_plugin`+`retain_builtins` (palette stability).

**Implementation:**
- `registry.rs` — add after `register_plugin`:
  ```rust
  /// Remove every `Plugin` entry, keeping builtins — the reload teardown's registry half (P2 §6b).
  /// Fully rebuilds `index` from the surviving `entries`: removing an interior entry shifts every
  /// later position, so the old indices are wholesale invalid — never patch them incrementally.
  ///
  /// # Examples
  /// ```
  /// # use wordcartel::registry::{Registry, CommandId};
  /// let mut r = Registry::builtins();
  /// r.register_plugin(CommandId("demo.hi"), "Hi", None).unwrap();
  /// r.retain_builtins();
  /// assert!(r.resolve_name("demo.hi").is_none());
  /// assert!(r.resolve_name("save").is_some());
  /// ```
  pub fn retain_builtins(&mut self) {
      // `matches!(&e.handler, …)` borrows the discriminant — HandlerKind is NOT Copy, so
      // `matches!(e.handler, …)` would try to move the field out of the `&CommandEntry` and fail.
      self.entries.retain(|e| matches!(&e.handler, HandlerKind::Builtin(_)));
      self.index.clear();
      for (i, e) in self.entries.iter().enumerate() {
          self.index.insert(e.id, i);
      }
  }
  ```
- `app.rs` (`run`) — delete the freeze line **`let reg = reg;` at `app.rs:660`** (the SOLE match from
  `grep -rn "let reg = reg" wordcartel/src`; it sits right after the plugin load block, before
  `build_keymap`). The binding stays `let mut reg` from construction (grep the `let mut reg =
  Registry::builtins();` a few lines above). `reduce`, the palette, the menu, `build_keymap` keep `&reg`.
  `mut` remains "used" (the load block's `load_sources(&mut reg, …)` mutates it), so no `unused_mut`
  warning. No other run-loop change in this task.

**Acceptance:** `cargo test -p wordcartel registry::` green; existing registry tests still pass; `cargo
build`/`test --no-run` warning-free; clippy clean; app runs unchanged.

**Contract:** touches the registry structurally only; palette/menu derivation unchanged.

---

## Task 2 — two-phase atomic commit: intern-on-commit + callback-key-on-commit + owned-stem (§7b)

The §7b rework, landed as one change (the intern leak and the callback-key overwrite are the same
non-atomicity). `PendingReg` carries the raw strings + the `mlua::Function`; `install_registration`
performs NO intern / NO `set_named_registry_value` during exec; `load_one` commits in two phases
(fallible Lua/intern writes first, infallible `register_plugin` second); a commit-time `mlua` error is a
fatal VM-exhaustion event that reverts the registry (`retain_builtins`, from Task 1) + nulls the host.

**Model:** most-capable (the atomicity invariant is the effort's sharpest correctness edge). **Files:**
`wordcartel/src/plugin/host.rs` (`PendingReg`), `wordcartel/src/plugin/api.rs` (`install_registration`
owned-stem), `wordcartel/src/plugin/load.rs` (`load_one` two-phase + `load_sources` `&mut PluginHost` +
fatal revert).

**TDD first (in `plugin/load.rs` tests — extend the existing suite):**
- `load_validation_failure_interns_nothing_and_writes_no_callback_key` — a plugin whose 2nd
  `register_command` collides (same name twice): assert `reg.commands().count()` unchanged AND the intern
  pool size unchanged (snapshot via a `#[cfg(test)]` `intern_pool_len()` helper on `plugin/mod.rs`) AND
  no `wc-cmd-atomic.x` key exists in the VM (`host.lua().unwrap().named_registry_value::<mlua::Function>
  ("wc-cmd-atomic.x").is_err()`).
- `commit_time_exhaustion_reverts_registry_and_nulls_host` — the **two-plugin** case: plugin `a` commits
  cleanly, then plugin `b`'s fallible commit phase is forced to `Err` via the `#[cfg(test)]` fault seam
  (below). Assert: `a`'s command `resolve_name("a.cmd")` → **`None`** (A's entry gone, not just B's), the
  registry is builtins-only (`commands().count()` == builtins count), and `host.has_vm()` is `false`.
- Keep the existing `load_is_atomic_per_plugin`, `load_reports_collision`, `load_rejects_*`,
  `load_registers_command_into_registry`, `load_parse_error_is_reported_not_fatal` green (the two-phase
  refactor must be transparent to them).

**Implementation:**
- `plugin/mod.rs` — add a `#[cfg(test)]` helper for the pool-size assertion:
  ```rust
  #[cfg(test)]
  pub(crate) fn intern_pool_len() -> usize {
      // Mirrors `intern`'s POOL; a test-only reader so a guardrail can assert "leaked nothing".
      // (Implement by exposing the count from the same static — e.g. a `POOL.lock()…map_or(0,len)`.)
      ...
  }
  ```
  (The implementer wires this against the real `intern` `static POOL` — a read-only `len()`.)
- `plugin/host.rs` — `PendingReg` gains the raw strings + the callback:
  ```rust
  /// One command a plugin's `wc.register_command` staged during exec — raw strings + the callback,
  /// interned/registry-written ONLY at commit (P2 §7b two-phase). Carrying an `mlua::Function` is
  /// sound: it is a GC-rooted handle into the loader's own VM, dropped if the plugin fails preflight.
  pub struct PendingReg {
      pub name_full: String,           // "<stem>.<name>" — raw, cap-checked, NOT yet interned
      pub label: String,               // raw, cap-checked
      pub menu: Option<MenuCategory>,
      pub func: mlua::Function,        // stored under wc-cmd-<id> only on commit
  }
  ```
  Add a `#[cfg(test)]` fault-injection seam for the exhaustion test (deterministic, mirrors M3's `Fs`
  fault seam):
  ```rust
  #[cfg(test)]
  thread_local! {
      /// When set, the NEXT fallible commit write in `load_one` returns a synthetic mlua error —
      /// exercises the commit-time-exhaustion fatal path without exhausting real memory.
      pub(crate) static FAIL_NEXT_COMMIT_WRITE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
  }
  ```
- `plugin/api.rs` — `install_registration` captures an **owned `stem: String`** and does NO intern / NO
  registry write during exec; it only cap-checks and pushes a raw `PendingReg`:
  ```rust
  /// Install the `wc` registration surface for ONE plugin's exec pass. `stem` (owned — moved into
  /// the 'static closure; an owned String is 'static, so this compiles without the `send` feature,
  /// exactly as the former `&'static str` did) fixes the namespace. Every plugin string is
  /// cap-checked on its BORROWED `mlua::String` bytes (resource-bound LAW) and pushed RAW into
  /// `sink`; interning + the `wc-cmd-<id>` callback write happen ONLY at commit (load_one), so a
  /// plugin that fails preflight leaks nothing and overwrites no live callback key (§7b).
  pub(crate) fn install_registration(
      lua: &mlua::Lua,
      stem: String,                                  // was &'static str
      sink: Rc<RefCell<Vec<PendingReg>>>,
      count: Rc<Cell<usize>>,
  ) -> mlua::Result<()> {
      let wc = wc_table(lua)?;
      let reg_fn = lua.create_function(move |_lua, spec: mlua::Table| {
          if count.get() >= crate::limits::PLUGIN_MAX_COMMANDS_PER_PLUGIN {
              return Err(mlua::Error::runtime("plugin: too many commands (max 256)"));
          }
          let name_raw: mlua::String = spec.get("name")?;
          if name_raw.as_bytes().len() > crate::limits::PLUGIN_MAX_NAME_LEN {
              return Err(mlua::Error::runtime("plugin: command name too long"));
          }
          let label_raw: mlua::String = spec.get("label")?;
          if label_raw.as_bytes().len() > crate::limits::PLUGIN_MAX_LABEL_LEN {
              return Err(mlua::Error::runtime("plugin: label too long"));
          }
          let menu_raw: Option<mlua::String> = spec.get("menu")?;
          let func: mlua::Function = spec.get("fn")?;
          let menu = match &menu_raw {
              None => None,
              Some(m) => Some(menu_from_str(m.to_str()?.as_ref())
                  .ok_or_else(|| mlua::Error::runtime("plugin: unknown menu value"))?),
          };
          // No intern, no set_named_registry_value here — commit does both (§7b). Own raw strings only.
          let name_full = format!("{stem}.{}", name_raw.to_str()?.as_ref());
          let label = label_raw.to_str()?.to_owned();
          count.set(count.get() + 1);
          sink.borrow_mut().push(PendingReg { name_full, label, menu, func });
          Ok(())
      })?;
      wc.set("register_command", reg_fn)?;
      Ok(())
  }
  ```
- `plugin/load.rs` — a distinguished failure type + the two-phase `load_one` + `load_sources` taking
  `&mut PluginHost` and doing the registry-level fatal revert. `load_one` no longer interns the stem
  before exec:
  ```rust
  /// Why loading ONE plugin failed. `Validation` → skip this plugin, batch continues (P1 isolation).
  /// `VmExhausted` → a commit-time mlua write failed: the VM is exhausted, fatal for the whole
  /// load_phase (§7b) — the caller nulls the host + reverts the registry.
  pub enum LoadFailure { Validation(String), VmExhausted(String) }

  fn load_one(reg: &mut Registry, lua: &mlua::Lua, stem_raw: &str, src: &str)
      -> Result<usize, LoadFailure> {
      if stem_raw.len() > PLUGIN_MAX_STEM_LEN {
          return Err(LoadFailure::Validation(format!(
              "plugin stem too long ({} bytes, max {PLUGIN_MAX_STEM_LEN})", stem_raw.len())));
      }
      let sink: Rc<RefCell<Vec<PendingReg>>> = Rc::new(RefCell::new(Vec::new()));
      let count = Rc::new(Cell::new(0usize));
      // NOTE: stem is OWNED into the closure now; set_name/collision-check take &str, so nothing
      // before commit needs &'static (the stem intern moved into the commit phase below).
      // `install_registration` has exactly ONE caller — this line (grep `install_registration\b`:
      // plugin/load.rs:140 is the sole non-doc call site) — so the `stem: &'static str → String`
      // signature change touches only here.
      crate::plugin::api::install_registration(lua, stem_raw.to_owned(), sink.clone(), count.clone())
          .map_err(|e| LoadFailure::Validation(format!("plugin {stem_raw}: {e}")))?;
      lua.load(src).set_name(stem_raw).exec()
          .map_err(|e| LoadFailure::Validation(format!("plugin {stem_raw}: {e}")))?;

      let pending: Vec<PendingReg> = sink.borrow_mut().drain(..).collect();
      if pending.len() > PLUGIN_MAX_COMMANDS_PER_PLUGIN {
          return Err(LoadFailure::Validation(format!(
              "plugin {stem_raw}: too many commands ({}, max {PLUGIN_MAX_COMMANDS_PER_PLUGIN})",
              pending.len())));
      }
      // Preflight on RAW strings — every id free of the live registry AND unique in this batch.
      let mut seen = std::collections::HashSet::new();
      for p in &pending {
          if reg.resolve_name(&p.name_full).is_some() || !seen.insert(p.name_full.as_str()) {
              return Err(LoadFailure::Validation(format!(
                  "plugin {stem_raw}: duplicate command id {}", p.name_full)));
          }
      }
      // ── Commit phase 1 (FALLIBLE): intern + write every wc-cmd-<id> key, Registry untouched. ──
      let mut committed: Vec<(CommandId, &'static str, Option<MenuCategory>)> =
          Vec::with_capacity(pending.len());
      for p in &pending {
          let id = CommandId(crate::plugin::intern(&p.name_full));
          let label: &'static str = crate::plugin::intern(&p.label);
          // Fault seam: deterministic exhaustion test (no real OOM needed).
          #[cfg(test)]
          if crate::plugin::host::FAIL_NEXT_COMMIT_WRITE.with(|c| c.replace(false)) {
              return Err(LoadFailure::VmExhausted(format!("plugin {stem_raw}: VM exhausted (test)")));
          }
          lua.set_named_registry_value(&format!("wc-cmd-{}", id.0), p.func.clone())
              .map_err(|e| LoadFailure::VmExhausted(format!("plugin {stem_raw}: {e}")))?;
          committed.push((id, label, p.menu));
      }
      // ── Commit phase 2 (INFALLIBLE): Registry mutation only — preflight ruled out Duplicate. ──
      let n = committed.len();
      for (id, label, menu) in committed {
          reg.register_plugin(id, label, menu)
              .expect("preflight already ruled out every possible Duplicate for this plugin");
      }
      Ok(n)
  }

  /// Filesystem-free load core. `host` is `&mut` so a commit-time VM-exhaustion (LoadFailure::VmExhausted)
  /// can null the VM; on that path the registry is ALSO reverted to builtins-only (`retain_builtins`) so
  /// no earlier-committed plugin is left pointing at a dead VM (§7b — the registry half of the fatal
  /// revert; the editor half, queue-clear + keymap_rebuild, is the reload seam's job, Task 8).
  pub fn load_sources(reg: &mut Registry, host: &mut PluginHost, sources: &[(String, String)])
      -> Vec<LoadReport> {
      if host.lua().is_none() { return Vec::new(); }
      let mut reports = Vec::new();
      let mut fatal = false;
      for (stem_raw, src) in sources {
          let lua = host.lua().expect("checked above"); // per-iteration borrow, released before loop end
          let outcome = crate::panicx::catch(|| load_one(reg, lua, stem_raw, src))
              .unwrap_or_else(|panic_msg| Err(LoadFailure::Validation(panic_msg)));
          match outcome {
              Ok(n) => reports.push(LoadReport { plugin: stem_raw.clone(), result: Ok(n) }),
              Err(LoadFailure::Validation(msg)) =>
                  reports.push(LoadReport { plugin: stem_raw.clone(), result: Err(msg) }),
              Err(LoadFailure::VmExhausted(msg)) => {
                  reports.push(LoadReport { plugin: stem_raw.clone(), result: Err(msg) });
                  fatal = true;
                  break; // stop the batch — the VM is unusable
              }
          }
      }
      if fatal {
          *host = PluginHost::null(); // drops the whole VM + every wc-cmd-* key/closure
          reg.retain_builtins();      // registry-level revert — discards EVERY plugin committed so far
      }
      reports
  }
  ```
  (`LoadReport { plugin: String, result: Result<usize, String> }` is **unchanged in this task** — the
  caller sees a command count or a string reason either way; the `LoadFailure` distinction is internal to
  steer the fatal revert. Task 6 adds a `hooks: usize` field to `LoadReport` when hooks exist — noted so a
  reviewer expects that later, not here.)
- **Migrate EVERY `load_sources` call site to `&mut host`** — the COMPLETE list, from
  `grep -rn "load_sources(" wordcartel/src` (verified 2026-07-12: **21 callers + the definition** at
  `load.rs:102` = 22 matches; the enumerated list below sums to 21). Each currently passes `&host`/`&h`
  (immutable) → flip to `&mut host`/`&mut h`; where the local is not already `mut`, make it `mut`:
  - **Production (1):** `app.rs:646` startup load block (`load_sources(&mut reg, &h, …)` → bind
    `Ok(mut h)` and pass `&mut h`; after the call, if `!h.has_vm()` the fatal revert already reverted
    `reg` — push `"plugins disabled: VM exhausted during load"`; the startup editor has empty queues +
    an unbuilt keymap, so no editor-side cleanup is needed here — that is reload-only, Task 8). Note:
    Task 5 further changes this site to call `reload::load_phase`, so it is rewritten again there.
  - **Contract-invariant tests (3) — `let mut host` already exists at each, only the arg flips:**
    `palette.rs:276`, `menu.rs:472`, `menu.rs:509` (the P1 law-3/law-4/law-7 tests that load a plugin
    into a registry to prove palette-completeness / menu-subset / keymap re-resolution — they call
    `load_sources(&mut reg, &host, …)`).
  - **e2e (1):** `e2e.rs:116` `new_with_plugin` (`load_sources(&mut reg, &host, &srcs)` → `&mut host`).
  - **`plugin/host.rs` tests (5):** the `make` helper at `host.rs:230`, plus `:403`, `:492`, `:520`,
    `:574` (proptest) — all `load_sources(&mut reg, &host, …)`; flip to `&mut host` (each `host` is
    already `let mut host`).
  - **In-module `plugin/load.rs` tests (11) — MINOR (listed for completeness; the compiler catches them):**
    `load.rs:229, 247, 259, 271, 283, 296, 307, 317, 328, 346, 358` (all `load_sources(&mut reg, &host,
    …)`; flip to `&mut host` — the `let host = PluginHost::new().unwrap()` locals become `let mut host`).
  Task 5 (config) adds the `config_map` + `&mut warns` params to `load_sources`, forcing a SECOND edit at
  every one of these sites — Task 5's migration list re-enumerates them; keep the two edits in mind so a
  reviewer expects both.

**Acceptance:** `cargo test -p wordcartel plugin::` green (new atomicity + exhaustion tests + all P1
load tests); warning-free; clippy clean. The `intern_pool_len` guardrail proves a failed load leaks zero.

**Contract:** N/A — does not touch the command surface (registration internals only).

---

## Task 3 — load-phase runaway guard (§7a)

Guard `load_one`'s `exec()` with a distinct, larger time budget so a `while true do end` at a plugin's
top level cannot hang startup before the terminal guard exists. Small, self-contained.

**Model:** standard. **Files:** `wordcartel/src/plugin/host.rs` (budget-parameterize `with_time_guard` +
`LOAD_TIME_BUDGET`), `wordcartel/src/plugin/load.rs` (wrap `exec()`).

**TDD first:**
- `plugin::load::tests::load_time_budget_aborts_runaway_toplevel` — a plugin whose source is
  `while true do end` → its `LoadReport.result` is `Err` (mentions a budget), the batch continues (a
  second good plugin in the same batch still registers). Drive it with a **low test budget** — see the
  budget-injection note.
- `plugin::host::tests::callback_budget_constant_unchanged` — `CALLBACK_TIME_BUDGET` is still 150 ms
  (the rename didn't move it).

**Implementation:**
- `plugin/host.rs`:
  - Rename the existing `const TIME_BUDGET` → `pub(crate) const CALLBACK_TIME_BUDGET: Duration =
    Duration::from_millis(150);` (grep + update `with_time_guard`'s use).
  - Add `pub(crate) const LOAD_TIME_BUDGET: Duration = Duration::from_secs(1);` with a doc comment: an
    order of magnitude over the callback budget for legitimate init table-building; worst case per hung
    plugin ~1 s (reported, survivable — the old behavior was ∞).
  - Make the guard budget-parameterized and reachable from the load layer as a free fn:
    ```rust
    /// The `set_hook` runaway guard, parameterized by budget so load (LOAD_TIME_BUDGET) and callbacks
    /// (CALLBACK_TIME_BUDGET) share one mechanism. RAII HookGuard removes the hook on return AND unwind.
    pub(crate) fn with_time_guard<T>(lua: &mlua::Lua, budget: Duration, f: impl FnOnce() -> mlua::Result<T>)
        -> mlua::Result<T> {
        let start = std::time::Instant::now();
        lua.set_hook(mlua::HookTriggers::new().every_nth_instruction(10_000), move |_lua, _dbg| {
            if start.elapsed() > budget {
                Err(mlua::Error::runtime("plugin: exceeded time budget"))
            } else { Ok(mlua::VmState::Continue) }
        });
        let _guard = HookGuard(lua);
        f()
    }
    ```
    The existing `PluginHost::with_time_guard` method becomes a thin forwarder to this free fn with
    `CALLBACK_TIME_BUDGET` (or the pump calls the free fn directly). **Test-budget injection:** add a
    `#[cfg(test)]` override so `load_one` can be driven with a tiny load budget — either a
    `#[cfg(test)] thread_local! LOAD_BUDGET_OVERRIDE: Cell<Option<Duration>>` consulted by `load_one`, or
    a `load_one`-internal `load_budget()` fn that reads it. (Mirrors the callback-budget test pattern
    already used in `host.rs` isolation tests.)
- `plugin/load.rs` — wrap the exec in `load_one` (grep `lua.load(src).set_name(stem_raw).exec()`):
  ```rust
  crate::plugin::host::with_time_guard(lua, load_budget(), || lua.load(src).set_name(stem_raw).exec())
      .map_err(|e| LoadFailure::Validation(format!("plugin {stem_raw}: {e}")))?;
  ```
  where `load_budget()` returns `LOAD_TIME_BUDGET` in release and the test override when set. A tripped
  guard is a normal `exec` `Err` → `LoadFailure::Validation` → that plugin skipped, batch continues.

**Acceptance:** `cargo test -p wordcartel plugin::` green (runaway-abort test + batch-continues);
warning-free; clippy clean.

**Contract:** N/A.

---

## Task 4 — same-stem dedup in `discover` (§7c)

Ambiguous `foo.lua` + `foo/init.lua` → load NEITHER, one loud report. `discover`-local; no VM.

**Model:** standard. **Files:** `wordcartel/src/plugin/load.rs`.

**TDD first (in `plugin/load.rs` tests):**
- `discover_rejects_ambiguous_same_stem` — a tempdir with BOTH `foo.lua` and `foo/init.lua`: assert the
  stem `"foo"` is absent from `disc.sources`, `disc.skipped` has exactly ONE report whose `plugin ==
  "foo"` and whose error mentions "ambiguous"/"remove one", and an unrelated `bar.lua` still loads.
- Keep `discover_reads_single_file_and_dir` (different stems `a`/`b` — no collision) green.

**Implementation:**
- `plugin/load.rs` `discover` — after the existing lexicographic `candidates.sort_by(|a,b| a.0.cmp(&b.0))`
  and before the per-candidate read loop, fold adjacent equal stems: a stem appearing more than once
  becomes ONE `skipped` `LoadReport` (`"ambiguous plugin '<stem>': both <stem>.lua and <stem>/init.lua
  exist — remove one"`) and is excluded from `sources`; disabled stems are still dropped silently first.
  Concretely, group the sorted candidates by stem; a group of size 1 → read + push to `sources` (existing
  path); a group of size ≥ 2 → push one ambiguous `skipped` report, read nothing. (One report per stem,
  matching `skipped`'s one-report-per-outcome convention — never one per colliding file.)

**Acceptance:** `cargo test -p wordcartel plugin::load` green; warning-free; clippy clean.

**Contract:** N/A.

---

## Task 5 — per-plugin config `[plugins.config.<name>]` + `[plugins].dir` + `wc.config` + 64 KiB cap (§4)

Config plumbing BEFORE events so `load_one`'s signature grows once for config (spec §15 "4 before 5").
Adds `RawPlugins.dir`/`config`, `PluginsConfig.dir`/`config`, `plugin/settings.rs` (TOML→Lua under caps),
`wc.config` install-at-load + clear-at-attach, and the new config limits.

**Model:** most-capable (the byte-cap-before-alloc LAW + the flatten-collision resolution). **Files:**
`wordcartel/src/config.rs`, `wordcartel/src/plugin/settings.rs` (new), `wordcartel/src/plugin/mod.rs`
(`pub mod settings;`), `wordcartel/src/plugin/load.rs` (`load_one`/`load_sources` config param),
`wordcartel/src/plugin/api.rs` (`wc.config` clear in `install_editor_api`), `wordcartel/src/limits.rs`.

**TDD first:**
- `config::tests::plugins_config_namespaced_parses` — `[plugins.config.wordcount]\nmin_words = 100` folds
  into `cfg.plugins.config["wordcount"]`; a plugin literally named `[plugins.config.dir]` parses too (no
  collision with the typed `dir` field — the namespacing regression guard); default `config` is empty.
- `config::tests::plugins_dir_parses` — `[plugins]\ndir = "/x/y"` → `cfg.plugins.dir == Some(PathBuf)`.
- `config::tests::plugins_config_replaces_per_layer` — a higher layer's `[plugins.config.foo]` wholly
  replaces a lower layer's for `foo`, leaving `bar`'s untouched.
- `plugin::settings::tests::config_to_lua_scalars_and_nesting` — a `toml::Value` table with
  string/int/bool/array/nested-table converts to the matching Lua shapes.
- `plugin::settings::tests::config_to_lua_rejects_over_byte_cap` — a string value of
  `PLUGIN_MAX_CONFIG_STR + 1` bytes → `Err`; likewise an over-cap KEY → `Err`.
- `plugin::settings::tests::config_to_lua_rejects_over_depth_and_over_nodes` — depth >
  `PLUGIN_MAX_CONFIG_DEPTH` and node count > `PLUGIN_MAX_CONFIG_NODES` each → `Err`.
- `plugin::load::tests::config_reaches_wc_config` — `load_sources` with a per-plugin config value → the
  plugin's `wc.config.min_words` reads back inside its `register_command` fn (assert via a status the
  command sets); a plugin with no config sees `wc.config == nil`; an over-cap config → the plugin still
  loads (its command registers) with `wc.config == nil`.

**Implementation:**
- `limits.rs` — add:
  ```rust
  /// Max nesting depth converted from [plugins.config.<name>] into a Lua table.
  pub const PLUGIN_MAX_CONFIG_DEPTH: usize = 8;
  /// Max total nodes (keys + values) converted from one plugin's config table.
  pub const PLUGIN_MAX_CONFIG_NODES: usize = 1024;
  /// Max byte length of any single config string VALUE or table KEY — the pre-allocation byte bound
  /// (resource-bound LAW) that depth+node counts miss: config::load reads the source unbounded, so one
  /// giant string/key must be rejected BEFORE lua.create_string allocates it.
  pub const PLUGIN_MAX_CONFIG_STR: usize = 64 * 1024;
  ```
  Extend `plugin_caps_are_sane` over the three new constants.
- `config.rs` — `RawPlugins` gains `dir: Option<PathBuf>` and `config: BTreeMap<String, toml::Value>`
  (namespaced, NOT `#[serde(flatten)]` — resolves the typed-field collision; a plugin named
  `enabled`/`disable`/`dir` is valid because its config lives under `[plugins.config.<name>]`);
  `PluginsConfig` gains `pub dir: Option<PathBuf>` and `pub config: BTreeMap<String, toml::Value>`
  (default empty). Fold in `load` beside the existing `raw.plugins.enabled`/`disable` arms:
  ```rust
  if let Some(v) = raw.plugins.dir { cfg.plugins.dir = Some(v); }
  for (k, v) in raw.plugins.config { cfg.plugins.config.insert(k, v); } // per-name REPLACE per layer
  ```
- `plugin/settings.rs` (new) — the conversion under caps:
  ```rust
  //! `[plugins.config.<name>]` TOML → an opaque `mlua::Value` for `wc.config`, bounded by the
  //! resource-bound LAW's config caps (depth/nodes/byte — checked BEFORE the Lua allocation).
  use crate::limits::{PLUGIN_MAX_CONFIG_DEPTH, PLUGIN_MAX_CONFIG_NODES, PLUGIN_MAX_CONFIG_STR};

  /// Convert one plugin's config value to a Lua value under the three caps. `Err(reason)` on any cap;
  /// the caller then hands the plugin `wc.config = nil` + a warning (plugin still loads).
  pub(crate) fn config_to_lua(lua: &mlua::Lua, v: &toml::Value) -> Result<mlua::Value, String> {
      let mut nodes = 0usize;
      convert(lua, v, 1, &mut nodes)
  }

  fn convert(lua: &mlua::Lua, v: &toml::Value, depth: usize, nodes: &mut usize)
      -> Result<mlua::Value, String> {
      if depth > PLUGIN_MAX_CONFIG_DEPTH { return Err(format!("config nesting deeper than {PLUGIN_MAX_CONFIG_DEPTH}")); }
      *nodes += 1;
      if *nodes > PLUGIN_MAX_CONFIG_NODES { return Err(format!("config exceeds {PLUGIN_MAX_CONFIG_NODES} nodes")); }
      Ok(match v {
          toml::Value::String(s) => {
              if s.len() > PLUGIN_MAX_CONFIG_STR { return Err(format!("config string exceeds {PLUGIN_MAX_CONFIG_STR} bytes")); }
              mlua::Value::String(lua.create_string(s).map_err(|e| e.to_string())?) // alloc AFTER the cap
          }
          toml::Value::Integer(i) => mlua::Value::Integer(*i),
          toml::Value::Float(f) => mlua::Value::Number(*f),
          toml::Value::Boolean(b) => mlua::Value::Boolean(*b),
          toml::Value::Datetime(d) => {
              let s = d.to_string();
              if s.len() > PLUGIN_MAX_CONFIG_STR { return Err("config datetime string too long".into()); }
              mlua::Value::String(lua.create_string(&s).map_err(|e| e.to_string())?)
          }
          toml::Value::Array(a) => {
              let t = lua.create_table().map_err(|e| e.to_string())?;
              for (i, item) in a.iter().enumerate() {
                  let lv = convert(lua, item, depth + 1, nodes)?;
                  t.set(i + 1, lv).map_err(|e| e.to_string())?; // Lua 1-based sequence
              }
              mlua::Value::Table(t)
          }
          toml::Value::Table(map) => {
              let t = lua.create_table().map_err(|e| e.to_string())?;
              for (k, val) in map {
                  if k.len() > PLUGIN_MAX_CONFIG_STR { return Err(format!("config key exceeds {PLUGIN_MAX_CONFIG_STR} bytes")); }
                  *nodes += 1;
                  if *nodes > PLUGIN_MAX_CONFIG_NODES { return Err(format!("config exceeds {PLUGIN_MAX_CONFIG_NODES} nodes")); }
                  let lv = convert(lua, val, depth + 1, nodes)?;
                  t.set(k.as_str(), lv).map_err(|e| e.to_string())?;
              }
              mlua::Value::Table(t)
          }
      })
  }

  /// Install `wc.config` for ONE plugin's exec pass (its converted table, or nil). Cleared at
  /// attach_bridge (install_editor_api) so it does not linger on the shared `wc` global.
  pub(crate) fn install_config(lua: &mlua::Lua, value: mlua::Value) -> mlua::Result<()> {
      crate::plugin::api::wc_table(lua)?.set("config", value)
  }
  ```
  (**MANDATORY visibility change:** `api::wc_table` is currently `fn wc_table(...)` — module-private
  (`plugin/api.rs:20`). `plugin/settings.rs::install_config` is a SIBLING module, so it cannot call it
  as-is. Change `fn wc_table` → `pub(crate) fn wc_table` in `api.rs`. Its two in-module callers
  (`api.rs:49`, `:166`) are unaffected.)
- `plugin/load.rs` — signatures grow ONCE for config (spec §15 "4 before 5"): `load_one` gains
  `config: Option<&toml::Value>` and `warns: &mut Vec<String>` (the warning channel — see below);
  `load_sources` gains `config_map: &BTreeMap<String, toml::Value>` and `warns: &mut Vec<String>`, looks
  up each plugin's value by stem (`config_map.get(stem_raw)`), and passes it + `warns` down. Before
  `exec`, `load_one` installs `wc.config`:
  ```rust
  // In load_one, after install_registration and BEFORE exec (config: Option<&toml::Value>,
  // warns: &mut Vec<String> are params):
  let cfg_value = match config {
      Some(v) => match crate::plugin::settings::config_to_lua(lua, v) {
          Ok(lv) => lv,
          Err(reason) => {                         // over-cap → nil, plugin STILL loads, but WARN loudly
              warns.push(format!("plugin {stem_raw}: [plugins.config.{stem_raw}] ignored — {reason}"));
              mlua::Value::Nil
          }
      },
      None => mlua::Value::Nil,
  };
  crate::plugin::settings::install_config(lua, cfg_value)
      .map_err(|e| LoadFailure::Validation(format!("plugin {stem_raw}: {e}")))?;
  ```
  **Config-cap warning surfacing — resolved concretely:** an over-cap config must WARN, not silently nil.
  `load_one` returns `Result<usize, LoadFailure>` (no warning channel in the return), so the warning
  flows through the `&mut Vec<String> warns` param threaded `load_sources → load_one` (mechanical — the
  same `warns` the caller already owns for skipped/failed plugins). `load_sources` signature becomes
  `load_sources(reg: &mut Registry, host: &mut PluginHost, sources: &[(String,String)], config_map:
  &BTreeMap<String, toml::Value>, warns: &mut Vec<String>) -> Vec<LoadReport>`.
- `plugin/api.rs` — in `install_editor_api` (the attach-time installer), CLEAR `wc.config` to nil beside
  the existing `install_registration_closed` call: `wc_table(lua)?.set("config", mlua::Value::Nil)?;`
  (add an `install_config_cleared` helper mirroring `install_registration_closed`, with a doc note: the
  last-loaded plugin's table must not linger on the shared `wc` global for callbacks; a plugin that wants
  its config at callback time captured it in a Lua local at load).
- **Migrate EVERY `load_sources` call site AGAIN** (this is the SECOND edit — Task 2 flipped `&host` →
  `&mut host`; Task 5 adds the trailing `config_map: &BTreeMap<String, toml::Value>` +
  `warns: &mut Vec<String>` args). Full list (same `grep -rn "load_sources(" wordcartel/src` set):
  - `app.rs:646` startup load → pass `&cfg.plugins.config` + `&mut warns` (this site is then folded into
    `reload::load_phase`, which owns the config/warns plumbing — see Task 8; net, `app.rs` calls
    `load_phase`, not `load_sources`, after Task 8).
  - `palette.rs:276`, `menu.rs:472`, `menu.rs:509` (contract tests) → `&BTreeMap::new()` +
    `&mut Vec::new()` (these tests don't exercise config).
  - `e2e.rs:116` `new_with_plugin` → `&BTreeMap::new()` + `&mut Vec::new()` (or thread real config in the
    dedicated plugin-config e2e).
  - `plugin/host.rs` tests: `:230` (`make`), `:403`, `:492`, `:520`, `:574` → `&BTreeMap::new()` +
    `&mut Vec::new()`.
  - `plugin/load.rs` tests: `:229, 247, 259, 271, 283, 296, 307, 317, 328, 346, 358` → same empty
    map/scratch warns. (A `plugin::load::tests::config_reaches_wc_config` test passes a REAL one-entry map.)

**Acceptance:** `cargo test -p wordcartel plugin:: config::` green (config parse/fold, conversion caps,
`wc.config` reachability + nil-on-over-cap); warning-free; clippy clean.

**Contract:** `[plugins].dir`/`config` are config-layer load machinery (law 2 — not runtime options);
no command surface touched here (the `plugins_reload` verb over them lands in Task 8).

---

## Task 6 — the event system: `PluginEvent`/queue/`fire_event` + pump event-drain + observer-only `InvokeState` (§3)

Observer-only hooks at the three cold-path sites, drained by the pump. `wc.on` registration (load-time,
atomic with commands), the host hook table, the `Editor.pending_plugin_events` queue, `fire_event` at
save/open/close, the pump's event-drain phase (still single-pass — hooks can't cascade yet), and the
`InvokeState` observer guard on the edit APIs. No pump signature change (event invocation needs no
dispatch context).

**Model:** most-capable (the borrow-safe drain + the observer invariant + the 3 fire-site borrow
choreographies). **Files:** `wordcartel/src/plugin/mod.rs` (`PluginEvent`/kind/`event_from_str`/
`fire_event`), `wordcartel/src/editor.rs` (`pending_plugin_events`), `wordcartel/src/plugin/host.rs`
(`Bridge.invoke_state`, `HookEntry`, hook table, event-drain, null-host clears events),
`wordcartel/src/plugin/api.rs` (`install_on` + closed stub + observer checks on edits),
`wordcartel/src/plugin/load.rs` (`load_one` collects+commits hooks, returns them),
`wordcartel/src/save.rs` / `workspace.rs` / `session_restore.rs` (fire sites).

**TDD first (in `plugin/host.rs` tests unless noted):**
- `on_save_hook_fires_with_path_payload` — register a plugin with `wc.on("save", function(ev)
  wc.status(ev.kind..':'..tostring(ev.path)) end)`; push a `PluginEvent{Save, Some("/x")}` onto the queue;
  pump; assert status `"save:/x"`.
- `hooks_fire_in_registration_order` — two hooks on `"save"` set a shared marker; assert order.
- `hook_error_is_isolated_other_hooks_run` — a first hook `error()`s; a second still runs; editor intact.
- `hook_cannot_edit_observer_guard` — assert EACH of the three blocked surfaces errors from a hook:
  separate hooks (or one hook using `pcall`) calling `wc.insert('X')`, `wc.replace(0,0,'X')`, and
  `wc.set_selection(0,1)` each → the buffer/selection are UNCHANGED and the error mentions "editing is
  not allowed from an event hook"; a hook calling `wc.status('ok')` STILL succeeds (status is the one
  allowed mutation surface). Then a normal command callback calling `wc.insert('X')` STILL succeeds (the
  flag reset, incl. after a panicking hook via the RAII case).
- `event_with_no_hooks_is_dropped` — firing an event kind no plugin subscribed to is a no-op.
- `null_host_clears_event_queue` — a `PluginHost::null()` pump clears `pending_plugin_events` (no
  unbounded growth under `--no-plugins`).
- `wc_on_after_load_errors` — a callback calling `wc.on` post-load → typed error (the closed stub),
  mirroring `wc.register_command`'s post-load error.
- `hooks_commit_atomically_with_commands` — a plugin whose 2nd `register_command` collides registers
  ZERO hooks too (the all-or-nothing now spans both verbs).
- `e2e.rs`: `on_save_hook_fires_on_real_save` — a harness plugin with an `on_save` hook; drive a real
  save (InlineExecutor); assert the hook ran (status/marker) exactly once on `Ok`.
- `e2e.rs`: `on_buffer_close_and_open_fire` — file-browser-style open fires `on_open` once (both the
  new-buffer and throwaway-reuse shapes — no double-fire); a clean close fires `on_buffer_close` with the
  pre-removal path; quit fires nothing.

**Implementation:**
- `plugin/mod.rs`:
  ```rust
  /// The three P2 event kinds (exhaustive — adding a kind is a deliberate act every match handles).
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub enum PluginEventKind { Save, Open, BufferClose }

  /// One fired event, queued on Editor.pending_plugin_events. Payload is OWNED (by drain time the
  /// buffer may be gone/changed), path clamped to PLUGIN_MAX_EVENT_PAYLOAD at capture.
  #[derive(Clone, Debug)]
  pub struct PluginEvent { pub kind: PluginEventKind, pub path: Option<String> }

  /// Parse a hook event name (the menu_from_str parse-to-enum precedent — the enum IS the bound).
  pub fn event_from_str(s: &str) -> Option<PluginEventKind> {
      match s { "save" => Some(PluginEventKind::Save), "open" => Some(PluginEventKind::Open),
                "buffer_close" => Some(PluginEventKind::BufferClose), _ => None }
  }
  fn kind_str(k: PluginEventKind) -> &'static str {
      match k { PluginEventKind::Save => "save", PluginEventKind::Open => "open",
                PluginEventKind::BufferClose => "buffer_close" }
  }

  /// Capture-and-enqueue an event at a fire site (cold-path only — save/open/close; never per-keystroke).
  /// One clamp + one push; drained the same frame by the pump. Path clamped at capture (resource LAW).
  pub(crate) fn fire_event(editor: &mut Editor, kind: PluginEventKind, path: Option<&std::path::Path>) {
      let path = path.map(|p| cap_status(p.to_string_lossy().as_bytes(), crate::limits::PLUGIN_MAX_EVENT_PAYLOAD));
      editor.pending_plugin_events.push_back(PluginEvent { kind, path });
  }
  ```
- `editor.rs` — add beside `pending_plugin_calls`:
  ```rust
  /// Observer-only plugin events (save/open/buffer_close) queued by cold-path fire sites, drained by
  /// the pump (P2 §3). Default-empty; edge-triggered by real ops, never by idle time.
  pub pending_plugin_events: std::collections::VecDeque<crate::plugin::PluginEvent>,
  ```
  Initialize `VecDeque::new()` in every `Editor` constructor (grep the `pending_plugin_calls:` init sites
  — the compiler forces each).
- `plugin/host.rs`:
  - `Bridge` gains `pub invoke_state: Rc<RefCell<InvokeState>>`; add:
    ```rust
    /// What the pump is currently invoking, shared with every wc.* closure (each captures a clone).
    /// `observer` is true exactly while a HOOK runs — the edit APIs (and wc.command, Task 7) check it
    /// and degrade to a typed error (the observer-only binding constraint, enforced in code).
    pub(crate) struct InvokeState { pub current: Option<String>, pub observer: bool }
    ```
    `attach_bridge` builds `Rc::new(RefCell::new(InvokeState { current: None, observer: false }))` and
    passes it to `install_editor_api` (the edit closures capture a clone).
  - `PluginHost` gains `hooks: Vec<HookEntry>` (owned, NOT interned — die with the VM at reload):
    ```rust
    /// A committed hook: its VM-registry key + kind + a label for plugin_error attribution.
    pub struct HookEntry { pub kind: crate::plugin::PluginEventKind, pub key: String, pub label: String }
    ```
    Add `pub(crate) fn append_hooks(&mut self, hs: Vec<HookEntry>) { self.hooks.extend(hs); }` (used by
    `load_sources` after `load_one` returns — the borrow of `lua` is released by then).
  - **Pump event-drain phase — INTERIM (single-pass; Task 7 supersedes it).** No signature change here
    (hooks are observer-only and cannot enqueue, so a single drain pass is correct for THIS task). **Task
    7 REWRITES `pump` into the unified re-drain loop (dispatch/call/event) with chain/time caps and
    re-runs THIS task's event-drain tests against that final loop** — so the drain architecture is tested
    twice (interim here, final in Task 7) and no cascade/cap behavior is validated against the interim
    shape. The **cascade + chain-cap + cycle-budget tests land in Task 7 with the final pump, never
    here.** After the existing call-drain, drain events with an observer guard per invocation:
    ```rust
    // Phase A also takes events: let events = std::mem::take(&mut e.pending_plugin_events)…;
    // Phase B (after calls), for each event, for each matching hook:
    for ev in events {
        for h in self.hooks.iter().filter(|h| h.kind == ev.kind) {
            let key = h.key.clone(); let label = h.label.clone();
            // Set observer mode via an RAII guard so a panicking hook can't leak it onto the next unit.
            let _obs = ObserverGuard::enter(self.bridge_invoke_state(), &label);
            let outcome = crate::panicx::catch(|| {
                let cb: mlua::Function = lua.named_registry_value(&key)?;
                let arg = self.event_table(lua, &ev)?;                       // { kind=…, path=… }
                with_time_guard(lua, CALLBACK_TIME_BUDGET, || cb.call::<()>((arg,)))
            });
            if let Err(msg) = normalize(outcome) { crate::plugin::plugin_error(editor, &label, &msg); }
        }
    }
    ```
    where `ObserverGuard::enter` sets `observer = true` + `current = Some(label)` and its `Drop` resets
    both (the `HookGuard` RAII pattern). `event_table` builds `{ kind = kind_str(ev.kind), path =
    ev.path or nil }`. **Null-host pump** (`self.lua` is `None`) now CLEARS `pending_plugin_events` (and
    `pending_plugin_calls`) under one short borrow instead of returning untouched.
- `plugin/api.rs`:
  - `install_on(lua, hook_sink: Rc<RefCell<Vec<(PluginEventKind, mlua::Function)>>>)` — the load-time
    `wc.on(name, fn)` collector: extract `name: mlua::String`, `event_from_str` (unknown → typed error,
    nothing stored), enforce `PLUGIN_MAX_HOOKS_PER_PLUGIN` on the sink length, push `(kind, func)`.
  - `install_editor_api` — the observer guard covers exactly the **mutation** surfaces. The four
    `try_borrow_mut` API closures in `plugin/api.rs` are `install_insert` (`wc.insert`),
    `install_replace` (`wc.replace`), `install_set_selection` (`wc.set_selection`), and `install_status`
    (`wc.status`, `api.rs:342`). **Observer mode BLOCKS the three editing surfaces**
    (`insert`/`replace`/`set_selection`) with a typed error, and **ALLOWS ONLY `wc.status`** (a hook's
    whole point is to read + emit status). Add the check at the TOP of the `install_insert`,
    `install_replace`, and `install_set_selection` closures (before any borrow/alloc); do NOT add it to
    `install_status`:
    ```rust
    if bridge.invoke_state.borrow().observer {
        return Err(mlua::Error::runtime("plugin: editing is not allowed from an event hook"));
    }
    ```
    (each of the three edit closures captures `bridge.invoke_state.clone()`; the reads — `wc.text`/
    `selection`/`cursor`/`len`/`version`/`path` — and `wc.status` are unguarded, and `wc.command`'s own
    observer check lands in Task 7.) Add `install_on_closed` (the post-load stub raising `"wc.on is only
    available during plugin load"`, mirroring `install_registration_closed`), and the `wc.config` clear
    from Task 5.
- `plugin/load.rs` — `load_one` installs `install_on` alongside `install_registration`, collects the
  hook sink, and in the **two-phase commit** writes each hook's `wc-ev-<stem>-<i>` key in the FALLIBLE
  phase (same fault-seam-guarded path as command keys). **Wire the hook count end-to-end so
  `plugin_list` (Task 8) has a real data source (IMPORTANT 1 — no placeholder):**
  - `load_one`'s Ok payload grows from `usize` to `(usize /*commands*/, Vec<HookEntry> /*committed hooks*/)`
    — i.e. `fn load_one(...) -> Result<(usize, Vec<HookEntry>), LoadFailure>`. It returns the committed
    `HookEntry`s (their `wc-ev-*` keys already written to the VM in the fallible phase).
  - **`LoadReport` gains a `pub hooks: usize` field** (`plugin/load.rs`) — the LOW-CHURN choice:
    `result: Result<usize, String>` STAYS the command count, so **every existing `assert_eq!(reports[i].
    result, Ok(n))` test is untouched** (`load.rs:231, 318, 333, 364`; `host.rs:231, 575`), and `hooks`
    is a sibling count. **Every `LoadReport` struct literal must set the new field — there are exactly
    FIVE construction sites (a missed one is a compile error). Against the post-Task-2 shape of
    `load_sources` (which has THREE push arms — `Ok`, `Validation`-error, `VmExhausted`-error — plus the
    two `discover` skipped constructors):**
    1. `load_sources` **`Ok` arm** → `hooks: n_hooks` (the REAL count from `load_one`'s
       `(usize, Vec<HookEntry>)` — see the next bullet).
    2. `load_sources` **`Err(LoadFailure::Validation(msg))` arm** → `hooks: 0` (atomic — a failed plugin
       committed no hooks).
    3. `load_sources` **`Err(LoadFailure::VmExhausted(msg))` arm** → `hooks: 0`.
    4. `discover` skipped constructor **`load.rs:77`** (bad-UTF-8 source) → `hooks: 0`.
    5. `discover` skipped constructor **`load.rs:82`** (oversize/unreadable) → `hooks: 0`.
  - `load_sources`, on `Ok((commands, hookvec))`, captures the length BEFORE the move
    (`let n_hooks = hookvec.len();`), calls `host.append_hooks(hookvec)` AFTER `load_one` returns (the
    `lua` borrow is released, so `&mut host` is free — same discipline as the fatal-null path), and pushes
    `LoadReport { plugin, result: Ok(commands), hooks: n_hooks }` (site 1 above).
  - Task 8's `PluginRecord`/`plugin_list` read `commands` from `result` and `hooks` from the new field —
    no side-channel placeholder.
- **Fire sites** (each a one-liner, cold-path):
  - `save.rs` `do_save_to` merge closure: compute `let fire_save: Option<PathBuf> = matches!(outcome,
    Ok(SaveOutcome::Saved | SaveOutcome::Unchanged)).then(|| target.clone());` from the closure's OWNED
    `target` (NOT from `b` — a closed buffer must still fire), and AFTER the `if let Some(b) =
    editor.by_id_mut(buffer_id)` block closes and after `editor.status = status`:
    `if let Some(p) = fire_save { crate::plugin::fire_event(editor, PluginEventKind::Save, Some(&p)); }`.
  - `workspace.rs` `open_as_new_buffer` `Ok(b)` arm (the non-reuse branch) and `session_restore.rs`
    `open_into_current` `Ok(b)` arm: `crate::plugin::fire_event(editor, PluginEventKind::Open,
    Some(path));` (exactly one fires per open — the reuse branch returns after delegating).
  - `workspace.rs` `close_buffer_now`: capture the path BEFORE the slot is removed/replaced
    (`let closing = editor.by_id(id).and_then(|b| b.document.path.clone());`), then beside the existing
    `editor.diag_provider.notify_close(id);` call `crate::plugin::fire_event(editor,
    PluginEventKind::BufferClose, closing.as_deref());`.

**Acceptance:** `cargo test -p wordcartel plugin:: e2e::` green (all hook/observer/fire-site tests);
warning-free; clippy clean; `module_budgets` `plugin/host.rs` ≤ 400 (report; if over, extract
`plugin/events.rs`).

**Contract:** events are host↔plugin data flow, not command-surface actors — no conformance impact.

---

## Task 7 — `wc.command` + unified re-drain loop + chain/time caps + post-pump `hydrate_overlays` (§5)

`wc.command(name)` enqueues a dispatch; the pump grows its dispatch context + a re-drain loop bounded by
`PLUGIN_PUMP_CHAIN_CAP` (count) and `PUMP_CYCLE_TIME_BUDGET` (wall, between-units); the run loop gains a
post-pump `hydrate_overlays` so a `wc.command`-opened overlay is hydrated like key/menu/mouse opens.

**Model:** most-capable (the pump signature change + re-drain invariants + the overlay-hydration gap).
**Files:** `wordcartel/src/editor.rs` (`pending_plugin_dispatch`), `wordcartel/src/plugin/mod.rs`
(`PluginDispatch`), `wordcartel/src/plugin/host.rs` (pump signature + re-drain + caps + null-host clears
dispatch), `wordcartel/src/plugin/api.rs` (`install_command` + observer check), `wordcartel/src/limits.rs`
(`PLUGIN_MAX_COMMAND_REF`/`PLUGIN_MAX_PENDING_DISPATCH`/`PLUGIN_PUMP_CHAIN_CAP`), `wordcartel/src/app.rs`
(pump call args + post-pump hydrate), `wordcartel/src/e2e.rs` (`step`/`step_timed` pump args).

**TDD first (in `plugin/host.rs` tests unless noted):**
- `wc_command_dispatches_a_builtin` — a plugin command calling `wc.command("select_all")` (a nullary
  builtin) has the builtin's observable effect after the pump.
- `wc_command_chains_a_plugin_command` — `a.cmd` calls `wc.command("b.cmd")` which inserts `X`; one pump
  cycle lands `X` (the re-drain picks up the enqueued `PluginCall`).
- `wc_command_unknown_name_reports_error` — `wc.command("nope")` → `plugin_error` status naming the
  origin, no panic.
- `wc_command_over_length_and_full_queue_reject` — a name > `PLUGIN_MAX_COMMAND_REF` and a dispatch queue
  at `PLUGIN_MAX_PENDING_DISPATCH` each → typed error, nothing queued.
- `wc_command_from_hook_is_rejected` — `wc.on("save", function() wc.command("x") end)` → typed error
  (observer), no dispatch.
- `pump_chain_cap_truncates_pingpong` — `a.cmd` dispatches `b.cmd` dispatches `a.cmd` … → the cycle
  terminates at `PLUGIN_PUMP_CHAIN_CAP` with all queues cleared + a status; the editor is intact.
- `pump_cycle_time_budget_truncates` — with a low test `PUMP_CYCLE_TIME_BUDGET`, a many-unit cascade is
  truncated between units.
- `e2e.rs`: `wc_command_palette_open_is_hydrated` — a plugin command calling `wc.command("palette")`,
  driven through the real `step` (reduce → pump → post-pump hydrate); assert `editor.palette`'s `rows`
  are populated (not empty). Repeat for `wc.command("menu")` → `menu.built` with groups.

**Implementation:**
- `plugin/mod.rs`:
  ```rust
  /// A queued wc.command dispatch (fire-and-forget). `origin` names the requesting plugin cmd/hook for
  /// error attribution; `name` is the raw target (resolved at drain — no call-time registry snapshot).
  #[derive(Clone, Debug)]
  pub struct PluginDispatch { pub origin: String, pub name: String }
  ```
- `editor.rs` — add `pub pending_plugin_dispatch: std::collections::VecDeque<crate::plugin::PluginDispatch>`
  beside the other two queues; init in every constructor.
- `limits.rs` — add `PLUGIN_MAX_COMMAND_REF = PLUGIN_MAX_STEM_LEN + 1 + PLUGIN_MAX_NAME_LEN`,
  `PLUGIN_MAX_PENDING_DISPATCH = 64`, `PLUGIN_PUMP_CHAIN_CAP = 64`; extend `plugin_caps_are_sane`. Add
  `pub(crate) const PUMP_CYCLE_TIME_BUDGET: Duration = Duration::from_millis(500);` in `host.rs` beside
  the other budgets (a between-units budget, checked before dequeuing the next unit — NOT preemptive).
- `plugin/api.rs` — `install_command` in `install_editor_api`:
  ```rust
  // wc.command(name): observer check → length cap → queue cap → enqueue (fire-and-forget).
  let editor = bridge.editor.clone();
  let invoke = bridge.invoke_state.clone();
  wc.set("command", lua.create_function(move |_, name: mlua::String| {
      let st = invoke.borrow();
      if st.observer {
          return Err(mlua::Error::runtime("plugin: wc.command is not allowed from an event hook"));
      }
      if name.as_bytes().len() > crate::limits::PLUGIN_MAX_COMMAND_REF {
          return Err(mlua::Error::runtime("plugin: command name too long"));
      }
      let origin = st.current.clone().unwrap_or_default();
      drop(st);
      let mut e = editor.try_borrow_mut().map_err(|_| mlua::Error::runtime("plugin: editor busy"))?;
      if e.pending_plugin_dispatch.len() >= crate::limits::PLUGIN_MAX_PENDING_DISPATCH {
          return Err(mlua::Error::runtime("plugin: command queue full"));
      }
      e.pending_plugin_dispatch.push_back(crate::plugin::PluginDispatch { origin, name: name.to_str()?.to_owned() });
      Ok(())
  })?)?;
  ```
  (Set `invoke_state.current` for command callbacks too — extend the pump's call-drain to set `current =
  Some(id.0.to_string())` via the same `ObserverGuard`-style enter with `observer=false`, so `wc.command`
  attribution works from a command callback.)
- `plugin/host.rs` — the pump signature grows and becomes a re-drain loop:
  ```rust
  /// One pump cycle: drain dispatches, calls, and events to quiescence or a cap. Takes the dispatch
  /// context (mirrors reduce's params) so a wc.command target routes through the SAME Registry::dispatch
  /// the palette/menu/keys use. Two between-units caps (§5c): PLUGIN_PUMP_CHAIN_CAP (count) +
  /// PUMP_CYCLE_TIME_BUDGET (wall); on trip → clear all queues + status.
  pub fn pump(&mut self, editor: &Rc<RefCell<Editor>>, reg: &crate::registry::Registry,
              ex: &dyn crate::jobs::Executor, clock: &dyn wordcartel_core::history::Clock,
              msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>) {
      let Some(_) = self.lua.as_ref() else {
          // Null host: clear the queues so unconditional fire sites can't grow them unbounded.
          let mut e = editor.borrow_mut();
          e.pending_plugin_calls.clear(); e.pending_plugin_events.clear(); e.pending_plugin_dispatch.clear();
          return;
      };
      let start = std::time::Instant::now();
      let mut units = 0usize;
      loop {
          // Phase A — take all three queues under ONE short borrow.
          let (dispatches, calls, events) = {
              let mut e = editor.borrow_mut();
              (std::mem::take(&mut e.pending_plugin_dispatch),
               std::mem::take(&mut e.pending_plugin_calls),
               std::mem::take(&mut e.pending_plugin_events))
          };
          if dispatches.is_empty() && calls.is_empty() && events.is_empty() { break; }
          // Phase B — process with NO outer borrow held; check caps BETWEEN units.
          for d in dispatches {
              if self.cap_tripped(units, start, editor) { return; } units += 1;
              self.drain_one_dispatch(editor, reg, ex, clock, msg_tx, &d);   // resolve + reg.dispatch under a short borrow
          }
          for c in calls {
              if self.cap_tripped(units, start, editor) { return; } units += 1;
              self.invoke_call(editor, c);                                    // P1 shape + ObserverGuard(observer=false)
          }
          for ev in events {
              for h in self.hooks_for(ev.kind) {                              // clone (key,label) to avoid borrow overlap
                  if self.cap_tripped(units, start, editor) { return; } units += 1;
                  self.invoke_hook(editor, &ev, &h);                          // observer=true guard
              }
          }
      }
  }
  ```
  Helpers (each a thin fn — keeps `pump` under `too_many_lines`): `cap_tripped(units, start, editor)`
  checks `units >= PLUGIN_PUMP_CHAIN_CAP || start.elapsed() > PUMP_CYCLE_TIME_BUDGET`, and on trip clears
  all three queues + sets a `plugin_error` status; `drain_one_dispatch` does
  `reg.resolve_name(&d.name)` → `None` → `plugin_error(editor, &d.origin, "unknown command …")`, else
  builds a `Ctx { editor: &mut *e, clock, executor: ex, msg_tx: msg_tx.clone() }` under a short
  `borrow_mut` and `reg.dispatch(id, &mut ctx)` (a `Builtin` runs sync; a `Plugin` enqueues a
  `PluginCall` for the next iteration — the borrow drops before any Lua). `invoke_call`/`invoke_hook`
  reuse the P1/Task-6 bodies with the `ObserverGuard`.
- `app.rs` (`run` loop) — the pump call gains its args and a post-pump hydrate lands before the keymap
  arm:
  ```rust
  plugin_host.pump(&editor, &reg, &executor, &clock, &msg_tx);
  // Hydrate any overlay a pump-dispatched wc.command opened — the pump's reg.dispatch is a dispatch
  // path with no per-site hydrate (unlike key/menu/mouse). Idempotent + self-guarding → a no-op when
  // reduce's own per-path hydrate already ran this iteration.
  { crate::app::hydrate_overlays(&mut editor.borrow_mut(), &reg, &keymap); }
  ```
  (`hydrate_overlays(&mut Editor, &Registry, &KeyTrie)` — the verified signature; `keymap` is the
  loop-local. NO new pump parameter for hydration — it reads `keymap` at the call site.)
- **Migrate EVERY `.pump(` call site** — the COMPLETE list, from `grep -rn "\.pump(" wordcartel/src`
  (verified 2026-07-12; the signature grows from `pump(&editor)` to
  `pump(&editor, reg, ex, clock, msg_tx)`):
  - **Production (3):** `app.rs:811` → `plugin_host.pump(&editor, &reg, &executor, &clock, &msg_tx)`
    (`executor`/`clock`/`msg_tx` are run-locals; `reg` is `mut` since Task 1); `e2e.rs:146` (`step`) and
    `e2e.rs:173` (`step_timed`) → `h.pump(&self.editor, &self.reg, &self.ex, &clock, &self.tx)`
    (harness fields `reg`/`ex`/`tx` exist; `clock` is the per-step `TestClock`; `&self.reg` immutable +
    `&mut self.plugin_host` are disjoint field borrows — allowed). Add the post-pump
    `{ crate::app::hydrate_overlays(&mut editor.borrow_mut(), &reg, &keymap); }` at `app.rs` and the
    mirror `{ crate::app::hydrate_overlays(&mut self.editor.borrow_mut(), &self.reg, &self.keymap); }` in
    `e2e.rs` `step`/`step_timed`, right after the pump slot.
  - **`plugin/host.rs` tests (~24 sites):** `host.rs:254, 268, 280, 289, 299, 309, 321, 336, 346, 357,
    368, 394, 407, 434, 444, 458, 466, 474, 482, 505, 530, 587, 596` all call `host.pump(&editor)`.
    Rather than thread five args through each, add a **`#[cfg(test)]` convenience** on `PluginHost`:
    ```rust
    #[cfg(test)]
    pub(crate) fn pump_test(&mut self, editor: &Rc<RefCell<Editor>>) {
        // A builtins registry + InlineExecutor + TestClock + throwaway channel — the P1 host tests
        // exercise callback/isolation behavior that needs no specific dispatch context.
        let reg = crate::registry::Registry::builtins();
        let ex = crate::jobs::InlineExecutor::default();
        let clock = crate::test_support::TestClock::new(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        self.pump(editor, &reg, &ex, &clock, &tx);
    }
    ```
    and flip those `host.pump(&editor)` → `host.pump_test(&editor)`. **New Task-7 tests that need a
    SPECIFIC registry** (`wc_command_dispatches_a_builtin`, the ping-pong cascade with two plugin
    commands) call the REAL `pump(&editor, &reg, &ex, &clock, &tx)` with an explicit `reg`.
- **Re-run Task 6's event-drain tests against this final pump (IMPORTANT 3).** The `on_save`/observer/
  order/isolation event tests from Task 6 now exercise the re-drain loop, not the interim single-pass —
  the drain architecture is validated once against the final shape.

**Acceptance:** `cargo test -p wordcartel plugin:: e2e::` green (dispatch, chaining, caps, observer,
overlay-hydration, AND the re-run event-drain tests); warning-free; clippy clean.

**Contract (law 1):** `wc.command` routes through `reg.resolve_name` + `reg.dispatch` — the identical
path palette/menu/keys use; no call-time name-set snapshot (a derived registry cache would be a second
source of truth).

---

## Task 8 — reload: `load_phase` extraction + `perform_reload` + `plugins_reload`/`plugin_list` builtins (§6)

The heaviest task — the effort's main risk. Extract the startup load block into a shared
`plugin/reload.rs::load_phase`, add `perform_reload` (whole-VM teardown + registry `retain_builtins` +
queue clear + keymap re-resolve + config re-read), the two builtins, the `plugin_inventory`, and the
run-loop reload seam. **Split into 8a/8b if one task is too large for a single implementer — but the
split must leave a COHERENT, non-broken state at each boundary (a committed task never registers a
user-facing command that silently does nothing).** The correct split (Codex option (a)):
- **8a — all reload MACHINERY, no command registered.** `plugin/reload.rs::load_phase` extraction (with
  the `app.rs` startup site rewired to call it), `perform_reload`, the `plugins_reload_requested` +
  `plugin_inventory` `Editor` fields, AND the run-loop reload seam (`if …plugins_reload_requested {
  perform_reload(…) }`). `plugins_reload`/`plugin_list` are NOT yet in `builtins()`, so nothing user-
  facing exists that could no-op — the seam is dormant (the flag is never set) but complete and unit-
  tested via `perform_reload` called directly (set `editor.plugins_reload_requested = true` in the test,
  then invoke the seam / `perform_reload`). Tree is green and coherent.
- **8b — register the two builtins, now that the machinery they drive is live.** Add `plugins_reload`/
  `plugin_list` to `Registry::builtins()`; the moment `plugins_reload` exists it is fully functional (the
  seam from 8a picks up its flag). Add the palette/menu-facing + interaction-matrix tests.
Keeping reload as ONE task is also acceptable; the split above is the only correct 2-way cut.

**Model:** most-capable. **Files:** `wordcartel/src/plugin/reload.rs` (new), `wordcartel/src/plugin/mod.rs`
(`pub mod reload;` + `PluginRecord`), `wordcartel/src/editor.rs` (`plugins_reload_requested`,
`plugin_inventory`), `wordcartel/src/registry.rs` (`plugins_reload`/`plugin_list` builtins),
`wordcartel/src/app.rs` (extract the load block → `load_phase`; the reload seam; **change the definition
`struct SystemClock;` at `app.rs:372` → `pub(crate) struct SystemClock;`** so `reload.rs` can construct
it for the reload `attach_bridge` — the same ZST the run loop uses. From `grep -rn "SystemClock"
wordcartel/src`: the def is `app.rs:372`; the existing uses at `app.rs:724` (`let clock = SystemClock;`)
and `app.rs:783` (`Rc::new(SystemClock)`) are in the same module and unaffected by widening visibility;
`reload.rs` adds a new `crate::app::SystemClock` use — no other files reference it).

**TDD first:**
- `registry.rs`: `plugins_reload_sets_flag` (dispatching `plugins_reload` sets
  `ctx.editor.plugins_reload_requested`); `plugin_list_formats_inventory` (dispatching `plugin_list`
  writes a summary of `editor.plugin_inventory` to status).
- `plugin/reload.rs` unit tests (drive `perform_reload` against a real `Rc<RefCell<Editor>>` + a temp
  plugins dir):
  - `reload_replaces_changed_plugin` — write `p.lua` registering `p.a`; `load_phase`; then rewrite `p.lua`
    to register `p.b`; set the flag; `perform_reload`; assert `p.a` gone, `p.b` resolves, host has a VM.
  - `reload_drops_removed_plugin_binding` — a keymap patch bound to `p.a`; after reload removes `p.a`,
    `keymap_rebuild` is set and (after the existing rebuild arm) the binding is dropped with a warning.
  - `reload_clears_stale_queues` — pre-seed `pending_plugin_calls`/`events`/`dispatch`; after
    `perform_reload` all three are empty.
  - `reload_with_no_plugins_flag_is_builtins_only` — `cli.no_plugins` (or `enabled=false`) → null host +
    builtins-only registry + working editor.
  - `reload_recovers_from_failed_vm` — start null (forced `PluginHost::new` failure simulated), reload
    with a valid dir → a live host + loaded plugin (reload doubles as recovery).
  - `reload_rereads_plugins_config_section` — change `[plugins.config.p]` between reloads → the new value
    reaches `wc.config` (⚠OPEN flag 2 = A, baked in).
  - `commit_exhaustion_during_reload_reverts_whole_subsystem` — force a commit-time `Err` during the
    reload's `load_phase` (the Task-2 fault seam): assert null host + builtins-only registry + cleared
    queues + `keymap_rebuild` set (the editor-side revert that startup didn't need).

**Implementation:**
- `plugin/mod.rs` — `PluginRecord` for the inventory:
  ```rust
  /// One discovered plugin's load outcome, for plugin_list + reload reporting (owned, bounded by the
  /// discover/load caps). `error: None` == loaded cleanly.
  pub struct PluginRecord { pub name: String, pub commands: usize, pub hooks: usize, pub error: Option<String> }
  ```
- `editor.rs` — add `pub plugins_reload_requested: bool` (default false; the `keymap_rebuild`/
  `settings_save_requested` flag pattern) and `pub plugin_inventory: Vec<crate::plugin::PluginRecord>`
  (default empty); init in constructors.
- `plugin/reload.rs` (new) — the shared load orchestration + reload sequence:
  ```rust
  //! Shared plugin load orchestration: `load_phase` (used by startup AND reload) and `perform_reload`
  //! (whole-VM teardown + registry revert + queue clear + keymap re-resolve, P2 §6). Keeps app.rs a
  //! thin caller (anti-regrowth): the reload BODY lives here, not in run().

  /// Resolve the plugins dir, discover, load, and populate host hooks + the inventory + warnings.
  /// Shared by startup and reload so both paths are byte-identical. On a commit-time VM-exhaustion
  /// `load_sources` already nulled the host + reverted the registry (§7b registry half); this fn
  /// reflects that in the inventory/warnings.
  pub(crate) fn load_phase(
      reg: &mut crate::registry::Registry,
      host: &mut crate::plugin::host::PluginHost,
      plugins: &crate::config::PluginsConfig,
      xdg: Option<&std::path::Path>,
      warns: &mut Vec<String>,
  ) -> Vec<crate::plugin::PluginRecord> {
      // [plugins].dir wins; else <xdg>/wordcartel/plugins; else warn (not silence — gap 6).
      let dir = plugins.dir.clone()
          .or_else(|| xdg.map(|x| x.join("wordcartel").join("plugins")));
      let Some(dir) = dir else {
          warns.push("plugins: no config directory found (set [plugins].dir)".into());
          return Vec::new();
      };
      let disc = crate::plugin::load::discover(&dir, &plugins.disable);
      let mut inventory = Vec::new();
      for r in &disc.skipped {
          warns.push(format!("plugin {} skipped: {}", r.plugin, r.result.as_ref().err().cloned().unwrap_or_default()));
          inventory.push(crate::plugin::PluginRecord { name: r.plugin.clone(), commands: 0, hooks: 0,
              error: r.result.as_ref().err().cloned() });
      }
      for r in crate::plugin::load::load_sources(reg, host, &disc.sources, &plugins.config, warns) {
          // LoadReport = { plugin, result: Result<usize/*commands*/, String>, hooks: usize } (Task 6):
          // commands from `result`, hooks from the real `hooks` field — no side-channel placeholder.
          match &r.result {
              Ok(n) => inventory.push(crate::plugin::PluginRecord { name: r.plugin.clone(), commands: *n,
                  hooks: r.hooks, error: None }),
              Err(e) => { warns.push(format!("plugin {}: {e}", r.plugin));
                  inventory.push(crate::plugin::PluginRecord { name: r.plugin.clone(), commands: 0,
                      hooks: r.hooks, error: Some(e.clone()) }); } // hooks == 0 on failure (atomic)
          }
      }
      inventory
  }

  /// The between-reduces reload seam (§6d). Reverts the plugin subsystem and rebuilds it from a fresh
  /// re-read of the [plugins] config section (⚠OPEN 2 = A). Never runs under a Lua frame — the run loop
  /// calls it AFTER pump() (which quiesced) and BEFORE rebuild_keymap_if_requested (so keymap_rebuild
  /// set here is honored the same iteration).
  pub(crate) fn perform_reload(
      host: &mut crate::plugin::host::PluginHost,
      reg: &mut crate::registry::Registry,
      editor: &std::rc::Rc<std::cell::RefCell<crate::editor::Editor>>,
      all_paths: &[std::path::PathBuf],
      xdg: Option<&std::path::Path>,
      no_plugins: bool,
      msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>,
  ) {
      { editor.borrow_mut().plugins_reload_requested = false; }              // 1. clear the flag
      let (cfg, _warns) = crate::config::load(all_paths);                     // 2. re-read config…
      let plugins = cfg.plugins;                                             //    …take ONLY [plugins]
      *host = crate::plugin::host::PluginHost::null();                        // 3. tear down the VM
      reg.retain_builtins();                                                  // 4. registry → builtins-only
      { let mut e = editor.borrow_mut();                                     // 5. drain stale queues
        e.pending_plugin_calls.clear(); e.pending_plugin_events.clear(); e.pending_plugin_dispatch.clear(); }
      let mut warns = Vec::new();
      let mut inventory = Vec::new();
      if no_plugins || !plugins.enabled {                                     // 6. rebuild (or stay null)
          warns.push(if no_plugins { "plugins disabled (--no-plugins)".into() }
                     else { "plugins disabled by config".into() });
      } else {
          match crate::plugin::host::PluginHost::new() {                      //    retry a failed VM
              Ok(mut h) => {
                  inventory = load_phase(reg, &mut h, &plugins, xdg, &mut warns);
                  if h.has_vm() {                                             //    load_sources may have nulled it (exhaustion)
                      if let Err(e) = h.attach_bridge(editor.clone(), msg_tx.clone(),
                          std::rc::Rc::new(crate::app::SystemClock) as std::rc::Rc<dyn wordcartel_core::history::Clock>) {
                          warns.push(format!("plugin bridge failed to attach: {e}"));
                      }
                      *host = h;                                              // 7. re-attach → NEW VM
                  } // else: exhaustion already reverted host(null)+reg(builtins); the editor-side
                    //       clear (step 5) + keymap_rebuild (step 8) below complete the subsystem revert.
              }
              Err(e) => warns.push(format!("plugins disabled: {e}")),
          }
      }
      let mut e = editor.borrow_mut();                                        // 8. re-resolve bindings + report
      e.keymap_rebuild = true;
      e.plugin_inventory = inventory;
      if let Some(w) = warns.first() { e.status = w.clone(); }
      else { e.status = format!("plugins reloaded ({} ok)",
          e.plugin_inventory.iter().filter(|r| r.error.is_none()).count()); }
  }
  ```
  (The hook count reaches the inventory via `LoadReport.hooks` — Task 6 wired it through concretely; each
  `PluginRecord` carries real `commands`/`hooks`/`error`.)
- `registry.rs` — the two builtins in `builtins()` (grep the settings-save `r.register(...)` block for
  the handler idiom; both `MenuCategory::Settings`):
  ```rust
  r.register("plugins_reload", "Reload Plugins", Some(MenuCategory::Settings), |c| {
      c.editor.plugins_reload_requested = true;
      c.editor.status = "reloading plugins…".into();
      CommandResult::Handled
  });
  r.register("plugin_list", "List Plugins", Some(MenuCategory::Settings), |c| {
      let inv = &c.editor.plugin_inventory;
      let ok = inv.iter().filter(|r| r.error.is_none()).count();
      let failed = inv.len() - ok;
      let cmds: usize = inv.iter().map(|r| r.commands).sum();
      let hooks: usize = inv.iter().map(|r| r.hooks).sum();  // real hook total (Task 6 wiring)
      c.editor.status = format!("plugins: {ok} ok ({cmds} cmds, {hooks} hooks), {failed} failed");
      CommandResult::Handled
  });
  ```
- `app.rs` (`run`) — extract the inline startup load block into a `load_phase` call (net-negative for the
  budget) and add the reload seam:
  - Replace the `match PluginHost::new() { Ok(h) => { discover…load_sources… } … }` block with:
    ```rust
    let mut plugin_host = if cli.no_plugins || !cfg.plugins.enabled {
        crate::plugin::host::PluginHost::null()
    } else {
        match crate::plugin::host::PluginHost::new() {
            Ok(mut h) => {
                let inv = crate::plugin::reload::load_phase(&mut reg, &mut h, &cfg.plugins, xdg.as_deref(), &mut warns);
                editor.plugin_inventory = inv;   // editor is still the plain &mut here (pre-Rc-wrap)
                h
            }
            Err(e) => { warns.push(format!("plugins disabled: {e}")); crate::plugin::host::PluginHost::null() }
        }
    };
    ```
    (The startup editor's queues are empty and its keymap is built AFTER this, so no editor-side revert is
    needed at startup even on exhaustion — `load_sources` already reverted host+registry.)
  - Reload seam, immediately AFTER `plugin_host.pump(...)` + the post-pump hydrate (Task 7) and BEFORE
    `rebuild_keymap_if_requested`:
    ```rust
    if editor.borrow().plugins_reload_requested {
        crate::plugin::reload::perform_reload(&mut plugin_host, &mut reg, &editor,
            &all_paths, xdg.as_deref(), cli.no_plugins, &msg_tx);
    }
    ```
    (`all_paths` and `xdg` are run-locals; `reg` is `mut` since Task 1.)

**Acceptance:** `cargo test -p wordcartel plugin:: registry:: e2e::` green (all reload cases, incl. the
exhaustion-during-reload subsystem revert); `module_budgets` app.rs ≤ 1000 (report the number — the
`load_phase` extraction must keep it under); warning-free; clippy clean.

**Contract:** `plugins_reload`/`plugin_list` are `Registry::builtins()` entries tagged
`MenuCategory::Settings` (laws 3/4 by derivation); reload's registry mutation is between-reduces only
(law 1); `keymap_rebuild` re-resolves plugin bindings (law 7).

---

## Task 9 — full §12 suite: contract/budget gates + idle guardrail + demo plugin with a hook

Consolidate the spec §12 evidence not already covered task-locally, plus the end-to-end demo.

**Model:** standard. **Files:** `wordcartel/src/plugin/` + `e2e.rs` test modules, a committed demo plugin
fixture (`wordcartel/tests/fixtures/plugins/wordcount.lua` or an inline e2e string).

**Tests:**
- **Loaded-but-idle guardrail** (extends the swap SSD-wear family): load a plugin WITH an `on_save` hook,
  drive idle `Msg::Tick`s through `step`; assert **zero** hook invocations (a host-side counter), zero
  growth of the three plugin queues, and `timers::next_wake` unchanged (events are edge-triggered by ops,
  never by idle time).
- **Contract invariants** (spec §9, merge GATEs): palette-completeness + menu-subset re-run over a
  **post-reload** registry (a reloaded plugin's `menu=Some(Edit)` command appears in palette + Edit menu;
  `plugins_reload`/`plugin_list` appear in the Settings menu); a patch-bound plugin command resolves in
  `build_keymap` and re-resolves after a reload (law 7); the every-persisted-setting guard unchanged
  (P2 adds no `SettingsSnapshot` field).
- **Config over-cap** (spec §12): an over-`PLUGIN_MAX_CONFIG_STR` value AND an over-cap key each →
  plugin loads with `wc.config == nil` + warning, its command still registers.
- **`wc.command` overlay hydration** (spec §5b, if not already in Task 7's e2e): `wc.command("palette")`
  → hydrated palette rows; `wc.command("menu")` → built menu.
- **Demo** (success criterion): `wordcount.lua` registers an `on_save` hook reading the buffer +
  `wc.status("Saved — N words")`, configured by `[plugins.config.wordcount] min_words = 100`; the e2e
  harness loads it, drives a real save, asserts the status; a live `plugins_reload` of an edited version
  is reflected.

**Acceptance:** `cargo test -p wordcartel` fully green; `cargo clippy --workspace --all-targets` clean;
`scripts/smoke/run.sh` run and its one-line summary quoted in the report; `module_budgets` green (app.rs
≤ 1000, plugin/host.rs ≤ 400 — report both numbers). The demo passes.

---

## Final gates (after Task 9)

1. **Fable whole-branch review** — cross-task invariants: the observer-only guard holds on EVERY edit +
   `wc.command` path (no gap); the two-phase commit is genuinely all-or-nothing incl. the commit-time
   exhaustion revert (registry + host + queues + keymap); no `mlua` type leaked into `registry.rs`/
   `wordcartel-core`; the pump holds no borrow across Lua on every re-drain iteration; idle does no
   plugin work; `hydrate_overlays` reaches every overlay-opening dispatch path. Fable compiles scratch
   probes against the real branch (reserved for THIS gate).
2. **Codex pre-merge gate** — independent GO/NO-GO: spec conformance, both LAWS' completeness re-checked
   against the merged code, module budgets, clippy, contract invariants, the reload airtightness.
Re-run each after fixes until clean/GO. Then merge `--no-ff` to the trunk, verify tests on the merged
result, delete the branch. Push only when asked.

---

## Ledger

Track in `$(git rev-parse --git-path sdd)/progress.md`: one line per completed task + commit range.
After any compaction, trust the ledger + `git log` over recollection; never re-dispatch a task it marks
done. Cross-task signature evolution to record (so a later task doesn't re-litigate it), each with its
FULL migration-site list in the owning task:
- `load_sources` grows `&PluginHost → &mut PluginHost` (Task 2, ~21 call sites) → `+config_map,
  &mut warns` (Task 5, same sites, SECOND edit).
- `load_one` returns `Result<usize,String> → Result<usize, LoadFailure>` (Task 2) → Ok payload
  `usize → (usize, Vec<HookEntry>)` (Task 6).
- `LoadReport` gains `pub hooks: usize` (Task 6). FIVE construction sites total (per Task 6): the
  `load_sources` `Ok` arm sets `hooks` to the real count from `load_one`'s `(usize, Vec<HookEntry>)`;
  the `load_sources` `Validation` and `VmExhausted` error arms and the two `discover` skipped
  constructors (`load.rs:77`/`:82`) set `hooks: 0`. `result: Result<usize,String>` (command count)
  stays, so no existing `Ok(n)` assertion changes.
- `PluginHost::pump` grows `(&editor) → (&editor, reg, ex, clock, msg_tx)` (Task 7; 3 production + ~24
  host.rs test sites via a `#[cfg(test)] pump_test` wrapper).
- `install_registration` `stem: &'static str → String` (Task 2; SOLE caller `load.rs:140`).
- `PendingReg` carries raw strings + `func` (Task 2).
- `wc_table` `fn → pub(crate) fn` (Task 5, MANDATORY for `settings.rs`); `SystemClock` `struct →
  pub(crate) struct` at `app.rs:372` (Task 8, for `reload.rs`); `let reg = reg;` at `app.rs:660` deleted
  (Task 1).
