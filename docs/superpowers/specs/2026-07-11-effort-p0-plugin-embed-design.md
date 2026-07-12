# Effort P0 — embed Lua + open the command seam (design)

**Status:** SPEC (2026-07-11). First phase of **Effort P** (the in-process Lua plugin system, the 1.0
capstone). Brainstormed + approved 2026-07-11. Grounds on the design-space doc
`docs/design/effort-p-plugin-system-design-space.md` and the real-code surface map (this session).

**P decomposition (agreed):** P is sliced into four sequential sub-efforts, each its own
spec→plan→build cycle: **P0** (this — embed + command seam) → **P1** (read API + hook/event seam) →
**P2** (validated edit API) → **P3** (trust/limits/loading conventions/distribution). Fault lines follow
"what's already built vs. what's new": the validated write boundary (`submit_transaction`) and panic
isolation (`panicx`) already exist; the command registry is closed and there is no hook seam yet.

---

## 1. Goal & scope

**Goal.** Embed a Lua VM in the `wordcartel` shell and open the command registry just far enough that a
user-installed single-file Lua plugin can register a command that appears in the palette and runs its
Lua callback with mediated, scoped access to the editor — panic-isolated, error-surfaced, and
conformant with the command-surface contract.

**In scope:**
- Embed **PUC Lua 5.4** via `mlua` (shell crate only; `wordcartel-core` stays Lua-free).
- A `PluginHost` (run-loop sibling of `Editor`) owning the VM + loaded-plugin records + callback refs.
- A startup loader that eager-loads `*.lua` files from a config'd dir.
- Open the registry: plugins register **additive, auto-namespaced, palette-only** commands.
- Minimal `wc.*`: `wc.register_command(name, label, fn)` and `wc.status(msg)`.
- Panic isolation on every Lua entry; typed `PluginError` surfaced to `editor.status`.
- `[plugins]` config (`dir`, `enabled`) + `--no-plugins` flag.

**Explicitly NOT in P0 (later phases):** reading buffer/selection/config, editing, event/hooks, keymap
registration for plugin commands (**P1**); the validated edit API wrapping `submit_transaction`
(**P2**); sandboxing/capability limits, resource caps (CPU/mem/output), multi-file plugins, lazy
`require`, manifests, plugin distribution, override of built-in commands (**P3**).

## 2. Forks resolved (from the brainstorm)

1. **VM = PUC Lua 5.4** (via `mlua`, `vendored`). Standard, current, portable; JIT irrelevant for
   event-driven plugins; standard dialect is what authors expect. (LuaJIT rejected — Lua 5.1, raw-FFI
   liability; Luau deferred — its VM sandbox is redundant with our mediated-API boundary for trusted
   plugins, revisit only if an untrusted marketplace ever lands.)
2. **Editor access = `mlua` scoped lending.** VM lives outside `Editor`; at dispatch we enter
   `lua.scope(|s| …)`, expose editor-backed `wc.*` scoped functions that borrow `&mut Editor` only for
   that call, then invoke the stored callback. Statically-checked borrow, no `RefCell`, no `'static`
   gymnastics. This is the "mediated access, not raw internals" load-bearing decision made concrete.
3. **Command registration = additive, auto-namespaced, override-free.** A plugin's `register_command
   ("reflow", …)` becomes id `pluginname.reflow` — collision-free with built-ins and other plugins by
   construction; a plugin can never replace a built-in. (Explicit `override_command` is a possible P3+
   opt-in, not built now.)
4. **On-disk shape = one `.lua` file per plugin**, eager-loaded. (Directories/manifests/lazy-`require`
   are P3; they layer on additively and won't break single-file plugins.)
5. **Lazy loading = not built now; lazy *execution* is already inherent** (registration eager,
   callback runs only on dispatch). Convention documented: a plugin's top-level script must be cheap
   (registration only), so P3's lazy-`require` slots in without retrofitting plugins.

## 3. Components & architecture

### 3.1 `PluginHost` (new module, e.g. `wordcartel/src/plugin/mod.rs` + submodules)
A run-loop sibling of `Editor` — **not** a field of `Editor` (fork 2: the VM cannot live inside the
value it must lend into a scope, or the borrow self-conflicts). Owns:
- `lua: mlua::Lua` — the VM.
- Loaded-plugin records: `name`, source path, and the Lua callback refs (`mlua::RegistryKey` per
  registered command) so callbacks persist across dispatches.
- The `wc` global table (populated with load-time functions; scoped functions added per-dispatch).

Public surface (sketch — the plan pins exact signatures):
```rust
pub struct PluginHost { /* lua, plugins, callbacks — all private */ }
impl PluginHost {
    pub fn new() -> Self;                     // hermetic; no VM work until load
    pub fn load_dir(&mut self, fs: &dyn Fs, dir: &Path, reg: &mut Registry) -> Vec<PluginError>;
    pub fn run_command(&mut self, key: PluginCallbackId, editor: &mut Editor /*, clock,… */)
        -> Result<(), PluginError>;
    pub fn is_idle_clean(&self) -> bool;      // guardrail: loaded-but-idle does no bg work
}
```
`load_dir` reads via the **`Fs` seam** (M3) so tests inject fixtures without touching the real FS. The
VM's `unsafe` is encapsulated inside `mlua`; the shell's own `#![forbid(unsafe_code)]` holds.

### 3.2 Loader
At startup (after config, before the run loop), if `plugins.enabled` and not `--no-plugins`: scan
`plugins.dir` for `*.lua`, sort **alphabetically (deterministic)**, and for each run its top level once
inside `panicx::catch`. A plugin whose load errors/panics is **skipped**, its error collected and
surfaced to `editor.status`; other plugins and startup proceed. Plugin name = filename stem.

### 3.3 Dispatch integration (the one spot with real churn)
Running a plugin command needs both `&mut Editor` and the host. Therefore **`Ctx` gains a plugin-host
handle**:
```rust
pub struct Ctx<'a> {
    pub editor: &'a mut Editor,
    pub clock: &'a dyn Clock,
    pub executor: &'a dyn Executor,
    pub msg_tx: Sender<Msg>,
    pub plugin_host: &'a mut PluginHost,   // NEW (or a narrower handle)
}
```
`Registry::dispatch` routes a `Handler::Plugin(key)` arm to `ctx.plugin_host.run_command(key,
ctx.editor, …)` (disjoint field borrows of `ctx`). `run_command` enters `lua.scope`, installs the
editor-backed scoped `wc.status`, and calls the stored callback. Native handlers dispatch exactly as
today. Threading the handle through `Ctx` construction sites (`input.rs`, `mouse.rs`, `app.rs::reduce`,
overlay dispatch) is the main mechanical cost; a `NullPluginHost`-style default keeps non-plugin builds
and tests simple.

## 4. The command seam (opening the registry)

Changes in `wordcartel/src/registry.rs`:
- `Handler` widens from `fn(&mut Ctx) -> CommandResult` to an enum:
  ```rust
  pub enum Handler { Native(fn(&mut Ctx) -> CommandResult), Plugin(PluginCallbackId) }
  ```
  Built-ins register `Native(fn)` (still zero-cost fn pointers). `dispatch` matches the enum.
- `CommandId` and `CommandMeta.label` widen from `&'static str` to **`Cow<'static, str>`** so runtime
  plugin strings fit while built-ins keep their `'static` literals (no allocation for built-ins).
- A new **runtime registration path** used only by the host/loader — e.g. `pub(crate) fn
  register_plugin_command(&mut self, id: Cow<'static,str>, label: Cow<'static,str>, key:
  PluginCallbackId) -> Result<(), RegisterError>` — that appends to `entries`/`index`. Built-in
  `Registry::builtins()` is unchanged in structure.
- Plugin commands carry `menu: None` (palette-only). Auto-namespacing (`pluginname.` prefix) is applied
  by the host before calling the registration path, guaranteeing no collision with built-ins (flat ids)
  or other plugins; a within-namespace duplicate is a `RegisterError` surfaced to status.

The registry stays **VM-agnostic** — it holds a `PluginCallbackId` (an opaque handle), never Lua types;
all VM machinery is in the host. Palette/menu derivation (`Registry::commands()`) is unchanged and now
naturally includes plugin commands (single source of truth preserved).

## 5. `wc.*` surface (P0)

- **`wc.register_command(name: string, label: string, fn: function)`** — available during plugin load.
  Because the plugin's script runs *inside* `lua.exec()` (the host's VM is borrowed for the duration),
  this callback cannot mutate the host or registry directly — it uses a **collect-then-apply** pattern:
  the callback captures a shared pending-registration buffer (`Rc<RefCell<Vec<PendingReg>>>`), stashes
  `fn` as a persistent `mlua::RegistryKey` (via `lua.create_registry_value`, which takes `&Lua` — mlua's
  interior mutability makes this legal mid-`exec`), and pushes `PendingReg { name, label, key }`. After
  the script's top level returns, the **loader** drains the buffer, computes `pluginname.name`, stores
  each callback in the host, and calls the registry's runtime registration path. Errors (duplicate
  within namespace, bad args, status-at-load) → `PluginError` surfaced after the drain.
- **`wc.status(msg: string)`** — a **scoped** function present only inside a dispatch call; sets
  `editor.status`. Invoking `wc.status` at load time (outside a scope) is a `PluginError`
  ("status unavailable during load").

`wc` is a single global table. No other surface in P0.

## 6. Config & CLI

New `[plugins]` section on `Config` (`wordcartel/src/config.rs`):
```rust
pub struct PluginsConfig { pub enabled: bool /* default true */, pub dir: PathBuf /* default XDG */ }
```
Default dir: `~/.config/wcartel/plugins/` (XDG config, consistent with existing config-path handling).
CLI: `--no-plugins` forces `enabled = false` for the session. (`config.rs`'s dead
`DiagnosticsConfig.linters: Option<Vec<String>>` is untouched — it belongs to the diagnostics-provider
selector, not this loader.)

## 7. Isolation & error model

- Every Lua entry — load, `register_command`, `run_command` — is wrapped in `panicx::catch` (the
  existing primitive, already named for "plugin call-sites"). A Lua runtime error or a Rust-side panic
  in the FFI path is caught; the editor is left intact.
- New typed `PluginError` enum (e.g. `Load { plugin, msg }`, `Runtime { plugin, msg }`,
  `Register { id, reason }`, `StatusAtLoad`) with a `describe(&self) -> String`, surfaced via the
  existing `editor.status = …` convention. No console/stderr (the app owns the alternate screen;
  `print_*` are deny-lints).
- A plugin error never aborts startup, never crashes the editor, never loses buffer data.

## 8. Testing & success criteria

**Deterministic tests** (no real FS/VM flakiness):
- Load a fixture plugin from an in-memory source via the `Fs` seam; assert the namespaced command
  exists in the registry with the right label and `menu: None`.
- Dispatch the plugin command through the real `Registry::dispatch` + host scope; assert `editor.status`
  changed as the callback intended.
- Load-error fixture (syntax error / top-level `error()`): assert it is skipped, a `PluginError`
  surfaced, and a sibling good plugin still loads.
- Callback-panic/`error()` fixture: assert `panicx` catches it, status shows the error, editor intact.
- `wc.status` at load time → `PluginError::StatusAtLoad`.
- Namespace-collision (a plugin registering the same command twice) → `RegisterError` surfaced.
- **Resource guardrail:** a loaded-but-idle host does no background work (`is_idle_clean`), mirroring
  the swap SSD-wear guardrail style — no threads spawned, no polling, consistent with "idle is free."

**Success (the demo):** a single `hello.lua` in the plugins dir registers `hello`; on startup it loads;
`hello.hello` appears in the palette with provenance; invoking it runs the callback → `wc.status("hello
from plugin")` shows on the status line. `--no-plugins` suppresses all of it.

**Gates:** `cargo test` green; `cargo build`/`--no-run` warning-free for touched crates; workspace
clippy clean; module budgets hold (`app.rs` ≤ 1000). PTY smoke run + quoted (advisory). `cargo deny`
re-run at release (new `mlua` + Lua deps recorded).

## 9. Command-surface-contract conformance

**Conforms.** Plugin commands are real `Registry` commands, so: **palette-complete** (they appear in the
palette), **menu ⊆ palette** (they are palette-only, `menu: None`), and the **registry stays the single
source of truth** (palette/menu derive from it unchanged). P0 adds **no user-settable option**, so
"every option has a command" and the one-shared-setter law are unaffected. **Keybindings for plugin
commands are P1** (this spec does not add hint re-resolution surface). No contract amendment is needed.

## 10. Anti-regrowth / module structure

`PluginHost` + loader are **new modules** — no bulk added to `reduce`/`run`. The dispatch change is a
thin delegation arm (`Handler::Plugin(k) => ctx.plugin_host.run_command(k, …)`), a row-not-body edit.
`app.rs` gains only run-loop wiring (construct the host, thread it into `Ctx`) and stays under its
1000-line budget. The registry type-widening (`Handler` enum, `Cow` ids) is mechanical, not new
dispatch bulk. This is Open–Closed: the hub gains a delegation seam, the plugin machinery lives in its
own module.

## 11. Risks / notes

- **`Ctx` churn** is the main integration cost — every `Ctx` construction site must supply the host
  handle. A default/null host keeps tests and any no-plugin path simple.
- **`mlua` dependency weight** (H2 lens): PUC 5.4 `vendored` is a small C build; record it in `cargo
  deny`. This is the first native VM dep — deliberate and scoped to the shell.
- **Scoped-function setup cost** per dispatch is negligible for command dispatch (a user action);
  P1 will decide hot-path hook execution separately.
- **FFI error propagation** (Lua `error`/`longjmp` vs Rust unwinding) is `mlua`-managed but warrants a
  fault-injection test (M3-style) — included in §8.
