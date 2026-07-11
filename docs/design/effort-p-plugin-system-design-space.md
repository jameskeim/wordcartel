# Effort P — in-process Lua plugin system: design space (pre-spec exploration)

**Status:** DESIGN-SPACE / pre-spec (2026-07-08). NOT law, NOT an approved spec — this grounds the
eventual Effort-P brainstorm; every fork below is an open decision for that brainstorm, not a
commitment. Effort P is the 1.0 capstone (see `CLAUDE.md`; memory `wordcartel-plugin-roadmap`).

**Provenance:** the project has been provisioned toward this — the **command registry** was built as
the "plugin/automation spine" (`docs/design/command-surface-contract.md`) and the **`std::thread` +
`mpsc` job substrate** as the off-hot-path execution seam. This note captures a Neovim comparison and
the resulting design space.

---

## Reference model — how Neovim does it
Neovim embeds a LuaJIT interpreter directly in its C core (no RPC), giving Lua synchronous access to
editor state. Three mechanisms: (1) **runtimepath** directory scan; (2) **eager `plugin/`** sourcing
(tiny files that register commands/keymaps) vs **lazy `lua/`** loading (a custom `package.searchers`
entry resolves `require('x')` on demand); (3) a global **`vim`** bridge — `vim.api` (direct C
buffer/window manipulation), `vim.fn` (Vimscript functions), `vim.cmd` (Ex commands).

**Feasibility in Rust:** all of this ports. `mlua` embeds Lua (LuaJIT / PUC 5.1–5.4 / Luau) in-process
with synchronous Rust↔Lua calls and lets you inject a custom `package.searchers` entry (the lazy-require
mechanism, identical technique). Nothing here is blocked by Rust.

## Reference model 2 — how Fresh does it (the sandboxed-JS cousin)
Fresh (`github.com/sinelaw/fresh`) is a mature Rust/ratatui terminal editor that made the OPPOSITE
runtime choice: plugins are **sandboxed TypeScript in a QuickJS VM (`rquickjs`) on a dedicated OS
thread**, talking to the editor only by async message-passing. Read at source 2026-07-11. It is the
empirical counter-model to Neovim's (and our) in-process approach — most of its design traits are
downstream of that one boundary, which is exactly why it's instructive.

**What the thread boundary buys Fresh — and why we don't inherit it (fork 3).** Because plugin code
runs off the edit thread, a slow or infinite-looping plugin *cannot* stall typing; Fresh exploits this
fully and ships with **zero execution bounds** (no timeout, no gas, no interrupt — "a plugin can
infinite-loop," `docs/internal/plugins.md`). **Our in-process Lua inverts this risk:** a slow Lua hook
on the edit thread WOULD freeze typing, so we cannot copy their no-bounds posture — we need either
job-substrate dispatch or a real preemption budget (an `mlua` debug-hook instruction count). Fresh
confirms the risk is real by structurally avoiding it the *other* way.

**The cost the boundary imposes — the trap NOT to copy (fork 4 + the module-structure GATE).** Because
plugin→host calls must be serializable across the thread, every capability is a wire-enum variant:
Fresh has a **~205-variant `PluginCommand` enum funneled through one `handle_plugin_command` match** — a
textbook dispatch-attractor god-object. In-process Lua avoids this for free: a Lua plugin calls a
registered Rust host fn directly (`mlua`), so a new capability is a **new entry in a `name → fn`
registration table**, not a new enum arm. *Explicit warning:* do NOT model our plugin→host calls as a
`PluginCommand`-style enum "for safety" — that recreates Fresh's god-hub and fails our own
anti-regrowth GATE.

**Transferable regardless of runtime (steal list):**
- **The "Provider law"** — the plugin supplies *data*; the host owns UI, layout, navigation, focus,
  hit-testing. Fresh's own retro: plugins that drew their own UI reimplemented navigation and produced
  keybinding/i18n bugs. Adopt the *law* (not necessarily their heavyweight virtual-DOM widget runtime).
  Reinforces fork 4 and the command-surface stance.
- **Generated typed API from the Rust impl** — a macro derives the plugin-facing contract from the
  single Rust source so it can't drift. For us: generate LuaLS `---@meta` annotations from the `wc.*`
  API definition the same way (authors get completion; the contract can't drift). Ties to fork 4 +
  command-surface "single source of truth."
- **Per-plugin side-effect tracking → compensating teardown on unload** — Fresh records every overlay/
  decoration/resource a plugin created so unload reverses it exactly. The no-leak discipline our
  "free at rest" / no-data-loss invariants want (forks 6–7).
- **A level-triggered feedback-loop bug worth pre-empting** — a plugin re-publishing an unchanged
  status every render frame created a render→hook→ack→render loop at ~13 Hz; Fresh fixed it by
  classifying outputs visual/non-visual and suppressing no-op re-publishes. Validates our
  edge-triggered-not-level-triggered rule; pre-empt directly if hooks fire on render/idle (forks 3, 7).

**Crash-isolation caution (fork 6).** Fresh catches JS exceptions, but a **Rust panic in a plugin FFI
callback re-panics onto the main thread and kills the editor.** Our M4 `catch_unwind` reuse must wrap
every host callback invoked *from* Lua, not just the plugin's own body — a plugin-triggered *host* bug
must not take down the word processor.

**Distribution data point (deferred non-goal).** Fresh ships plugins as git repos with a `pkg.ts`
package-manager *plugin* + an install-time code-review confirmation dialog — a reference for our
deferred distribution story, not adopted now.

---

## The load-bearing decision: adopt the mechanics, NOT "raw access to internals"
Neovim's `vim.api` hands Lua **first-class synchronous access to internal memory structures**. WordCartel
**cannot and should not** inherit that part — for two reinforcing reasons:
- **Philosophy:** the codebase is valid-by-construction (private fields; validated constructors
  `ChangeSet`/`Selection`; the hardened untrusted-edit boundary `submit_transaction`). Raw plugin
  mutation of buffers/block-tree/selection would defeat the no-data-loss / no-invalid-state guarantees.
- **Rust mechanics:** you can't easily fling `&mut Editor` into a Lua callback (lifetimes/borrow
  checker); the language pushes you to expose a controlled API object (`mlua` `UserData` with methods).

**Direction:** adopt Neovim's **lifecycle + loading mechanics**, but replace raw-internals access with
**mediated access through the command registry + the validated transaction boundary**. Plugin edits
route through the same validated path as any untrusted input.

### What maps cleanly
| Neovim | WordCartel equivalent | Blocker? |
|---|---|---|
| `runtimepath` scan | configured plugin-dir scan | No — convention |
| eager `plugin/` sourcing | plugin registers **commands** into the registry at load | No |
| lazy `lua/` via `package.searchers` | custom `mlua` searcher — same technique | No |
| `vim.api`/`vim.fn`/`vim.cmd` | a `wc.*` namespace over the registry + a validated buffer/selection API | No — scoped, not raw |

---

## Open forks (each is a brainstorm decision)

**1. Which Lua VM.** LuaJIT (fast, but Lua 5.1, asm-heavy, patchier platform support, less maintained)
vs **PUC Lua 5.4** (portable, maintained, no JIT) vs **Luau** (Roblox's Lua with built-in sandboxing +
capability limits + types). All three are `mlua` backends. The choice couples to fork 2 (trust) and to
the dependency-weight/audit concern (engineering-health H2). *Tentative lean: PUC 5.4 or Luau; LuaJIT's
speed is unlikely to matter for editor plugins and its FFI/portability cost is real.*

**2. Trust model.** Fully-trusted plugins (Neovim: arbitrary code, filesystem, shell — simplest, expected)
vs sandboxed/capability-limited (Luau, or a curated API with no raw IO). WordCartel's security-conscious
posture argues for at least a *considered* boundary. *Open — decide deliberately.*

**3. Hot-path hook policy.** Neovim runs Lua synchronously on its main loop; a slow autocmd janks it.
WordCartel's #1 invariant is instant typing, and the new **"Resource behavior — free at rest"** invariant
(`CLAUDE.md`) forbids blocking the input loop and idle busy-work. So plugin hooks that touch the hot path
must be **bounded, time-sliced, or dispatched onto the job substrate** — the hook API cannot be
"run arbitrary Lua synchronously on every keystroke." Define which events are hookable and their
execution model (sync-but-bounded vs async/job).

**4. The `wc.*` API surface.** Likely: register/run **commands** (the registry is the spine); **read**
buffer text/selection/block-tree; **edit** only via a validated `ChangeSet`/`submit_transaction` wrapper
(never raw); status-line output; config get/set through the shared setters (command-surface contract);
keymap registration. Decide what is exposed vs withheld, and how the command-surface contract's laws
(every option a command; palette exhaustive; one shared setter) extend to plugin-registered commands.

**5. Loading + directory convention.** A runtimepath analog (eager `plugin/` register + lazy `lua/`
require) vs an explicit manifest. Eager files must be tiny (register triggers only); heavy logic loads
on first `require`. Decide discovery, ordering, and conflict handling.

**6. Isolation, limits, failure.** Reuse the existing hardening patterns: **panic isolation**
(`catch_unwind`, M4) so a plugin error can't crash the editor or lose data; **resource caps** (M5) for
plugin CPU/memory/output; **the FFI error-propagation hazard** (LuaJIT `longjmp` vs Rust unwinding —
`mlua` manages it but it needs deliberate fault-injection testing, cf. M3). A plugin error surfaces to
the status line (typed error), never the console.

**7. Determinism + testing.** `wordcartel-core` stays PURE and untouched — plugins live only in the
shell — so core's property tests / cargo-fuzz are unaffected. The plugin surface needs its own tests:
fault injection, panic isolation, resource caps, and — per the resource-behavior invariant — a
**guardrail that a loaded-but-idle plugin does no background work** (no spin, no idle disk writes),
mirroring the swap SSD-wear guardrails.

---

## Constraints inherited from existing law (non-negotiable)
- **`#![forbid(unsafe_code)]`** (`wordcartel-core/src/lib.rs`, `wordcartel/src/lib.rs`, `main.rs`): applies
  to *our* code, not dependencies — `mlua`'s unsafe is encapsulated in the crate, so the shell hosts it
  while its own code stays unsafe-free; core never gets Lua. (Cost: the audit/dep surface grows by a
  native VM — engineering-health H2.)
- **Instant typing / never block the input loop** and the **Resource-behavior invariant** (idle is free;
  edge-triggered not level-triggered) — shapes fork 3.
- **No data loss / valid-by-construction** — plugin edits go through the validated boundary (the
  load-bearing decision).
- **Command-surface contract** — plugin-registered commands must conform (fork 4).

## Non-goals / deferred
- Not RPC / out-of-process plugins (Neovim also supports these; in-process Lua is the Effort-P target).
- Not a package manager / plugin distribution story (separate, later).
- Language bindings beyond Lua.

## References
- **Reference model 2 — Fresh (`github.com/sinelaw/fresh`)**: the sandboxed-QuickJS/TypeScript cousin,
  read at source 2026-07-11. See the "Reference model 2" section above (runtime fork, the anti-god-hub
  lesson, the Provider law, crash-isolation caution). Companion messaging findings folded into backlog
  item A17 (`docs/ux-backlog.md`); windowing findings into S1.
- `docs/design/command-surface-contract.md` (the registry = plugin spine).
- `docs/engineering-health.md` H2 (dependency weight — the VM adds to it).
- `CLAUDE.md` — Resource-behavior invariant; instant-typing / no-data-loss priorities; hardening
  campaign (M3 fault injection, M4 panic isolation, M5 resource caps — all reusable here).
- Memory: `wordcartel-plugin-roadmap`.
