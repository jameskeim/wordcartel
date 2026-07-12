# Effort P2 — Plugin events + per-plugin config + reload: breadth on the proven P1 spine

**Status:** SPEC (2026-07-12). Effort **P** phase 2 of 3. Extends the shipped P1 system
(`docs/superpowers/specs/2026-07-11-effort-p1-plugin-commands-design.md`) with events/hooks,
per-plugin config, reload, `wc.command`, and three confirmed hardening fixes. P3
(async/timers/parameterized commands) remains out of scope — see the explicit NOT-in-P2 list.

Binding constraint sources (authoritative, unchanged): `CLAUDE.md` (project law),
`docs/design/command-surface-contract.md`, and the P2 grounding's §2 binding constraints
(`docs/design/effort-p2-grounding.md` — hook power = observer-only; hooks never abort/delay;
`on_save` fires after a successful write; reload = whole-VM teardown; per-plugin config = opaque
table; `wc.command` allowed under a chain cap; contract conformance mandatory). Real code surface
verified against the live tree 2026-07-12 (`plugin/{mod,host,api,load}.rs`, `registry.rs`,
`app.rs`, `config.rs`, `save.rs`, `workspace.rs`, `session_restore.rs`, `theme_cmds.rs`,
`limits.rs`, `jobs_apply.rs`, `prompts.rs`). Anchors are cited by symbol NAME — re-locate by
name, not line.

Both P1 design LAWS remain binding on every new surface in this spec:

> **LAW (input-validation).** Every plugin API that accepts a byte offset or range MUST
> pre-validate it against the live buffer via `plugin_check_range` and degrade to a typed Lua
> error. No raw plugin-supplied offset ever reaches an asserting core primitive.

> **LAW (resource-bound).** Every plugin-supplied string that crosses into a permanent leak or a
> Rust allocation MUST be bounded — a hard plugin-layer cap or an existing buffer-memory bound —
> checked/justified BEFORE the allocation.

P2 adds **no** new offset/range-taking API (§10 audits this), so the input-validation LAW is
inherited unchanged through the existing `plugin_check_range` chokepoint; the resource-bound LAW
gets seven new entries (§10's table).

---

## 1. Goal & scope

**Goal.** Give the proven P1 spine its breadth: a plugin can now *react* to the editor
(`wc.on` hooks at save/open/buffer-close), *be configured* (`[plugins.config.<name>]` TOML handed over
as an opaque Lua table), *invoke commands* (`wc.command`, through the same registry dispatch),
and *be iterated on live* (`plugins_reload` — whole-VM teardown + re-load — plus `plugin_list`).
No new load-bearing architecture: every mechanism reuses the P1 model (`Rc<RefCell<Editor>>`
handle, main-thread pump, `panicx::catch` at every host→Lua entry, `with_time_guard`, the
`limits.rs` cap discipline, atomic-per-plugin load).

**Success demo.** A `wordcount.lua` in the plugins dir registers an `on_save` hook that reads the
buffer and sets status `"Saved — 1,234 words"`, configured by `[plugins.config.wordcount]
min_words = 100`. The author edits the script, runs `plugins_reload` from the palette, and the new behavior
is live — same session, no restart, keybindings re-resolved.

### In scope (P2)
- **Events/hooks**: `wc.on(event, fn)` for `"save"` / `"open"` / `"buffer_close"`, observer-only,
  fired via a new `Editor.pending_plugin_events` queue drained by the pump (§3).
- **Per-plugin config**: `[plugins.config.<name>]` → an opaque `mlua::Table` readable as `wc.config`
  during that plugin's load (§4).
- **`wc.command(name)`**: enqueue-and-route through `Registry::dispatch` — never a side channel
  (§5).
- **Pump chain cap + re-drain loop** — required now that hooks and `wc.command` create the first
  mid-cycle cascades (§5c).
- **Reload**: `plugins_reload` (whole-VM teardown, registry rebuild, keymap re-resolve) and
  `plugin_list`, both ordinary registry commands (§6).
- **`[plugins].dir`** config key + a warning (not silence) when no plugins dir can be resolved
  (§4d).
- **Three hardening fixes**: load-phase runaway guard, atomic-on-commit registration
  (intern + Lua callback key), same-stem dedup (§7).
- **New limits** in `limits.rs`: hook count, event payload, config depth/size, dispatch-queue
  caps (§8).

### NOT in P2 (deferred) — explicit
- **Mutation from a hook** (format-on-save class) — a deliberate deferred fast-follow per the
  binding constraints. Hooks read + emit status ONLY (§3e enforces it mechanically).
- **Hook-aborted operations** (a save "veto") — architecturally excluded by the binding
  constraints, not merely deferred: the op has already happened by the time the hook runs.
- **New event kinds** beyond the three (no `on_key`, no `on_change`, no timers) — hot-path hooks
  stay excluded (P1 §1); timers/periodic work are P3.
- **Plugin async / `spawn_process`** → P3.
- **Parameterized commands** (`wc.command("set_scrollbar", "off")`) → P3; `wc.command` takes a
  name only, matching the contract's nullary-today rule 10.
- **Per-plugin side-effect teardown** — reload is whole-VM rebuild by binding constraint; no
  compensating bookkeeping.
- **Config schema/validation on the host side** — the table is opaque by binding constraint; the
  plugin self-validates.
- **Whole-config hot reload** — `plugins_reload` re-reads the `[plugins]` section only (§6f,
  flagged); nothing else in `Config` changes at runtime.

---

## 2. Architecture & components (what changes where)

New/changed surface, all inside the existing module family (nothing plugin-specific enters
`wordcartel-core`):

- **`plugin/mod.rs`** — adds `PluginEvent` + `PluginEventKind` (+ `event_from_str`, the
  parse-to-enum mirror of `registry::menu_from_str`), `PluginDispatch`, `PluginRecord`, and the
  `fire_event` helper (§3c). `PluginCall`, `intern`, `plugin_error`, `cap_status` unchanged.
- **`plugin/host.rs`** — the pump grows a dispatch context, a unified re-drain loop, and the
  chain/time caps (§5c); the host gains the hook table (`hooks: Vec<HookEntry>`, owned `String`
  keys — deliberately NOT interned, they die with the VM at reload) and the shared
  `InvokeState` (§3e). `with_time_guard` gains a budget parameter so load and callbacks use
  distinct budgets (§7a).
- **`plugin/api.rs`** — adds `install_on` (load-time hook registration + its closed post-load
  stub), `install_command` (`wc.command`), and the observer-mode checks in the edit closures.
- **`plugin/load.rs`** — `load_sources`/`load_one` take `&mut PluginHost` (hooks commit into the
  host) and the per-plugin `config` map; `discover` gains the same-stem dedup (§7c); `PendingReg`
  carries raw `String`s + the callback `mlua::Function` for atomic-on-commit registration (§7b).
- **`plugin/settings.rs` (new)** — `toml::Value` → `mlua::Value` conversion under the config
  caps, and the per-plugin `wc.config` install/clear (§4).
- **`plugin/reload.rs` (new)** — `load_phase` (the startup plugin-load block extracted verbatim
  from `app::run`, shared by startup and reload) and `perform_reload` (§6). This extraction
  makes P2's net `app.rs` growth approximately zero (§11).
- **`registry.rs`** — `Registry::retain_builtins` (the sole new mutation method, §6b) and the
  two new builtins `plugins_reload` / `plugin_list` (flag-setting + inventory-formatting
  handlers, the same thin shape as `switch_keymap_preset`).
- **`config.rs`** — `PluginsConfig`/`RawPlugins` gain `dir: Option<PathBuf>` and a plain typed
  `config: BTreeMap<String, toml::Value>` holding `[plugins.config.<name>]` tables (namespaced,
  not flattened — §4a resolves the typed-field collision).
- **`editor.rs`** — four new fields beside the existing `pending_plugin_calls`:
  `pending_plugin_events: VecDeque<PluginEvent>`, `pending_plugin_dispatch:
  VecDeque<PluginDispatch>`, `plugins_reload_requested: bool` (the `settings_save_requested` /
  `keymap_rebuild` flag pattern), and `plugin_inventory: Vec<PluginRecord>` (§6e).
- **`app.rs`** — the startup load block (currently inline between `Registry::builtins()` and the
  `let reg = reg;` freeze — the `PluginHost::new`/`discover`/`load_sources` match arm) moves OUT
  to `plugin/reload.rs::load_phase`; the freeze becomes `let mut reg` (§6a); the pump call gains
  its dispatch-context arguments; a one-line post-pump `hydrate_overlays` call (§5b — hydrates a
  `wc.command`-opened overlay) and a reload-seam CALL (whose BODY lives in
  `plugin/reload.rs::perform_reload`) land between the pump and `rebuild_keymap_if_requested`
  (§6d). Fire-site one-liners land in `save.rs` / `workspace.rs` / `session_restore.rs`, not in
  `app.rs`. Net `app.rs` line delta is a TARGET, not an asserted count — §11 states the
  budget-gate mechanism.

---

## 3. The event system

### a. Event model — data captured at fire time, drained at pump time

```rust
/// The three P2 event kinds. Exhaustive on purpose (house pattern-matching rule): adding a
/// kind is a deliberate act every match must handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginEventKind { Save, Open, BufferClose }

/// One fired event, queued on `Editor.pending_plugin_events`. Payload is OWNED data captured
/// at the fire site — by drain time the buffer may be gone (close) or changed (save), so the
/// event must not hold references into editor state.
#[derive(Clone, Debug)]
pub struct PluginEvent {
    pub kind: PluginEventKind,
    /// The affected file path (lossy string, clamped to `PLUGIN_MAX_EVENT_PAYLOAD` via
    /// `cap_status`), or `None` (an unnamed buffer).
    pub path: Option<String>,
}
```

`event_from_str(s: &str) -> Option<PluginEventKind>` maps `"save"`/`"open"`/`"buffer_close"` —
the parse-to-enum bound (the `menu_from_str` precedent): an unknown event name is a typed Lua
error at `wc.on` time, never stored, never interned. The enum IS the length/content cap.

The hook callback receives one Lua table argument: `{ kind = "save"|"open"|"buffer_close",
path = <string>|nil }`. Deliberately minimal — adding fields later is additive/compatible;
removing them would not be.

### b. `wc.on(event, fn)` — load-time registration, atomic with the plugin's commit

- **Shape:** positional `wc.on("save", function(ev) … end)`. (The grounding's `wc.on{…}`
  notation is not API law; two required arguments with no optional fields gain nothing from a
  spec-table — one-line rationale, decided here.)
- **Load-time only**, exactly like `wc.register_command`: `install_on` installs the real
  collector for one plugin's exec pass; `install_editor_api` (which runs at
  `PluginHost::attach_bridge`, the existing load→callback boundary) overwrites it with a stub
  that raises `"wc.on is only available during plugin load"` — the
  `install_registration_closed` pattern applied to the second registration verb.
- **Collection + caps:** hooks collect into a per-plugin sink
  (`Rc<RefCell<Vec<(PluginEventKind, mlua::Function)>>>`) beside the existing `PendingReg` sink.
  The N+1'th `wc.on` past `PLUGIN_MAX_HOOKS_PER_PLUGIN` is a typed error (resource-bound LAW —
  each hook stores a Lua function in the VM registry plus a Rust-side `HookEntry`).
- **Atomic per plugin:** hooks commit together with the plugin's commands, only after
  `load_one`'s preflight passes. A plugin whose commands fail preflight registers ZERO hooks —
  the P1 all-or-nothing guarantee now covers both registration verbs.
- **Storage:** on commit, each callback is stored in Lua's named registry under
  `wc-ev-<stem>-<i>` (`i` = per-plugin hook index) and the host records
  `HookEntry { kind: PluginEventKind, key: String, label: String }` (label =
  `"<stem>.on_<kind>"`, for `plugin_error` attribution). Keys and labels are **owned `String`s
  on the host — never interned**: unlike command ids (which live in the process-lifetime
  `Registry`/keymap), hooks die with the VM at reload, so interning them would be a
  reload-shaped leak. Invocation order = host `Vec` order = lexicographic-stem load order, then
  within-plugin registration order — deterministic.

### c. The three fire sites — enqueue only, never inline Lua (grounding gap 5)

All three sites run on the main thread holding a live `&mut Editor`; the pump's invariant is "no
outer borrow held when Lua runs." So a fire site does exactly one thing: push a `PluginEvent`
onto `editor.pending_plugin_events` — a plain field push, no Lua, no re-entrancy. The shared
helper (new, `plugin/mod.rs`):

```rust
/// Capture-and-enqueue an event at a fire site. Cheap (one clamp + one push), cold-path only
/// (save/open/close — never per-keystroke), drained the same frame by the pump. The path is
/// clamped to `PLUGIN_MAX_EVENT_PAYLOAD` at capture (resource-bound LAW: the queue holds
/// bounded owned data even for a pathological path).
pub(crate) fn fire_event(editor: &mut Editor, kind: PluginEventKind, path: Option<&std::path::Path>)
```

The sites (verified anchors):

1. **`on_save`** — inside `save::do_save_to`'s merge closure (`save.rs`), keyed on the
   `Ok(SaveOutcome::Saved) | Ok(SaveOutcome::Unchanged)` outcome.
   **Implementation constraint (Codex-flagged borrow choreography).** The merge closure already
   mutates buffer state *inside* an `if let Some(b) = editor.by_id_mut(buffer_id)` block and
   assembles `status` into a local so the `b` borrow ends before `editor.status = status` runs
   after the block. The `fire_event(editor, Save, …)` call takes `&mut editor` too, so it MUST
   run **after the `by_id_mut` block closes** — never inside it (a live `b: &mut Buffer` borrow
   would conflict). And the **saved path must be captured from the closure's OWN owned data, not
   from `b`**: the merge closure already captures `target` by move (it uses `target.clone()` in
   the SaveAs arm), and since `write_path = target.clone()` at the top of `do_save_to`, `target`
   IS the path written in every mode (Normal saves the doc's own path passed in as `target`;
   SaveAs the re-key target) — known independently of whether the buffer still exists. So compute
   `let fire_save: Option<PathBuf> = matches!(outcome, Ok(SaveOutcome::Saved |
   SaveOutcome::Unchanged)).then(|| target.clone())` from the closure's owned values, and after
   the `if let Some(b)` block (and after `editor.status = status`) do
   `if let Some(p) = fire_save { fire_event(editor, Save, Some(&p)); }`. This makes the two
   deliberate semantics fall out correctly:
   - fires on `Unchanged` as well as `Saved` — both are the user-visible "Saved" outcome (the
     hook observes "a save completed", not "bytes moved").
   - fires even when `by_id_mut(buffer_id)` is `None` (buffer closed while the save job was in
     flight): the path comes from the closure's owned `target`, not `b`, so a gone buffer does
     not suppress the event — the write DID succeed. The `Err(e)` arm sets no `fire_save`, so it
     fires nothing.
   The merge runs inside `reduce` (via `jobs_apply::apply_result` → `(r.merge)(editor)`), so the
   event is drained by the pump in the same frame.
2. **`on_open`** — in the two success arms of the open seams: `workspace::open_as_new_buffer`
   (`workspace.rs`, its own `Buffer::from_file` `Ok(b)` arm) and
   `session_restore::open_into_current` (`session_restore.rs`, its `Ok(b)` arm). No double-fire:
   `open_as_new_buffer`'s throwaway-reuse branch *returns after delegating* to
   `open_into_current`, so exactly one of the two arms runs per open. The only production caller
   today is the file browser's Enter (`file_browser.rs`); an `OpenError` fires nothing.
   Startup is a flagged fork — see §13 flag 1.
3. **`on_buffer_close`** — in `workspace::close_buffer_now` (`workspace.rs`), immediately beside
   the existing `editor.diag_provider.notify_close(id)` call — the established "buffer is truly
   going away" notification point, reached by all three close shapes (clean close, the
   `PromptAction::CloseDiscard` arm in `prompts::resolve_prompt`, and
   `PostSaveAction::CloseBuffer` in `jobs_apply::apply_result`). The path is captured BEFORE the
   slot is removed/replaced. Fires for the last-ordinary-replacement shape too (that buffer is
   closing even though a fresh one takes its slot). Does NOT fire at app exit — quitting is not
   closing (documented; matches `notify_close`, which also only fires through
   `close_buffer_now`).

### d. Drain seam — the pump extends, no second pipeline stage

Events drain inside the existing `PluginHost::pump`, not a sibling pass. Rationale (one line):
hooks and `wc.command` can each enqueue work for the other, so one loop with ONE cap must govern
the whole cascade — two independent drains would each need their own cap and could ping-pong
between stages unbounded. §5c specifies the unified loop.

Per-invocation treatment is identical to P1 command callbacks: look up the stored function by
its named-registry key, invoke inside `crate::panicx::catch` + `with_time_guard(lua,
CALLBACK_TIME_BUDGET, …)`, route any failure through `plugin_error(editor, <hook label>, msg)`.
One hook's failure never skips the remaining hooks for that event (per-invocation isolation),
and — binding constraint — the save/open/close **already happened**: there is nothing to abort,
delay, or roll back. The hook system is physically incapable of blocking the operation because
it observes a queue written after the operation completed.

**Null-host discipline (new, Codex-shaped detail):** in P1 a null host could early-return
because nothing could ever enqueue a `PluginCall` without a live registry `Plugin` entry. In P2
the fire sites push events **unconditionally** — under `--no-plugins` the queues would grow
without bound. So the null-host pump now **clears** `pending_plugin_events` /
`pending_plugin_dispatch` / `pending_plugin_calls` (a `VecDeque::clear` under one short borrow)
instead of returning untouched. Fire sites stay unconditional on purpose: they are cold-path
O(1) pushes drained (or cleared) the same frame; gating them on host state would couple
`save.rs`/`workspace.rs` to plugin wiring for no measurable win.

**Resource behavior:** events are edge-triggered by real operations (a save, an open, a close) —
never wall-clock. An idle editor fires nothing, enqueues nothing, and the pump's empty-queue
check is one `is_empty` under a short borrow. `timers::next_wake` is untouched — no new wake
source, idle stays free. The P1 loaded-but-idle guardrail test extends to hooks (§12).

### e. Observer-only enforcement — mechanical, not documentation

The binding constraint (hooks read + emit status; NO editing) is enforced by the API layer, not
by trust. The `Bridge` gains one shared cell:

```rust
/// What the pump is currently invoking, shared with every wc.* closure (each captures a clone).
/// `current` names the plugin command/hook for wc.command attribution + error messages;
/// `observer` is true exactly while a HOOK callback runs — the edit APIs and wc.command check
/// it and degrade to a typed error (the observer-only binding constraint, enforced in code).
pub(crate) struct InvokeState { current: Option<String>, observer: bool }
// held as Rc<RefCell<InvokeState>> on the Bridge
```

The pump sets it around every invocation via an RAII guard (reset on drop — normal return and
unwind alike, the `HookGuard` pattern), so a panicking hook cannot leak observer mode onto the
next command callback. Checks:

- `wc.insert` / `wc.replace` / `wc.set_selection` → when `observer`, typed error
  `"plugin: editing is not allowed from an event hook"` — before any borrow, cap check, or
  allocation.
- `wc.command` → when `observer`, typed error `"plugin: wc.command is not allowed from an event
  hook"`. Rationale: a command can mutate anything (that is its purpose), so hook→command is
  mutation-by-proxy — and `on_save`→`save` would self-cascade. Mutation-on-event arrives as the
  designed fast-follow, not through a loophole.
- Reads (`wc.text`/`selection`/`cursor`/`len`/`version`/`path`) and `wc.status` → allowed
  unchanged. Note the reads observe the **live post-operation** state (e.g. an `on_buffer_close`
  hook's `wc.path()` reads the *newly active* buffer — the closed one is already gone; its path
  arrives in the event payload). Documented, and exactly why payloads are captured at fire time.

---

## 4. Per-plugin config — `[plugins.config.<name>]` → `wc.config`

### a. Config shape (`config.rs`) — per-plugin config is namespaced, NOT flattened

Per-plugin tables live under a dedicated `config` sub-key — `[plugins.config.<name>]` — NOT
directly under `[plugins.<name>]`. `RawPlugins` carries a plain typed field, no `#[serde(flatten)]`:

```rust
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawPlugins {
    enabled: Option<bool>,
    disable: Option<Vec<String>>,
    dir: Option<std::path::PathBuf>,
    /// `[plugins.config.<name>]` sub-tables — one opaque `toml::Value` per plugin stem.
    config: std::collections::BTreeMap<String, toml::Value>,
}
```

**Decisive rationale (Codex-flagged collision — resolved by construction).** A flattened
`[plugins.<name>]` map shares the `[plugins]` table namespace with the typed `enabled` /
`disable` / `dir` keys: a plugin literally named `enabled`/`disable`/`dir` would have its config
swallowed by (or fail-parse against) the typed field of that name. Two escapes exist — reserve
those three stem names, or namespace the per-plugin tables under a non-colliding sub-key. P2
takes the sub-key (`[plugins.config.<name>]`): it is collision-proof for EVERY plugin name
(imposes **no** restriction on valid plugin stems — the contract-visible fact, §9), it drops the
fragile flatten-plus-typed-fields interaction entirely, and it keeps `config` a plain typed
`BTreeMap` folded with the existing per-field pattern. Cost: config is spelled
`[plugins.config.wordcount]` rather than `[plugins.wordcount]` — one extra path segment,
documented; the grounding's `[plugins.<name>]` sketch was illustrative ("a field … like"), not
API law.

`PluginsConfig` gains `dir: Option<PathBuf>` and `config: BTreeMap<String, toml::Value>`. Fold
rules in `config::load` (the existing per-field pattern at the `raw.plugins` fold, lines around
`if let Some(v) = raw.plugins.enabled …`):

- `dir`: per-field override, like `enabled`.
- `config`: **per-plugin-name REPLACE per layer** — a higher layer's `[plugins.config.foo]`
  wholly replaces a lower layer's; other plugins' tables are untouched (iterate `raw.plugins.config`,
  `cfg.plugins.config.insert(k, v)` per key). Rationale: the table is opaque (binding constraint —
  the host does not know its schema), so a deep merge could stitch together halves of two
  incompatible shapes; replace-whole matches `disable`'s REPLACE semantics. (Contrast:
  `theme.styles` accumulates per-key, but those values are flat known `RawFace`s.)

### b. Hand-off — `wc.config`, valid during that plugin's load

`load_one` (which now receives the plugin's `Option<&toml::Value>`, looked up by stem in
`cfg.plugins.config`) installs `wc.config` before `exec()`: the plugin's table converted to an
`mlua::Table`, or nil when no `[plugins.config.<stem>]` section exists. The idiom is documented as:

```lua
local cfg = wc.config or {}   -- capture at load; self-validate your own shape
```

At `attach_bridge` time (the load→callback boundary, where `wc.register_command` is closed),
`wc.config` is **cleared to nil** — otherwise the last-loaded plugin's table would linger on the
shared `wc` global for every later callback (the exact stale-sink hazard
`install_registration_closed` exists to prevent, applied to data instead of a function). A
plugin that wants its config at callback time captures it in a local — the captured Lua table
reference stays alive through the closure, GC-safe. One-line rationale for read-at-load rather
than a callback-time getter: config is startup-layered TOML that cannot change until
`plugins_reload` — which re-runs load anyway — so a live getter would only ever return the same
value the load saw.

### c. Conversion + caps (`plugin/settings.rs`, new)

`config_to_lua(lua, &toml::Value) -> Result<mlua::Value, String>` — recursive, exhaustive over
`toml::Value` variants (string/integer/float/boolean → the Lua scalar; datetime → its string
form; array → a sequence table; table → a table). **Four caps, each checked BEFORE the Lua
allocation it bounds** (grounding §3.6's "new caps … config table size/depth", plus the
resource-bound LAW's byte dimension — see the note below):

- depth > `PLUGIN_MAX_CONFIG_DEPTH` → `Err`;
- total converted nodes > `PLUGIN_MAX_CONFIG_NODES` → `Err`;
- any single string VALUE's byte length > `PLUGIN_MAX_CONFIG_STR` → `Err` (checked on the
  `&str` from `toml::Value::String` BEFORE `lua.create_string`, which would allocate it into the
  Lua heap);
- any table KEY's byte length > `PLUGIN_MAX_CONFIG_STR` → `Err` (checked on the key `&str`
  BEFORE it is set into a Lua table — keys are strings too and equally attacker-controlled).

**Why a byte cap is REQUIRED, not optional (Codex round-3 Important — resource-bound LAW).**
`config::load` reads each layer with an **unbounded** `fs::read_to_string` + `toml::from_str`
(config is a deliberately-unbounded config-class file on the Rust side — verified in `config.rs`).
So a *single-node* `[plugins.config.<stem>]` value — one 50 MB string, or one 50 MB key — passes
both the depth and node caps yet forces a large `lua.create_string` allocation during conversion.
The design LAW is explicit: "every plugin-supplied string that crosses into a Rust/Lua allocation
MUST be bounded — checked BEFORE the allocation" — the 64 MiB VM heap cap (`spike_confirmed_mem_cap`)
is a backstop, not the primary bound, and relying on it here would be exactly the LAW's forbidden
"rely on a downstream cap." The per-string / per-key byte cap is that pre-allocation check.

On ANY of the four `Err`s the plugin still loads with `wc.config = nil` plus a load warning
naming the cap (`"plugin <stem>: [plugins.config.<stem>] ignored — <reason>"`). Load-beats-fail
rationale (consistent with the depth/node behavior): the plugin self-validates and falls back to
defaults; the warning keeps it loud. Do NOT abort the plugin's commands/hooks — only its config
is dropped.

### d. `[plugins].dir` + the no-dir warning (grounding gap 6)

The plugins directory becomes: `cfg.plugins.dir` if set (used verbatim as a `PathBuf`; a
relative value resolves against the process CWD — documented, not clever), else the existing
`<dirs::config_dir()>/wordcartel/plugins`. When plugins are enabled but NEITHER source yields a
directory (the `config_dir() == None` case that today skips silently), `load_phase` pushes a
startup warning: `"plugins: no config directory found (set [plugins].dir)"` — warn, not silence,
per gap 6. Contract treatment of `dir` is in §9.

---

## 5. `wc.command` — dispatch through the registry, deferred and capped

### a. Call semantics — fire-and-forget enqueue

`wc.command(name)` (callback time only; a load-time call errors like the other editor APIs —
the API is simply not installed until `attach_bridge`):

1. `observer` check (§3e) — typed error from a hook.
2. Length cap on the borrowed Lua bytes: `name` longer than `PLUGIN_MAX_COMMAND_REF` (=
   `PLUGIN_MAX_STEM_LEN + 1 + PLUGIN_MAX_NAME_LEN` — the longest possible registered id, so the
   cap can never reject a resolvable name) → typed error, nothing allocated.
3. Queue cap: `pending_plugin_dispatch.len() >= PLUGIN_MAX_PENDING_DISPATCH` → typed error
   `"plugin: command queue full"` (resource-bound LAW — a single callback looping on
   `wc.command` must not grow an unbounded `String` queue before the chain cap can even run).
4. Push `PluginDispatch { origin: String /* from InvokeState.current */, name: String }` onto
   `editor.pending_plugin_dispatch` under the closure's own short `try_borrow_mut`. Return
   `()` — **fire-and-forget**.

**Resolution is deferred to the drain**, where `&Registry` lives — deliberately NOT validated at
call time. Rationale: call-time validation would require the closure to hold a name-set snapshot
of the registry, a derived cache that contract law 1 (registry = single source of truth) says
should not exist and that reload would have to keep coherent. Cost: an unknown name surfaces on
the status line (`plugin_error(editor, origin, "unknown command '<name>'")`) instead of as a
`pcall`-able error in the caller — acceptable for a status-line-first editor, and consistent
with `Registry::dispatch`'s own unknown-id behavior.

### b. Drain-side dispatch — the same `Registry::dispatch`, a short borrow, no Lua underneath

The pump (which in P2 receives the dispatch context — §5c) drains each `PluginDispatch`:

- `reg.resolve_name(&d.name)` — `None` → `plugin_error`, continue.
- `Some(id)` → build a `Ctx` under one short `borrow_mut` and call `reg.dispatch(id, &mut ctx)`
  — the SAME path the palette/menu/keymap use (contract law 1 / binding constraint: never a side
  channel). A `Builtin` runs synchronously inside that borrow (builtin handlers never call Lua,
  so no borrow crosses the FFI); a `Plugin` entry enqueues a `PluginCall` back onto
  `editor.pending_plugin_calls` and returns — picked up by the next re-drain iteration. The
  borrow drops before any Lua runs.

Effects flow through the existing between-reduces arms automatically: a dispatched builtin that
sets `keymap_rebuild` / `settings_save_requested` / theme flags is honored the same frame,
because the pump runs before those arms in the loop.

**Overlay hydration for a pump-dispatched overlay open (Codex round-4 Important).** A builtin
that OPENS an overlay (`palette`, `menu`, `theme`, `file_browser`) installs a *placeholder* whose
contents/geometry are filled in a second step: `app::hydrate_overlays(editor, &reg, &keymap)`
(`app.rs`) rebuilds the palette rows / builds the menu groups. Today that step runs
**per-dispatch-path** — the key path (`input.rs`, right after `reg.dispatch`), the
overlay-command/menu-select path (`app::dispatch_overlay_command`), and the mouse path
(`app.rs` `Event::Mouse` arm + `mouse.rs`) each call it after their dispatch. The pump's
`reg.dispatch` is a NEW dispatch path that shares none of those call sites, so without action a
plugin calling `wc.command("palette")` (or `"menu"`/`"theme"`/`"open"`) would open an
**un-hydrated** overlay — empty rows, unbuilt groups, broken geometry.

Resolution (the PREFERRED uniform option — `hydrate_overlays` is **idempotent and self-guarding**:
it rebuilds palette rows only when `rows.is_empty() && query.is_empty()`, and builds the menu only
when `!built`, so re-running it over an already-hydrated overlay is a cheap no-op): add **one**
`hydrate_overlays` call in the `app::run` loop **immediately after `plugin_host.pump(...)`**, using
the loop-local `keymap` and `reg`:

```rust
plugin_host.pump(&editor, &reg, &executor, &clock, &msg_tx);   // §5c signature
// Hydrate any overlay a pump-dispatched wc.command opened — the pump's reg.dispatch is a
// dispatch path with no per-site hydrate, unlike key/menu/mouse. Idempotent + self-guarding,
// so it is a no-op when reduce's own per-path hydrate already ran this iteration.
{ crate::app::hydrate_overlays(&mut editor.borrow_mut(), &reg, &keymap); }
```

This runs once per loop iteration after BOTH `reduce` (whose key/menu/mouse opens are already
hydrated by their per-path calls — the post-pump call is then a guarded no-op over them) AND the
`pump` (hydrating a `wc.command`-opened overlay identically). **No new pump parameter** — the
pump signature stays the §5c four-arg form; hydration reads `&keymap` at the run-loop call site,
not inside the pump (the pump does not take `&KeyTrie`). Placed before the
`rebuild_keymap_if_requested` arm, so an overlay's hints use the same keymap the key-path
hydrate would have used this frame (a `wc.command` that also triggers a preset switch shows the
pre-switch hints for one frame — identical to the existing key-path preset-switch behavior). The
per-path calls in `input.rs`/`dispatch_overlay_command`/`mouse.rs` are LEFT in place (removing them
is an orthogonal cleanup outside P2's scope); the post-pump call is purely additive and its
idempotence makes the overlap harmless.

### c. The unified re-drain loop + chain cap (grounding gaps 5 & 7)

P1's pump was a deliberate single pass ("nothing can enqueue mid-pump"). P2 breaks that premise
twice — a command callback can `wc.command` (→ new calls), and any operation inside a dispatched
builtin can fire events (→ new hook invocations). The P2 pump:

```rust
/// One pump cycle. Drains calls, events, and command-dispatches to quiescence or to a cap.
pub fn pump(&mut self, editor: &Rc<RefCell<Editor>>, reg: &Registry,
            executor: &dyn Executor, clock: &dyn Clock,
            msg_tx: &std::sync::mpsc::Sender<Msg>)
```

- **Loop:** Phase A takes all three queues under ONE short `borrow_mut`
  (`std::mem::take` each); all empty → done. Phase B processes them with no outer borrow held:
  dispatches (§5b), then calls (P1 shape), then events × their hooks (§3d). Then loop —
  Phase B may have enqueued more.
- **Two caps, each checked BETWEEN units — at the top of every loop iteration, before the next
  unit of Phase-B work is dequeued.** Neither preempts a unit already running (see the sharp
  edge below):
  - `PLUGIN_PUMP_CHAIN_CAP` (count) — every processed unit (one dispatch, one command callback,
    one hook invocation) increments one counter. This is the deterministic, testable bound on
    ping-pong cascade *length* (A dispatches B dispatches A …).
  - `PUMP_CYCLE_TIME_BUDGET` (wall clock, whole cycle) — bounds the *total elapsed across all
    units so far*: once the accumulated cycle time exceeds it, no further unit is dequeued. It
    exists because the count cap alone permits `CAP × CALLBACK_TIME_BUDGET` of stall
    (64 × 150 ms ≈ 9.6 s) — many short units summing large — which the count cannot express but
    the wall clock can. It is a *between-units* budget, not a preemptive one.
- **Sharp edge — what bounds a SINGLE unit's duration (not the cycle budget).** Because both
  caps are checked between units, neither can interrupt a unit already executing. The two unit
  kinds are bounded differently, and it matters:
  - A **plugin callback / hook** (Lua) is bounded by the per-invocation `CALLBACK_TIME_BUDGET`
    (150 ms `set_hook` abort) — the real preemption of a running Lua unit, unchanged from P1.
  - A **`wc.command`-dispatched builtin** runs synchronously in Rust under the dispatch borrow;
    NOTHING preempts it mid-run — not `set_hook` (no Lua frame), not the cycle budget (checked
    only between units). Its duration is bounded solely by **the builtin being hot-path-safe by
    construction**: every `Registry` builtin either completes in `O(visible)+O(edited)` or
    dispatches heavy work to the job substrate (project law — "never block the input loop"); a
    `Plugin`-kind dispatch merely enqueues a `PluginCall` (no synchronous Lua). So a
    `wc.command` cannot reach a blocking unit because no builtin is one. The cycle budget bounds
    how MANY such builtins a cascade may chain and their SUM, not any one's length.
- **On trip (either cap):** clear all three queues (one short borrow), set status via
  `plugin_error` (`"plugins: work truncated (chain cap)"`), return. Dropping beats carrying over:
  queued plugin work is advisory (observer hooks, fire-and-forget dispatches) and deferring it to
  the next frame would let a hostile cascade starve every subsequent frame instead of one.

Signature note: the five parameters mirror `reduce`'s existing style
(`reduce(msg, &mut editor.borrow_mut(), &reg, &keymap, &executor, &clock, &msg_tx)`); the call
site in `app::run` changes on one line. The null-host early path clears the queues first (§3d).

---

## 6. Reload — the heavy section

The run-loop today: `Registry::builtins()` → inline plugin load → **`let reg = reg;` (freeze)**
→ `build_keymap(&cfg.keymap, &reg)` → … → loop { `reduce` → `pump` →
`rebuild_keymap_if_requested` → … }. Reload must mutate what the freeze declares immutable.
P2's resolution: **keep `reg` a plain loop-local value, drop the freeze, and mutate it only at a
dedicated between-reduces seam** — the same discipline the keymap swap already uses
(`keymap_rebuild`), extended to the registry. No `Rc<RefCell<Registry>>`, no interior
mutability: `reduce` keeps `&Registry`, dispatch is never live while the registry changes, and
every borrow stays visible inside `run()`. One-line rationale: the frozen-while-dispatching
invariant is the thing worth keeping; the freeze *statement* was only its P1 spelling.

### a. Unfreezing (`app.rs`)

`let reg = reg;` is deleted; the binding stays `let mut reg` from construction through the loop.
The registry remains immutable in fact except inside `perform_reload` (§6d), which runs only at
the seam. `reduce`, the palette, the menu, `build_keymap` — every consumer keeps `&reg`.

### b. `Registry::retain_builtins` (new, `registry.rs`)

```rust
/// Remove every Plugin entry, keeping builtins — the reload teardown's registry half.
/// Rebuilds `index` from the surviving `entries` (positions shift when interior entries are
/// removed, so the old indices are wholesale invalid — never patch them incrementally).
pub fn retain_builtins(&mut self)
```

Implementation shape: `entries.retain(|e| matches!(&e.handler, HandlerKind::Builtin(_)))` —
`matches!(&e.handler, …)` borrows the discriminant (`HandlerKind` is NOT `Copy`; `e` is `&CommandEntry`
inside `retain`, so `matches!(e.handler, …)` would try to move the field out of a borrow and fail
to compile). Then **fully rebuild `index`**: `self.index.clear()`, and re-insert `(e.id, i)` for
every survivor at its NEW position `i` (`for (i, e) in self.entries.iter().enumerate()`) — the old
indices are wholesale invalid once any interior entry is removed, so this is a full rebuild, never
an incremental patch. `registry.rs` stays Lua-free. Unit tests: builtins survive with working
dispatch; plugin ids no longer resolve; re-registering the same plugin id after a retain succeeds
(no ghost index entry); `commands()` order for builtins is preserved (palette stability).

Interned strings from the removed entries stay leaked — accepted and bounded: `intern` de-dupes
by value, so reloading the *same* plugin re-uses the existing `&'static` allocations (zero
growth per reload — the steady-state author-iteration loop leaks nothing new); only *renamed*
ids/labels leak once each, capped per load by the P1 count/length caps. §10 carries the audit
row.

### c. The commands: `plugins_reload` and `plugin_list`

Both are ordinary builtins in `Registry::builtins()` (§9 for contract placement):

- **`plugins_reload`** — handler sets `ctx.editor.plugins_reload_requested = true` and a status
  (`"reloading plugins…"`); the seam does the work between reduces. The flag pattern is exactly
  `switch_keymap_preset` → `keymap_rebuild`. Deferral is what makes *a plugin invoking
  `plugins_reload` via `wc.command`* safe: the requesting callback (and the whole pump cycle)
  completes on the old VM; teardown happens after the pump returns, never under a Lua frame.
- **`plugin_list`** — handler formats `ctx.editor.plugin_inventory` (§6e) into `editor.status`:
  e.g. `"plugins: 2 ok (date, wordcount) · 1 failed (broken: …)"`, naturally clamped by the
  status line (nullary, read-only, no new UI surface).

### d. `perform_reload` (`plugin/reload.rs`, new) — the seam and the sequence

Run-loop seam, placed **after `plugin_host.pump(…)` and before `rebuild_keymap_if_requested`** —
in `app.rs` this is only the guarded CALL; the entire body lives in `perform_reload`
(`plugin/reload.rs`), so `app.rs` gains a handful of lines, not the reload logic:

```rust
if editor.borrow().plugins_reload_requested {
    crate::plugin::reload::perform_reload(&mut plugin_host, &mut reg, &editor,
        &all_paths, cli.no_plugins, &msg_tx);
}
```

(Ordering rationale: after the pump so the request — set by `reduce` or by a pump-dispatched
command — never tears down a VM with Lua frames on the stack or queued work un-drained; before
the keymap arm so the `keymap_rebuild` that reload sets is honored in the SAME iteration.)

`perform_reload` sequence — each step anchored to machinery that already exists:

1. **Clear the flag** (`plugins_reload_requested = false`) under a short borrow.
2. **Re-read `[plugins]` config** (§13 flag 2): `config::load(&all_paths)` (the same layered
   paths + warning handling `run()` used at startup; `all_paths` includes the overrides layer
   and is already CLI-`--no-config`-aware because `config_layer_paths` built it that way), take
   only `.plugins` from the result. Fresh `enabled`/`disable`/`dir`/`config` govern this
   reload; every other `Config` field keeps its startup value.
3. **Tear down**: replace `*host = PluginHost::null()` first (drops the old `mlua::Lua` — every
   named-registry callback, hook function, and `wc.*` closure with its captured
   `Rc<RefCell<Editor>>` clones dies here; no cycles, the VM is not reachable from `Editor`).
4. **Registry teardown**: `reg.retain_builtins()` (§6b).
5. **Drain stale queues**: clear `pending_plugin_calls`, `pending_plugin_events`,
   `pending_plugin_dispatch` under one short borrow. (The pump ran to quiescence immediately
   before the seam, so these are normally already empty — this is defense against the
   cap-truncation path and future re-orderings, and it guarantees no stale `CommandId` from the
   old command set ever reaches the new VM's lookup.) Interned `CommandId` strings are
   `&'static`, so a stale id would not dangle — it would MISRESOLVE against a same-named new
   command or fail lookup; clearing removes the class.
6. **Rebuild**: if `cli.no_plugins` → status `"plugins disabled (--no-plugins)"`, leave the null
   host, skip to step 8. Else if fresh `enabled == false` → status `"plugins disabled by
   config"`, same. Else `PluginHost::new()` (a startup VM failure is retried here — a reload can
   *recover* a null host) → `load_phase(&mut reg, &mut host, &plugins_cfg, …)` — the extracted
   startup block: resolve dir (§4d), `discover(dir, &disable)` (with §7c dedup),
   `load_sources(…)` with the fresh per-plugin `config` map, collect warnings + the inventory.
   `load_phase` owns the §7b fatal path: a **commit-time VM-exhaustion** mid-load reverts the
   whole plugin subsystem — it nulls the host, calls `reg.retain_builtins()` AGAIN (dropping any
   plugin, including an earlier fully-committed one, that this reload's `load_phase` had already
   re-committed onto the freshly-retained registry), clears the plugin queues, and (already
   pending from step 8) leaves `keymap_rebuild` set. On reload the editor thus lands in the same
   clean builtins-only state a failed `PluginHost::new()` would, and a later `plugins_reload`
   retries from a fresh VM. (Note the double-retain is intentional and idempotent: step 4 clears
   the OLD plugin set before rebuild; the fatal path re-clears the partial NEW set on exhaustion.)
7. **Re-attach**: `host.attach_bridge(editor.clone(), msg_tx.clone(), Rc::new(SystemClock))` —
   installs the `wc.*` editor API into the NEW VM, closes registration/`wc.on`, clears
   `wc.config` (§4b) — the same call, the same boundary semantics as startup.
8. **Re-resolve bindings**: set `editor.keymap_rebuild = true`. The existing
   `rebuild_keymap_if_requested(editor, &cfg.keymap.patches, &reg)` arm — which runs next in
   this same iteration — re-runs `build_keymap` against the post-reload registry: patch-bound
   plugin commands that survived re-resolve; ones that vanished are dropped with the existing
   warning (contract law 7, zero new keymap code — grounding: "the keymap half is already
   wired").
9. **Report**: `editor.status` = one-line summary (`"plugins reloaded: 2 ok, 1 failed"`), the
   inventory (§6e) updated for `plugin_list`, load warnings surfaced through the same
   status-line channel startup uses.

Failure containment: every per-plugin failure is a `LoadReport` (the P1 guarantee — atomic per
plugin, batch continues); a whole-VM construction failure leaves the **null host + a builtins-only
registry + a rebuilt keymap** — the editor is fully functional minus plugins, and a later
`plugins_reload` can try again. There is no partial state in which old-VM callbacks are
reachable: the host was nulled (step 3) before anything could fail.

**Interaction matrix** (the cross-task invariants the branch gate should probe):

| In-flight thing | Behavior across reload |
|---|---|
| Save job dispatched pre-reload, merge lands post-reload | The merge closure is host-independent (`jobs_apply` → editor state); its `on_save` fire enqueues a fresh `PluginEvent` drained by the NEW host's hooks — correct: events are data, not old-VM references. |
| `pending_plugin_calls` entry for a removed command | Impossible via the normal path (pump quiesced, then step 5 cleared); the clear is the guarantee, not the hope. |
| A plugin calling `wc.command("plugins_reload")` | Runs to completion on the old VM; teardown at the seam after the pump — never mid-Lua. |
| Keymap patch bound to a plugin command that vanished | Dropped with the existing `build_keymap` warning; re-appears if a later reload restores the command. |
| `[plugins.config.<name>]` edited between reloads | Honored — step 2 re-reads the section (flag 2). |
| Startup `PluginHost::new()` failure, later reload | Retried at step 6 — reload doubles as plugin-system recovery. |

### e. `plugin_inventory` — the data `plugin_list` reads

```rust
/// One discovered plugin's load outcome, for `plugin_list` + reload reporting. Owned Strings,
/// bounded by discover/load caps (stem ≤ PLUGIN_MAX_STEM_LEN; error messages are formatted
/// load errors, clamped at display time by the status-line cap).
pub struct PluginRecord { pub name: String, pub commands: usize, pub hooks: usize,
                          pub error: Option<String> }
```

Written by `load_phase` (startup and every reload — one record per discovered source, including
`skipped` ones with their error; disabled stems are omitted, matching `discover`'s "user opt-out
is not a failure"). To carry both counts, `load_one`'s success payload grows from `Ok(usize)` to
`Ok(LoadStats { commands: usize, hooks: usize })` (new type, `plugin/load.rs`), with
`LoadReport.result` following — a mechanical signature migration the plan sequences with task 5. Lives on `Editor` because builtin handlers reach state only through
`Ctx.editor` — the same reason `pending_plugin_calls` lives there.

### f. Why reload re-reads `[plugins]` (and only `[plugins]`)

P2 ships per-plugin config (§4). If reload used the startup snapshot, the author loop for
config-driven plugins would be "edit TOML → restart the app" — halving reload's reason to
exist. Re-reading ONLY the `[plugins]` section keeps the blast radius one subsystem: no other
config consumer observes a mid-session change, so no other subsystem needs hot-reload
semantics. The asymmetry (plugins fresh, rest stale) is the flagged trade — §13 flag 2.

---

## 7. The three hardening fixes

### a. Load-phase runaway guard (grounding gap 1)

`load_one`'s `lua.load(src).set_name(stem).exec()` is today completely unguarded — a
`while true do end` at plugin top level hangs startup before the terminal guard exists
(`panicx::catch` around `load_one` is a panic backstop, not an interrupt). Fix:

- `with_time_guard` becomes budget-parameterized and callable from the load layer:
  `pub(crate) fn with_time_guard<T>(lua: &mlua::Lua, budget: Duration, f: impl FnOnce() ->
  mlua::Result<T>) -> mlua::Result<T>` (a free fn in `host.rs`; the method forwards or callers
  migrate — plan's choice). The RAII `HookGuard` remove-on-drop semantics are unchanged.
- Two named budgets in `host.rs` (time constants stay beside the guard; sizes/counts stay in
  `limits.rs`):
  - `CALLBACK_TIME_BUDGET = 150ms` — the existing `TIME_BUDGET`, renamed for contrast.
  - `LOAD_TIME_BUDGET = 1s` — wraps each plugin's `exec()` (and nothing else — the
    cap/preflight/commit code after exec is host Rust, not plugin Lua). One-line rationale for
    1 s: an order of magnitude over the callback budget for legitimate init table-building,
    while keeping worst-case startup delay per hung plugin ~1 s (N hung plugins cost N seconds
    — visible, reported, survivable; the old behavior was ∞).
- A tripped guard is a normal `exec` error → the existing `Err` path → `LoadReport`
  `"plugin <stem>: exceeded load time budget"` → skip that plugin, batch continues.
- Reload inherits the guard automatically (same `load_one`).

### b. Atomic registration on commit — intern AND the callback key (grounding gap 2, extended)

Today `api.rs`'s `install_registration` closure, running **during exec** (before `load_one`'s
all-or-none preflight), performs THREE non-atomic side effects per `wc.register_command`:
`CommandId(intern(&full))`, `intern(label)`, and — the one the first draft missed —
`lua.set_named_registry_value(&format!("wc-cmd-{}", id.0), func)`. All three fire before the
preflight that may reject the plugin, so a plugin whose 5th command collides commits zero
commands to the `Registry` yet has already (a) permanently leaked 5 id/label string pairs AND
(b) written 5 `wc-cmd-<id>` Lua-registry values. Effect (b) is worse than a leak: if the failed
plugin's `wc-cmd-<id>` key collides with a **live** command's key (a same-id command that
committed earlier, or survives on reload), the failed plugin's function **overwrites** the live
one — the Registry entry then dispatches to the wrong (failed-plugin) callback. The "inert dead
keys" description in an earlier draft was wrong: a same-id overwrite is live, not inert.

Fix — stage EVERYTHING raw through preflight; intern AND write the callback key ONLY for
committed survivors:

- `PendingReg` becomes `{ name_full: String, label: String, menu: Option<MenuCategory>,
  func: mlua::Function }` — owned/transient strings (cap-checked at `wc.register_command` time
  exactly as today: the borrowed-length-check-then-convert pattern is unchanged; a capped
  `String` is a bounded transient allocation, not a leak) plus the callback carried by value.
  Holding an `mlua::Function` in the sink is sound — it is a GC-rooted handle into the same VM
  the loader owns, dropped when the sink is drained or the plugin fails.
- `install_registration`'s closure does **no** interning and **no** `set_named_registry_value`
  during exec — it only cap-checks, parses `menu`, and pushes a `PendingReg` (raw strings +
  `func`) into the sink. Nothing touches the process-global intern pool or the shared Lua
  registry until preflight passes.
- `load_one`'s preflight checks collisions on the raw strings (`reg.resolve_name(&p.name_full)`
  takes `&str`, no intern needed; plus the in-batch `HashSet<&str>`). After a clean preflight,
  **commit runs in two phases so that the fallible Lua writes cannot half-mutate the Registry**
  (Codex round-2 CRITICAL — `set_named_registry_value` returns `mlua::Result` and CAN fail at
  commit time: a VM registry/memory error, e.g. the `set_memory_limit` 64 MiB cap tripping
  mid-commit):
  - **Fallible phase — every Lua-side + intern write first, Registry untouched.** For ALL of the
    plugin's survivors (commands AND hooks), do every fallible write and collect the results into
    a local staging `Vec`: `intern(&p.name_full)` → `CommandId`, `intern(&p.label)`,
    `lua.set_named_registry_value("wc-cmd-<id>", p.func)?` for each command, and
    `lua.set_named_registry_value("wc-ev-<stem>-<i>", hook.func)?` for each hook (§3b). If ANY
    `?` fails here, the plugin's commit aborts having made **zero `register_plugin` calls** — the
    `Registry` is byte-identical to before this plugin (the surviving-command list and the hook
    table for THIS plugin were never applied). Handling of that abort is the fatal path below.
  - **Infallible phase — Rust-side Registry mutation only.** Only once every fallible write
    above has succeeded: run the `reg.register_plugin(id, label, menu)` calls (they return
    `Result<(), RegisterError>` whose sole `Duplicate` variant preflight already ruled out, so
    each is an `.expect("preflight ruled out Duplicate")` — no *new* failure mode) and append the
    plugin's `HookEntry`s to the host table. No Lua, no allocation that can fail. The key bytes
    equal `name_full` (intern is identity-preserving), so the pump's
    `format!("wc-cmd-{}", call.id.0)` lookup is unchanged.
- **A commit-time `mlua` failure is a fatal VM-health event, not a plugin-authoring error.** It
  means the VM is exhausted (memory/registry) — no plugin author can "fix their script" past it.
  It aborts the ENTIRE `load_phase`, and — because plugins in a `load_phase` commit
  **sequentially**, so an EARLIER plugin A may already have run its infallible phase (Registry
  entries + Lua keys) before plugin B hits a commit-time `mlua` error — the fatal path must
  revert the **whole plugin subsystem** to the builtins-only baseline, NOT just the VM. The host
  does not own Registry state, so nulling the VM alone would leave A's commands committed in the
  `Registry` while their VM is gone — they would dispatch into a torn-down VM (the Codex round-3
  Critical). Concretely, on ANY commit-time `mlua` failure `load_phase`/`perform_reload` performs
  ALL of:
  1. **Null the host** (`*host = PluginHost::null()`) — drops the whole `mlua::Lua` and with it
     every `wc-cmd-*`/`wc-ev-*` key and closure written so far.
  2. **`reg.retain_builtins()`** — reset the Registry to builtins-only, discarding EVERY plugin's
     entries committed so far this phase, **including earlier fully-committed plugins** (A above),
     not merely the failing one.
  3. **Clear `editor.pending_plugin_calls` AND `pending_plugin_events` AND
     `pending_plugin_dispatch`** (one short borrow) — any queued `PluginCall`/event/dispatch now
     references a dead VM and a gone command set.
  4. **`editor.keymap_rebuild = true`** — so any keymap patch bound to a now-removed plugin
     command re-resolves away via the existing `rebuild_keymap_if_requested` arm (contract law 7).
  5. **Report** a load warning (`"plugins disabled: VM exhausted during load"`) and mark the
     `plugin_inventory` accordingly.
  Interned strings from the torn-down subsystem are bounded (per-plugin caps) and moot once the
  VM is gone. Rationale: VM exhaustion is degenerate and rare; a coherent builtins-only editor
  that reports why beats a half-populated command surface pointing at a dead VM, and a later
  `plugins_reload` retries from a fresh VM (§6d step 6). At startup the run-loop's normal
  `build_keymap` still runs after `load_phase`, so step 4's flag simply makes reload's path
  uniform with startup's.
- The stem intern likewise moves into the fallible commit phase — interning it before exec (as
  today) leaks once per failed load of a never-successful plugin. This requires one concrete
  change to `install_registration` (Codex round-2 IMPORTANT 1): its `stem: &'static str`
  parameter — captured by the `move |lua, spec|` `create_function` closure — becomes an **owned
  `stem: String`** moved into the closure (an owned `String` is `'static`, so the closure
  compiles WITHOUT the `send` feature, exactly as today's `&'static str` did). The closure builds
  `format!("{stem}.{name}")` from the owned `String` and pushes it raw into the sink; `set_name`
  and the preflight collision check both accept `&str`, so nothing before the commit phase needs
  `&'static`.

> **Atomicity guarantee (precise).** On any **load-validation** failure of a plugin — a
> cap exceeded, a duplicate id, a preflight rejection, a parse/exec error, a load-budget trip
> (§7a) — **zero** state is committed for that plugin: no intern, no Lua key
> (`wc-cmd-*` or `wc-ev-*`), no `Registry` entry, no `HookEntry`; the batch continues with the
> other plugins (P1 per-plugin isolation). A **commit-time VM-exhaustion** failure (a fallible
> Lua write returning `Err` mid-commit) is not a per-plugin skip — it **reverts the ENTIRE plugin
> subsystem to the builtins-only baseline** for that startup/reload: the VM is nulled, the
> Registry's plugin entries are removed (`retain_builtins` — including earlier successfully
> committed plugins), the plugin queues are cleared, and the keymap re-resolves. No
> partially-registered plugin is ever left live; there is no reachable state in which a plugin's
> Registry entry points at a missing/foreign Lua key OR at a torn-down VM.

- Guardrail tests (§12): a **load-validation** failure (over-cap / duplicate id / preflight
  rejection) leaves the intern-pool size unchanged AND writes no `wc-cmd-<id>` key — a same-id
  live command's stored callback is byte-identical before and after the failed plugin (the
  overwrite regression guard). The commit-time-exhaustion path is asserted at the `load_phase`
  level with TWO plugins: plugin A commits fully, then plugin B's fallible commit phase is forced
  to fail (a test seam forcing a `set_named_registry_value` `Err`); the assertion is that **A's
  command is GONE** (not merely that B didn't commit) — the host is null, the registry is
  builtins-only, and the editor is otherwise functional.

### c. Same-stem dedup (grounding gap 4)

`discover` today pushes BOTH `foo.lua` and `foo/init.lua` under the identical stem — two
independent `load_one` execs in one namespace: same-named commands collide confusingly
(a cross-file duplicate reported as if within one plugin), differently-named ones silently
cohabit. **Policy: ambiguous → load NEITHER, report loudly.**

- `discover` detects the stem collision post-scan (candidates are already sorted by stem, so
  equal stems are adjacent) and, instead of pushing either source into `sources`, pushes **one
  `LoadReport` for the ambiguous stem** into `skipped`:
  `"ambiguous plugin 'foo': both foo.lua and foo/init.lua exist — remove one"`. (`skipped`
  carries one report per *plugin outcome* everywhere else in `discover` — an oversize file, a
  bad-UTF-8 source — so the ambiguous stem is likewise a single report keyed on the stem, NOT one
  per colliding file.) Neither `foo.lua` nor `foo/init.lua` is loaded.
- One-line rationale: any silent preference (file-over-dir or vice versa) makes the OTHER file a
  no-op the user must debug blind; there is no ecosystem-compat pressure forcing a preference,
  and explicit-beats-implicit is house law. Rejecting is also the only choice that stays correct
  if a future P3 adds more plugin shapes.
- Test: the `discover_reads_single_file_and_dir` family gains the same-stem case — one skipped
  report for the ambiguous stem, that stem absent from `sources`, other plugins unaffected.

---

## 8. New limits (`limits.rs` — the one auditable place)

```rust
/// P2 event/config/dispatch caps (resource-bound LAW; grounding §3.6).
/// Max registered hooks per plugin (each = one VM-registry function + one host-side entry).
pub const PLUGIN_MAX_HOOKS_PER_PLUGIN: usize = 64;
/// Clamp on an event's owned path payload at capture time.
pub const PLUGIN_MAX_EVENT_PAYLOAD: usize = 4096;
/// Max nesting depth converted from [plugins.config.<name>] into a Lua table.
pub const PLUGIN_MAX_CONFIG_DEPTH: usize = 8;
/// Max total nodes (keys + values) converted from one plugin's config table.
pub const PLUGIN_MAX_CONFIG_NODES: usize = 1024;
/// Max byte length of any single config string VALUE or table KEY — the pre-allocation byte
/// bound (resource-bound LAW) that depth+node counts miss: config::load reads the source
/// unbounded, so one giant string/key must be rejected BEFORE lua.create_string allocates it.
pub const PLUGIN_MAX_CONFIG_STR: usize = 64 * 1024;
/// Longest wc.command target accepted (the longest registrable id: stem + '.' + name).
pub const PLUGIN_MAX_COMMAND_REF: usize = PLUGIN_MAX_STEM_LEN + 1 + PLUGIN_MAX_NAME_LEN;
/// Max queued-but-undrained wc.command requests (checked at call time).
pub const PLUGIN_MAX_PENDING_DISPATCH: usize = 64;
/// Max processed units (dispatch/callback/hook) per pump cycle — the cascade count cap.
pub const PLUGIN_PUMP_CHAIN_CAP: usize = 64;
```

Time budgets live beside the guard in `host.rs` (they are durations, not sizes):
`CALLBACK_TIME_BUDGET = 150ms` (renamed `TIME_BUDGET`; the only *preemptive* guard — `set_hook`
aborts a running Lua unit), `LOAD_TIME_BUDGET = 1s` (§7a), `PUMP_CYCLE_TIME_BUDGET = 500ms`
(§5c — a *between-units* budget, checked before dequeuing the next unit; it does not preempt a
running one). Event **names** need no cap constant: `event_from_str`'s
parse-to-enum is the bound (the `menu_from_str` precedent — unknown/oversized input is a typed
error, nothing stored). The `plugin_caps_are_sane` test extends over the new constants.

---

## 9. Command-surface-contract conformance (REQUIRED)

P2 touches commands, the palette, the menu, options, and keybinding hints. Conformance,
law by law:

- **Law 1 (registry = single source of truth).** `plugins_reload` and `plugin_list` are ordinary
  `Registry::builtins()` entries. `wc.command` resolves via `reg.resolve_name` and dispatches via
  `reg.dispatch` — the identical path palette/menu/keys use; there is deliberately no call-time
  name-set snapshot (§5a's rationale — a derived cache of the registry would be a second source).
  Registry mutation happens ONLY at the reload seam, between reduces, mirroring the keymap-swap
  discipline — dispatch is never live against a mutating registry.
- **Law 2 (every user-settable option is a command).** `[plugins].dir`, `disable`, `enabled`,
  and `[plugins.config.<name>]` tables are **config-layer load machinery, not runtime options** —
  the same class as `keymap.patches` and `theme.file`, which likewise have no per-key setter
  command (the P1 §9 posture, extended). **Contract-visible naming note:** because per-plugin
  config is namespaced under `[plugins.config.<name>]` (§4a), the plugin-stem namespace carries
  **no reserved names** — any stem a filesystem permits is a valid plugin name (the flatten
  design would have forbidden `enabled`/`disable`/`dir`; the sub-key design does not). They are not `SettingsSnapshot` fields, so the
  law-2 recurrence-guard test is unaffected. The runtime *verb* over all of them is
  `plugins_reload` — a registered command — so a plugin/user CAN apply a `[plugins]` config edit
  at runtime through the command surface (reload re-reads the section, §6f). If `dir` ever
  becomes live-settable state, it requires the rule-10 parameterized set-command machinery — P3,
  noted, not smuggled.
- **Law 3 (palette exhaustive).** The two new builtins enter through `Registry::builtins()` and
  appear in the palette by derivation (`palette.rs` iterates `reg.commands()`). The
  palette-completeness invariant test re-runs over a post-reload registry (§12) — reload must
  never leave a phantom or missing palette row.
- **Law 4 (menu ⊆ palette).** `plugins_reload` and `plugin_list` are tagged
  `MenuCategory::Settings` (the one per-command judgment: plugin management is a
  browse-for-by-category Settings concern, beside the keymap/theme commands). Both are palette
  entries, so the subset law holds by derivation. No dynamic sections in P2.
- **Law 5 (mouse path).** Falls out of the menu placement.
- **Law 6 (one setter).** Reload's state changes flow through existing shared setters/flags:
  `keymap_rebuild` (the same flag `switch_keymap_preset` uses), `editor.status`, and the
  registry's own `register_plugin`/`retain_builtins`. No bypass mutation path is introduced;
  `wc.command` gives plugins the same verbs users have, which is law 6's purpose.
- **Law 7 (hints track the active keymap).** After a reload sets `keymap_rebuild`, the existing
  `rebuild_keymap_if_requested` arm re-runs `build_keymap` against the post-reload registry —
  hints for surviving plugin commands re-resolve, vanished bindings drop with the existing
  warning. The hints-re-resolution invariant test gains a reload case (§12).
- **Rule 10 (commands = the plugin spine).** `wc.command` is this rule realized: the registry is
  now literally the automation API. Commands stay nullary; the parameterized form remains P3.

Amendment check: **no amendment to the contract is required.** Events, per-plugin config tables,
and `wc.config` are host↔plugin data flow, not command-surface actors; the contract's plugin
clause ("plugins route through the registry spine") is exactly what §5 implements.

---

## 10. Input-validation + resource-bound audit (every new plugin-input site)

**Input-validation LAW status:** P2 adds NO API that accepts a byte offset or range. The
range-taking surface is unchanged (`wc.text`, `wc.replace`, `wc.insert`'s cursor check) and
still funnels through `plugin_check_range`. New inputs are strings/functions/tables — audited
below. The observer-mode check (§3e) is *additive* in front of the edit APIs; it cannot weaken
the range check behind it (it runs before, and rejects strictly more).

| # | Plugin-supplied input | Crosses into | Bound (before the allocation/effect) |
|---|---|---|---|
| 1 | `wc.on` event name | nothing stored | **parse-to-enum** (`event_from_str`) on the borrowed bytes — unknown → typed error; the enum is the bound (menu precedent). |
| 2 | `wc.on` callback + count | VM named-registry function + host `HookEntry{key,label}` (owned `String`s) | `PLUGIN_MAX_HOOKS_PER_PLUGIN` per plugin; key/label are `O(stem)` small, **never interned** (die at reload — §3b); Lua side under the VM heap cap. |
| 3 | `wc.command` target name | transient owned `String` in `pending_plugin_dispatch` | length cap `PLUGIN_MAX_COMMAND_REF` on the borrowed bytes; queue cap `PLUGIN_MAX_PENDING_DISPATCH` at call time; drained/cleared every cycle. |
| 4 | `wc.command`-induced work (cascades) | main-thread time | cascade LENGTH + total elapsed bounded between units by `PLUGIN_PUMP_CHAIN_CAP` (count) + `PUMP_CYCLE_TIME_BUDGET` (wall); a single Lua unit preempted by `CALLBACK_TIME_BUDGET` (`set_hook`); a single dispatched builtin NOT preempted — bounded by builtins being hot-path-safe by construction (§5c sharp edge); on trip → queues cleared + status. |
| 5 | `[plugins.config.<name>]` table | one `mlua::Table` per plugin at load | `PLUGIN_MAX_CONFIG_DEPTH` (nesting) + `PLUGIN_MAX_CONFIG_NODES` (count) + `PLUGIN_MAX_CONFIG_STR` (per-string-value AND per-key byte length, checked BEFORE `lua.create_string`) during conversion; over-any-cap → `wc.config = nil` + warning, plugin's commands/hooks still load. The byte cap is REQUIRED, not backstopped by the VM heap: `config::load` reads the source unbounded (config-class), so one giant string/key would otherwise force a large Lua alloc — the LAW requires the pre-allocation check (grounding §3.6). |
| 6 | Event payload (path) | owned `String` per queued event | host-originated, clamped at capture to `PLUGIN_MAX_EVENT_PAYLOAD` via `cap_status` (char-boundary safe); queue drained or cleared every cycle, incl. the null host (§3d). |
| 7 | registration ids/labels + callback keys (revisited) | the permanent intern pool + the shared Lua registry | unchanged caps (stem/name/label/count) + **two-phase commit** (§7b): the fallible Lua writes (intern + every `wc-cmd-*`/`wc-ev-*` `set_named_registry_value`) run FIRST for all of a plugin's survivors; the infallible `register_plugin` calls run only if every Lua write succeeded — so a **load-validation** failure leaks zero AND overwrites no live callback key, and a **commit-time VM-exhaustion** failure reverts the ENTIRE plugin subsystem to builtins-only (null host + `retain_builtins` + clear queues + `keymap_rebuild`), so no earlier-committed plugin is left pointing at a dead VM. Repeated reload of unchanged plugins leaks zero (intern de-dup); renamed ids leak once each, cap-bounded. |
| 8 | plugin source at load | one `exec()` on the main thread | `PLUGIN_MAX_SOURCE_BYTES` (existing read bound) + `LOAD_TIME_BUDGET` (§7a — the previously-missing time bound) + `panicx::catch` (existing) + VM heap cap (existing, covers load-phase allocation). |
| 9 | hook execution (observer surface) | editor state | mechanically read-only: edits/`wc.command` rejected via `InvokeState.observer` (§3e), reset by RAII on unwind; status writes share the existing `PLUGIN_MAX_STATUS_LEN` clamp. |
| 10 | `wc.command`-opened overlay (`palette`/`menu`/`theme`/`open`) | a placeholder overlay needing hydration | the pump's `reg.dispatch` is a new dispatch path with no per-site hydrate, so it would open UN-hydrated; resolved by the uniform post-pump `hydrate_overlays` run-loop call (§5b) — idempotent + self-guarding, so no correctness/geometry gap and no double-build. Not a resource cap — a UI-correctness path, listed because it is plugin-reachable. |

Error routing for every row: typed `mlua::Error` → `pcall`-able by the plugin; uncaught →
`normalize` → `plugin_error` → status line. Never a panic, never a console write, never an
aborted user operation.

---

## 11. Anti-regrowth / module structure

- **`app.rs` (886/1000 — the budget most at risk; ~114 lines of headroom).** Net `app.rs`
  growth is a **TARGET of ≤ 0 lines, ENFORCED by the `module_budgets` gate** — not a line count
  asserted here (the exact delta is unverified until the plan writes the code, and the reload
  work — config re-read, inventory, queue clearing, host replacement, load summaries, keymap
  re-resolution — is real, so it must NOT land inline). The mechanism that pays for it, in
  priority order: (1) extract the inline startup plugin-load match arm (`PluginHost::new` /
  `discover` / `load_sources`, between `Registry::builtins()` and the freeze) OUT into the shared
  `plugin/reload.rs::load_phase` — this alone removes more lines than P2 adds; (2) keep the
  reload seam in `app.rs` a guarded CALL only, with the whole body in
  `plugin/reload.rs::perform_reload`; (3) the freeze line is deleted, the pump call gains
  arguments on its existing line, fire sites live in `save.rs`/`workspace.rs`/`session_restore.rs`.
  The one net-additive `app.rs` line P2 genuinely adds is the post-pump `hydrate_overlays` call
  (§5b — it reuses the existing `app::hydrate_overlays` fn, no new logic in `app.rs`); it is
  covered many times over by (1)'s extraction.
  If (1)+(2) do not fully pay for the seam, the plan moves more of the seam's glue into
  `plugin/reload.rs` until the gate passes — bumping the budget is explicitly NOT the escape.
  Hooks and reload logic enter through seams (`fire_event`, `perform_reload`, the pump); `reduce`
  and `run` gain no new inline bodies. The 114-line headroom is comfort margin, not the plan.
- **`plugin/host.rs` (203/400).** Grows: pump re-drain + caps, hook table, `InvokeState`,
  budget-parameterized guard. Estimated ~330 — within budget; if the plan's reality exceeds it,
  extract `plugin/events.rs` (the hook table + event invocation) rather than bumping the budget
  — the budget test's own instruction.
- **New modules, one axis each:** `plugin/settings.rs` (TOML→Lua hand-off),
  `plugin/reload.rs` (load-phase orchestration + reload sequence). `plugin/api.rs` grows by
  three installers on the existing flat seam (`install_on`, `install_command`, the observer
  checks) — adding API areas never edits a dispatcher.
- **`registry.rs` stays Lua-free** — `retain_builtins` and two builtin rows; no `mlua` import,
  no new dispatcher.
- **`clippy::too_many_lines` (100)** applies to every new function; the pump's re-drain loop
  stays a thin loop over three delegate methods (`drain_dispatches`, `invoke_call`,
  `invoke_hooks`), not an inline body.

---

## 12. Testing & success criteria

The P1 test architecture carries over: the pump is the `InlineExecutor` of plugins — build a
host from string sources, enqueue, pump against a real `Rc<RefCell<Editor>>`, assert. New suites:

- **Events:** a registered `on_save`/`on_open`/`on_buffer_close` hook fires with the right
  `{kind, path}` payload when the corresponding `fire_event` is enqueued + pumped; multiple
  hooks fire in registration order; a hook error is isolated (status set, other hooks still
  run, editor intact); a hook exceeding `CALLBACK_TIME_BUDGET` is aborted (low test budget);
  an event with no registered hooks is dropped for free; the **null host clears** the event
  queue (no unbounded growth under `--no-plugins`).
- **Fire sites (integration, e2e.rs journeys):** a real save (InlineExecutor job + merge) fires
  exactly one `Save` event on `Ok` and none on `Err`; file-browser open fires `Open` once for
  both the new-buffer and throwaway-reuse shapes (no double-fire); each of the three close
  shapes (clean, Discard, post-save close) fires `BufferClose` with the pre-removal path; quit
  fires nothing.
- **Observer-only:** each of `wc.insert`/`wc.replace`/`wc.set_selection`/`wc.command` from a
  hook → typed error, buffer/selection/queues unchanged; the SAME calls from a command callback
  still succeed (the flag resets — including after a panicking hook, the RAII case).
- **`wc.command`:** dispatches a builtin (observable effect, e.g. a toggle) and a plugin command
  (chained callback runs, same frame); unknown name → status error naming the origin plugin;
  over-length name and a full dispatch queue → typed errors, nothing queued; the ping-pong
  cascade (two plugin commands dispatching each other) terminates at `PLUGIN_PUMP_CHAIN_CAP`
  with queues cleared + status; a cycle exceeding a low test `PUMP_CYCLE_TIME_BUDGET` is
  truncated.
- **`wc.command` overlay hydration (§5b regression):** a plugin command that calls
  `wc.command("palette")` (and, separately, `wc.command("menu")`) — driven through a real pump +
  the post-pump `hydrate_overlays` step — leaves the overlay **HYDRATED**, not empty: assert
  `editor.palette`'s `rows` are populated (all registry commands) / the `menu` is `built` with
  groups. Guards against the regression where the pump's `reg.dispatch` opened an overlay no
  per-path hydrate reached.
- **Config:** `[plugins.config.<name>]` reaches `wc.config` with correct types (string/int/bool/
  array/nested table); absent section → nil; per-layer REPLACE fold; a plugin named
  `enabled`/`disable`/`dir` gets its config correctly (no collision — the namespacing regression
  guard); over-depth, over-node, AND over-`PLUGIN_MAX_CONFIG_STR` (a config value string past the
  byte cap, and separately an over-cap table KEY) → plugin still loads with `wc.config` nil +
  warning, and its commands/hooks are unaffected; `wc.config`
  is nil at callback time (cleared at attach) while a load-time captured local still works.
- **Reload:** `retain_builtins` unit tests (§6b); full `perform_reload` — a changed plugin
  source is live post-reload, removed plugin's command/palette entry/binding gone, added
  plugin's present; keymap patch re-resolution both ways; stale-queue clearing; reload with
  `cli.no_plugins`/`enabled=false` → null host + builtins-only registry + working editor;
  reload retrying a failed VM; `plugin_list` formats the inventory; intern-pool size stable
  across N reloads of the same plugin (the §7b guardrail, extended: reload leaks zero for an
  unchanged plugin set).
- **Hardening:** load-budget trip → `LoadReport` error, batch continues, later callbacks
  unaffected (a leaked hook would fire on the next Lua call — the `HookGuard` test); a
  load-validation failure (over-cap / duplicate / preflight rejection) leaves the intern pool
  size unchanged AND writes no `wc-cmd-<id>` key (the same-id-overwrite regression guard — a live
  command's stored callback is byte-identical before and after a failed plugin that re-declares
  its id, §7b); a **commit-time VM-exhaustion with TWO plugins** — A commits fully, then B's
  fallible commit phase is forced to `Err` (test seam on `set_named_registry_value`) — asserts
  **A's command is GONE** (whole-subsystem revert: null host + `retain_builtins` + cleared queues
  + `keymap_rebuild`), no plugin command resolves, editor otherwise functional (§7b); same-stem
  pair → one skipped report for the ambiguous stem, named.
- **Contract invariants (merge GATEs):** palette-completeness + menu-subset re-run over a
  post-reload registry; hints-re-resolution across a reload; the every-persisted-setting guard
  (unchanged — P2 adds no `SettingsSnapshot` field).
- **Resource guardrails:** the P1 loaded-but-idle test extends — a plugin with hooks loaded,
  idle `Msg::Tick`s driven: zero hook invocations, zero queue growth, `timers::next_wake`
  unchanged (events are edge-triggered by ops, never by time).
- **Success criterion:** the §1 `wordcount.lua` demo end-to-end, including a live
  `plugins_reload` of an edited script.

---

## 13. ⚠ OPEN — HUMAN DECISION flags (both product-visible, neither determined by §2)

1. **Does `on_open` fire for the startup file?** The CLI-arg buffer is constructed
   (`Buffer::from_file` in `app::run`) long before the plugin load phase and the bridge exist;
   session-resume restores cursor/scroll into that same buffer (no reopen). So without deliberate
   synthesis, `on_open` fires ONLY for in-session opens (today: the file browser). Options:
   **(A — recommended)** do not synthesize in P2: `on_open` means "a file was opened during the
   session"; adding a startup event later is additive/compatible, removing one would break
   plugins — start narrow. **(B)** after `attach_bridge`, synthesize one `Open` event for the
   startup buffer if it has a path (Neovim's `BufReadPost`-at-startup precedent; plugins see
   every buffer exactly once). The spec is written to (A); flipping to (B) is one guarded
   `fire_event` call after the bridge attaches.
2. **`plugins_reload` re-reads the `[plugins]` config section** (§6f). Options:
   **(A — recommended, spec'd)** re-read `[plugins]` (only) at each reload: makes
   config-driven-plugin iteration (`[plugins.config.<name>]` edits, disable-list changes, even
   `enabled = false→true`) work without restarting — the main audience of reload is exactly the
   person editing these files; blast radius is one subsystem. **(B)** use the startup snapshot:
   perfectly uniform "config is read once" semantics, but plugin config iteration then requires
   a full restart, and `plugin_list` can show a plugin the user just disabled. The asymmetry in
   (A) — `[plugins]` fresh, all other sections stale — is the cost being flagged.

---

## 14. Grounding gap map (§4 of the grounding → this spec)

| Gap | Resolution |
|---|---|
| 1. Load-phase runaway | §7a — `LOAD_TIME_BUDGET` (1 s) via the budget-parameterized `with_time_guard` around `exec()`. |
| 2. Intern-on-commit leak (+ callback-key overwrite) | §7b — raw-`String`+`func` `PendingReg`; two-phase commit (all fallible Lua/intern writes first, infallible `register_plugin` second); a load-validation failure leaks zero + overwrites no live key; a commit-time VM-exhaustion failure reverts the whole subsystem to builtins-only (null host + `retain_builtins` + clear queues + `keymap_rebuild`). |
| 3. Registry mutation / reload | §6 — `retain_builtins` + unfrozen loop-local `reg` + the between-reduces reload seam + queue clears + `keymap_rebuild`. |
| 4. Same-stem dedup | §7c — ambiguous stem loads neither, named report, test added. |
| 5. Event sites hold a borrow | §3c/§3d — `fire_event` enqueues only; Lua runs solely in the pump with no outer borrow. |
| 6. No `[plugins].dir` | §4d — the config key + verbatim-path semantics + the no-dir warning (silence removed). |
| 7. Pump chain cap | §5c — unified re-drain loop; `PLUGIN_PUMP_CHAIN_CAP` + `PUMP_CYCLE_TIME_BUDGET`; clear-and-report on trip. Plus §5b — post-pump `hydrate_overlays` so a `wc.command`-opened overlay is hydrated like key/menu/mouse opens. |

---

## 15. Task decomposition sketch (ordering for the plan to refine)

1. **Guard refactor + load budget** (gap 1): budget param on `with_time_guard`, rename to
   `CALLBACK_TIME_BUDGET`, add `LOAD_TIME_BUDGET`, wrap `exec()`, tests. Small, independent.
2. **Atomic-on-commit registration** (gap 2): `PendingReg` → raw `String`s + carried
   `mlua::Function`; `install_registration`'s closure captures an OWNED `stem: String` (not
   `&'static str`) and does no intern / no `set_named_registry_value` during exec; the commit
   splits into a fallible phase (all intern + `wc-cmd-*`/`wc-ev-*` writes for the plugin's
   survivors) then an infallible phase (`register_plugin` + `HookEntry` append); a commit-time
   `mlua` `Err` reverts the whole subsystem (null host + `reg.retain_builtins()` + clear the three
   plugin queues + `keymap_rebuild = true`). Tests: intern-pool + no-callback-key-overwrite on
   load-validation failure; the TWO-plugin exhaustion test (A committed, B forced to `Err` → A's
   command GONE, builtins-only registry). Note: this task's fatal path calls `retain_builtins`, so
   it lightly depends on task 7's `Registry::retain_builtins` landing first (or co-landing).
3. **Same-stem dedup** (gap 4): `discover` change + test. Independent.
4. **Config plumbing** (gap 6 + §4): `RawPlugins.dir` + namespaced `config` (§4a) + fold +
   warnings; `plugin/settings.rs` conversion under the new caps; `wc.config` install in
   `load_one` + clear at attach; `load_sources`/`load_one` signature growth (`&mut PluginHost`,
   the per-plugin `config` value).
5. **Events core** (gap 5): `PluginEvent`/kinds/`event_from_str`, `Editor.pending_plugin_events`,
   `wc.on` + hook sink + atomic commit + host hook table, `InvokeState` + observer checks,
   `fire_event` + the three fire sites, event drain in the pump (still single-pass at this
   task), the closed post-load `wc.on` stub. The largest single task; may split
   registration/firing vs. draining.
6. **`wc.command` + pump context + caps** (gap 7): `pending_plugin_dispatch`, the installer,
   pump signature growth (`reg`/`executor`/`clock`/`msg_tx`), the unified re-drain loop,
   `PLUGIN_PUMP_CHAIN_CAP` + `PUMP_CYCLE_TIME_BUDGET`, null-host queue clearing, the **post-pump
   `hydrate_overlays` run-loop call** (§5b — a `wc.command`-opened overlay must hydrate) + its
   regression test, cascade tests.
7. **Reload** (gap 3): `retain_builtins`; extract `load_phase`; `perform_reload`; unfreeze
   `reg`; the run-loop seam; `plugins_reload`/`plugin_list` builtins + `plugin_inventory`;
   config re-read; interaction-matrix tests. Last because it exercises everything above.
8. **Gates**: contract-invariant re-runs over post-reload registries, the extended idle
   guardrail, e2e journeys, module budgets, the success demo.

Dependency notes for the plan: 4 before 5 (`load_one`'s signature changes once); 5 before 6
(the re-drain loop caps both queues); 6 before 7 (reload's "pump quiesced before the seam"
argument assumes the re-drain loop exists). **Task 2's fatal path calls `Registry::retain_builtins`
(task 7's method), so land `retain_builtins` with — or before — task 2's fatal-path test** (the
method is small and Lua-free; pulling it earlier, or co-landing the two, keeps task 2 self-testing).
Tasks 1 and 3 are order-free and can lead.
