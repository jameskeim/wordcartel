# Effort P2 — plugin events + per-plugin config + reload: grounding (facts + binding constraints)

**Status:** GROUNDING (2026-07-12). Facts-only code-surface map + the binding constraints the human
has already decided. This is the factual base handed to the independent spec author (Fable), the P2
sibling of `docs/design/effort-p-grounding.md`. Every code anchor here was verified against the real
source on 2026-07-12 (post-P1-ship). Anchor on symbol NAMES, not line numbers — lines drift as tasks
edit files.

---

## 1. Goal

P1 shipped in-process Lua plugin **commands** (a plugin registers a namespaced palette command that
runs Lua with mediated, validated editor access). P2 adds **breadth on that proven spine**:

- **Events / hooks** — `wc.on{"save"|"open"|"buffer_close", fn}` firing at existing cold-path sites.
- **Per-plugin config** — a `[plugins.<name>]` TOML table handed to the plugin as a Lua table.
- **Reload** — `plugins_reload` (whole-VM teardown + re-load) and `plugin_list`.
- **`wc.command`** — a plugin invoking another registered command (dispatch context).
- The three confirmed hardening carry-forwards (load-phase guard, intern-on-commit, same-stem dedup).

No new load-bearing architecture: P1 proved the `Rc<RefCell<Editor>>` + main-thread-pump + panic +
resource-isolation model. P2 reuses every one of those mechanisms.

---

## 2. BINDING CONSTRAINTS (human-decided 2026-07-12 — NOT open for the brainstorm)

These are settled. The spec conforms to them; it does not re-litigate them.

- **HOOK POWER = OBSERVER-ONLY.** `on_save`/`on_open`/`on_buffer_close` hooks may **read** editor
  state and **emit status/messages** — they may **NOT edit** the document or mutate editor state.
  There is no `submit_transaction` access from a hook in P2. Mutation-on-event (format-on-save class)
  is a deliberate deferred fast-follow; do not design an edit path from a hook.
- **Hooks NEVER abort or delay the operation.** A hook that errors → status line (via the existing
  `plugin_error` seam); a runaway hook → killed by a time guard; the save/open/close **proceeds
  regardless**. This is forced by the project law: *no data loss, no silent UI waits*. A buggy or
  slow plugin must never block a save.
- **`on_save` fires AFTER a successful write** (a "saved" event), not on save failure.
- **Reload = whole-VM teardown.** Plugin Lua state does NOT persist across `plugins_reload` (the
  cheaper model — no per-plugin compensating teardown).
- **Per-plugin config = opaque Lua table** from `[plugins.<name>]`; the plugin self-validates its
  own shape. The host does not impose a schema.
- **`wc.command`** (plugin invoking a registered command) is allowed, bounded by a pump chain cap.
- **Command-surface-contract conformance is MANDATORY and explicit.** `plugins_reload`, `plugin_list`,
  and any `[plugins].dir`/config option are real commands/options in the registry (palette-exhaustive,
  every-option-has-a-command). The spec states conformance; the contract's invariant tests are merge
  GATEs. `wc.command` routes through the same registry dispatch, not a side channel.

---

## 3. Code-surface map (exact anchors, verified 2026-07-12)

### 3.1 Cold-path event sites — all hold a live editor borrow

**Save (`on_save`).** `save::do_save_to(ctx, target, mode)` (`wordcartel/src/save.rs`) dispatches a
background `Job{JobKind::Save}`; the disk write runs off-thread (`file::save_atomic`), but the
**completion `merge` closure runs on the main thread**. The "saved successfully" statement is the
`Ok(SaveOutcome::Saved | Unchanged)` arm in that merge closure (sets `status = "Saved"`, calls
`swap::delete`). The merge runs via `jobs_apply::apply_result(r, editor: &mut Editor)` →
`(r.merge)(editor)`, reached from `reduce()` on the JobDone message — i.e. **inside
`editor.borrow_mut()`** (app.rs run loop). No `SaveError` enum by that name; the error path is
`Err(e)` formatted via `e.to_string()`.

**Open (`on_open`).** `file::open(path) -> Result<String, OpenError>` (`wordcartel/src/file.rs`;
`OpenError{NotFound,IsDir,Permission,TooLarge,Binary,Io}`). Success into a live buffer completes in
`workspace::open_as_new_buffer(editor, path)` (`wordcartel/src/workspace.rs`) and
`session_restore::open_into_current(editor, path)` (in-place throwaway reuse). Both are **synchronous,
main-thread, `&mut Editor` under the run loop's borrow**. Caller: `file_browser.rs` on Enter.

**Buffer close (`on_buffer_close`).** `workspace::close_buffer(editor)` is the entry (scratch → no-op;
dirty → C4 prompt; clean → `close_buffer_now`). `workspace::close_buffer_now(editor, id: BufferId)`
is **the one place a buffer is truly removed**; it already calls `editor.diag_provider.notify_close(id)`
— the existing "buffer closing" notification precedent to mirror. Three callers reach it, each a
hook-fire site: direct clean-close (`close_buffer`), the Discard arm
(`prompts::resolve_prompt` → `PromptAction::CloseDiscard`), and post-save close
(`jobs_apply::apply_result` → `PostSaveAction::CloseBuffer`). **All three main-thread, synchronous,
`&mut Editor` — a live borrow held.**

> **Architectural consequence (GAP #5):** none of the three sites can call Lua inline (a live borrow
> is held; the pump's invariant is "no outer borrow held"). The lower-risk, higher-consistency route
> is **enqueue onto a new event queue** (mirroring `Registry::dispatch`'s `HandlerKind::Plugin` arm,
> which pushes a `PluginCall` and runs no Lua) and **drain it via a pump-like pass placed AFTER the
> borrow drops** — most naturally alongside the existing `plugin_host.pump(&editor)` call in the run
> loop, which already runs post-`reduce` with no outer borrow held.

### 3.2 The pump — the mechanism events reuse

`PluginHost::pump(&mut self, editor: &Rc<RefCell<Editor>>)` (`wordcartel/src/plugin/host.rs`),
two-phase single-pass: **Phase A** short `borrow_mut` + `std::mem::take(&mut e.pending_plugin_calls)`
then drop; **Phase B** invoke each callback with **no outer borrow held**, each wrapped in
`panicx::catch(...)` + `self.with_time_guard(lua, || cb.call::<()>(()))`. Callback lookup by registry
key `wc-cmd-<CommandId.0>`. Editor handed as `&Rc<RefCell<Editor>>` (never `&mut`) so a `wc.*` closure
can re-borrow via `try_borrow`/`try_borrow_mut`, degrading to a typed "editor busy" Lua error on
genuine re-entry (test `editor_busy_on_nested_reentry_degrades_not_panics`).

`Registry::dispatch` `HandlerKind::Plugin` arm pushes `PluginCall{id}` onto
`editor.pending_plugin_calls` and returns `CommandResult::Handled` — **no Lua runs at dispatch**.

**P2 event seam:** a new `pending_plugin_events: VecDeque<PluginEvent>` on `Editor`, drained by a
sibling Phase-A/B pass (a second method or an extended `pump`) reusing the exact "drain under short
borrow, invoke with none held, `panicx::catch` + `with_time_guard` per invocation" shape. Since hooks
are OBSERVER-ONLY (§2), the invoked closure needs read + status access but NOT `try_borrow_mut` for
edits.

**Chain cap: NONE today** (pump doc comment: "a re-drain loop and a chain cap are P2 concerns").
Correct for P1 (nothing can enqueue mid-pump). `wc.command` and event-enqueue-from-hook create the
first mid-cycle-cascade risk → P2 needs a re-drain loop + chain cap.

### 3.3 Time/resource guards — LOAD PHASE IS UNGUARDED (GAP #1)

`host.rs`: `TIME_BUDGET = 150ms`; `with_time_guard(lua, f)` installs
`lua.set_hook(every_nth_instruction(10_000), ..)` aborting once `elapsed > TIME_BUDGET`, RAII
`HookGuard(&Lua)` whose `Drop` calls `remove_hook()` on both normal return and unwind.
`spike_confirmed_mem_cap() -> Some(64<<20)` applied once in `PluginHost::new()` via
`set_memory_limit` — **VM-wide, so it DOES bound load-phase allocation**.

But `with_time_guard` is called from **exactly one place: `pump` Phase B**. The load path
`load::load_one` runs `lua.load(src).set_name(stem).exec()` **completely unguarded**. `load_sources`
wraps `load_one` in `panicx::catch` — a panic backstop, NOT a time guard (`catch_unwind` cannot
interrupt an infinite loop). **A `while true do end` at a plugin's top level hangs the whole editor at
startup**, before the terminal guard is even installed. Plugin load happens in `app::run` between
`Registry::builtins()` and `let reg = reg;` — early. **P2 must guard the load phase** — likely
`with_time_guard` (or equivalent) around the `exec()`, with a **distinct, larger budget constant**
(150ms is too tight for legitimate table-building init). This is a design decision to state, not
inherit.

### 3.4 Registry + reload seam — GAP #3 (frozen, no removal API)

`registry.rs`: `enum HandlerKind{Builtin(Handler), Plugin}` (Plugin carries no payload).
`register_plugin(id: CommandId, label: &'static str, menu: Option<MenuCategory>) -> Result<(),
RegisterError>` is the only plugin-command write path; `RegisterError{Duplicate}` is the sole variant
(collision-only). `resolve_name(name: &str) -> Option<CommandId>`. Storage:
`Registry{entries: Vec<CommandEntry>, index: HashMap<CommandId, usize>}` — **builtins and plugin
commands share the same Vec/map**, separable only by filtering `entry.handler` for `Plugin`.

- **NO unregister/removal API** (`grep unregister|remove|retain` → nothing). Reload needs a new
  method (e.g. `retain_builtins()`) that filters `entries`/`index` to `Builtin` only, **re-indexing
  survivors** (positions shift), then re-runs `load_sources`.
- **`reg` is FROZEN in `app::run`** (`let reg = reg;` after load) and threaded by reference through
  `reduce`/keymap-rebuild — a plain value, not `Rc<RefCell>`. In-place reload requires either interior
  mutability on `reg` or rebuilding it fresh and threading the new value back into the loop-local
  bindings. **This is the real work of reload; it is a restructuring, not just an added method.**
- On reload, also: drain/clear `editor.pending_plugin_calls` (stale `CommandId`s → torn-down VM), and
  rebuild the keymap.
- **Keymap re-resolution is ALREADY solved.** `keymap::build_keymap(km, reg)` resolves patches via
  `reg.resolve_name` (unresolved → silently skipped + warning string). `theme_cmds::
  rebuild_keymap_if_requested(editor, patches, reg)` runs every loop iteration right after
  `pump`, gated on `editor.keymap_rebuild`. So after a reload mutates/replaces `reg`, setting
  `editor.keymap_rebuild = true` re-resolves patches against the post-reload command set via the
  existing wiring. The gap is entirely on the Registry-mutation side.

### 3.5 Interning — NOT commit-atomic (GAP #2)

`plugin::intern(s) -> &'static str` — one process-global `Mutex<HashSet<&'static str>>`, de-dupes by
value (`Box::leak` only on first sighting; re-interning equal string returns same pointer).

But `intern` is called **eagerly, per `wc.register_command` call, DURING `exec()`**: in `api.rs`'s
registration closure, `CommandId(intern(&full))` and `intern(label)` fire as the script runs.
`load_one`'s atomic preflight (count cap + collision check + commit-all-or-none) runs **only AFTER
`exec()`**. So a plugin whose 5th command collides commits **zero** commands to the Registry (atomic)
**but has already leaked all 5 (id, label) string pairs**. Bounded per-attempt by the length/count
caps, but a **permanent leak on every failed-preflight load** — and P2's `plugins_reload` makes it
worse (an author iterating on a broken plugin leaks a fresh batch each reload). **P2 fixes
intern-on-commit**: intern at commit time, or stage raw strings through preflight and intern only the
survivors.

### 3.6 Config — no `dir` field (GAP #6); per-plugin table seam

`config.rs`: `PluginsConfig{enabled: bool, disable: Vec<String>}` (default `enabled: true`); raw
mirror `RawPlugins{enabled: Option<bool>, disable: Option<Vec<String>>}`; folded per-field in `load()`
(disable REPLACES per layer). **No `dir` field anywhere.** The plugins dir is hardcoded at ONE site:
`app::run` builds `<dirs::config_dir()>/wordcartel/plugins`; if `config_dir()` is `None` the whole
load block is skipped silently (no warning).

Config load: layered TOML via `config_layer_paths` + `load(paths)` (`toml::from_str::<RawConfig>`,
per-field fold, bad layer skipped with warning). **Per-plugin `[plugins.<name>]` seam:** add a field
on `RawPlugins`/`PluginsConfig` like `settings: BTreeMap<String, toml::Value>` (mirrors
`RawTheme.styles: BTreeMap<String, RawFace>`; `[plugins.<name>]` sub-tables deserialize naturally).
Hand to the plugin by converting that plugin's `toml::Value` to an `mlua::Table` and **injecting at
load time** (a global before `exec`, or a `wc.config` read) — requires `Config`/`PluginsConfig` to
flow from `app::run` into `load::load_sources`/`load_one`, which today take no config beyond `disable`.

`limits.rs` `PLUGIN_MAX_*`: `COMMANDS_PER_PLUGIN=256`, `STEM_LEN=64`, `NAME_LEN=128`, `LABEL_LEN=256`,
`STATUS_LEN=4096`, `SOURCE_BYTES=1MiB`; edit text reuses `PASTE_MAX_BYTES=8MiB`. **No event/config
caps yet** — P2 adds new caps here (event name len, event payload, config table size/depth) following
the file's "one auditable place" convention.

### 3.7 Loader — no same-stem dedup (GAP #4)

`load.rs`: `load_sources(reg, host, sources: &[(String,String)]) -> Vec<LoadReport>` (fs-free core;
null-host early-out; per-pair `load_one` in `panicx::catch`). `load_one(reg, lua, stem_raw, src) ->
Result<usize, String>` (stem cap+intern → fresh sink + exec → preflight ALL → commit all-or-none).
`discover(dir, disable) -> Discovered{sources, skipped}` scans `<name>.lua` files AND
`<name>/init.lua` dirs, disable-filters by stem, bounded-reads via `bounded_read_opt(.., 1MiB)`,
sorts candidates lexicographically by stem (stable sort).

**No same-stem dedup:** if both `foo.lua` and `foo/init.lua` exist, **both are pushed and both
loaded** as independent `load_one` calls under the identical stem. Same-named commands → second
collides + fails atomically with no cross-file diagnostic; differently-named → both silently coexist
under one namespace. Untested (`discover_reads_single_file_and_dir` uses different stems). **P2 decides
explicit policy** (reject ambiguous / prefer one form / document allowed) + adds a test.

### 3.8 Budgets + startup wiring

`app.rs` production = **886/1000** (114 headroom; the budget "most at risk" — plugin arms/hooks MUST
register into a seam, not grow `reduce`/`run`). `plugin/host.rs` = **203/400**. In `app::run`:
`Registry::builtins()` → plugin load (`PluginHost::new`/`discover`/`load_sources`) → `let reg = reg;`
(freeze) → `build_keymap(&cfg.keymap, &reg)` → `Rc::new(RefCell::new(editor))` →
`plugin_host.attach_bridge(...)` → loop: `reduce(..)` → `plugin_host.pump(&editor)` →
`rebuild_keymap_if_requested(..)`. Load is BEFORE `build_keymap` so plugin commands get free bindings.

---

## 4. The seven gaps P2 must resolve (spec author: address each explicitly)

1. **Load-phase runaway (§3.3)** — guard `load_one`'s `exec()` with a distinct, larger budget.
2. **Intern-on-commit leak (§3.5)** — intern only committed survivors.
3. **Registry mutation for reload (§3.4)** — add removal/rebuild path; make `reg` mutable/replaceable
   in the run loop; drain `pending_plugin_calls`; trigger keymap rebuild. The heaviest task.
4. **Same-stem dedup (§3.7)** — decide + implement + test a policy.
5. **Event sites hold a borrow (§3.1)** — enqueue-and-drain via a pump-like pass, never inline Lua.
6. **No `[plugins].dir` (§3.6)** — add the config option (contract: it's a real option), keep the
   `None`-XDG behavior sane (warn, not silent).
7. **Pump chain cap (§3.2)** — add a re-drain loop + chain cap once hooks/`wc.command` can cascade.

---

## 5. Reference (P1 artifacts, for the author)

- P1 spec: `docs/superpowers/specs/2026-07-11-effort-p1-plugin-commands-design.md` (the two design
  LAWS — input-validation + resource-bound — and the §7 audit style to mirror).
- P1 plan: `docs/superpowers/plans/2026-07-11-effort-p1-plugin-commands-plan.md`.
- P1 grounding: `docs/design/effort-p-grounding.md`. Decomposition memory:
  `wordcartel-effort-p-decomposition`. Command-surface contract:
  `docs/design/command-surface-contract.md`.
