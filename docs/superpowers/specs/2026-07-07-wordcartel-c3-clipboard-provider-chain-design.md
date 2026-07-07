# C3 — Robust cross-application clipboard copy-out (provider/fallback chain)

**Status:** design (brainstorm-approved 2026-07-07)
**Branch:** `effort-c3-clipboard-provider-chain`
**Scope:** shell-only (`wordcartel/src`), plus one `wordcartel-core` fact (the register, unchanged). No new crates.

---

## Command-surface contract conformance

This effort adds a user-settable option (`clipboard.provider`), so it MUST conform to the
command-surface contract (`docs/design/command-surface-contract.md`). How it honors each relevant law:

- **Registry = single source of truth / every option IS a command:** `clipboard.provider` is a
  four-state option exposed as four set-value primitive commands
  (`clipboard_provider_auto/native/osc52/off`) plus a `clipboard_provider_cycle` command — the
  contract's multi-state shape rule (set-per-state primitives + a cycle representative).
- **One shared setter:** all paths (the five commands, config load, profile/startup seeding) route
  through a single `Editor::set_clipboard_provider`. No direct field writes.
- **Palette exhaustive / menu ⊆ palette:** all five commands are registered and therefore appear in
  the palette; the palette-completeness invariant test gates this. Per shape rule 8, the four
  set-per-state primitives are tagged `menu: None` (palette-only), and the **cycle
  (`clipboard_provider_cycle`) is the stateful menu representative** — carried in the menu (Settings
  category) with state-in-label (e.g. "Clipboard: Auto"). It is NOT optional; a multi-state option
  must have exactly one stateful menu representative.
- **Hints track the active keymap:** the commands carry no default binding (clipboard-provider
  switching is a rare, palette-driven action); hint re-resolution therefore has nothing keymap-specific
  to display, which the hint-re-resolution invariant test tolerates (a command may have no binding).
- **Invariant tests are merge gates:** palette-completeness, every-option-has-a-command, and
  hint-re-resolution all run; the `SettingsSnapshot` field-guard gains a `clipboard_provider` arm.

The three durability primitives are untouched; this is a shell-UI + IO-boundary effort.

---

## 1. Problem and goals

Cross-application copy-OUT (wcartel → another app) is unreliable across the terminal × SSH ×
multiplexer × display-server matrix. Today (`clipboard.rs`) every copy unconditionally fires two
paths — a **bare** OSC 52 emit (`osc52_set`, clipboard.rs:170) to the terminal, and an **arboard**
set on the worker thread — with **no environment detection anywhere** (grep for
`TMUX`/`SSH`/`WAYLAND_DISPLAY`/`DISPLAY` finds nothing). Two concrete failures result:

1. **tmux/screen silently drop the bare OSC 52.** tmux's `set-clipboard` default changed `on`→`external`
   at 2.6, so a bare OSC 52 from an app *inside* tmux never reaches the system clipboard unless it is
   wrapped in the tmux DCS passthrough. (Verified: tmux wiki + issue #3192.)
2. **Linux persistence-on-exit.** arboard owns the clipboard by keeping the worker thread alive; on
   quit that ownership dies and, on X11/Wayland, the copied text is lost unless a clipboard manager
   performed a SAVE_TARGETS handoff. Additionally, arboard's `wayland-data-control` backend needs the
   compositor to implement `wlr-`/`ext-data-control`; where it doesn't (older GNOME/Mutter), the
   Wayland set can silently fail.

**Goal:** a "works everywhere" provider/fallback chain — reliable copy-out on local Linux (X11 and
Wayland incl. GNOME), SSH (plain / tmux / nested / screen), macOS, Windows, and WSL — favoring
reliability over avoiding helper processes, with graceful degradation to register-only and a clear
status line, never a silent data loss on the everyday path.

**Non-goals (explicitly out of scope):**
- Paste-IN robustness — already shipped (bracketed paste; `term.rs:42` + `Event::Paste` handlers). The
  Paste *command*'s `backend.get()` is updated for symmetry (below) but paste-in is not re-litigated.
- OSC 52 **read/query** — security-gated across terminals (kitty prompts; Windows Terminal ships
  write-only); we never depend on it.
- **PRIMARY** selection (X11 select-to-copy) — nice-to-have, deferred.
- A spawn-timeout watchdog for a hung helper — accepted risk (helpers fail fast with no display).

---

## 2. Architecture: three layers, one resolved plan

All behavior derives from a single pure function of the environment (plus any user override) that
produces one plan:

```rust
enum Layer1Choice { WlCopy, Xclip, Xsel, WinYank, ClipExe, Arboard, Null }
enum Osc52Wrap    { Bare, Tmux, Screen }

struct ProviderPlan {
    layer1: Layer1Choice,       // who owns the LOCAL clipboard
    osc52:  Option<Osc52Wrap>,  // None = suppress; Some(w) = emit, wrapped for the environment
}
```

- **Layer 0 — Register** (`wordcartel-core::register::Register`, single `Option<String>` slot): always
  set first, unconditionally. In-app copy/paste can never break. *Unchanged.*
- **Layer 1 — local owner**, exactly one, chosen at worker-init by detection + a PATH binary probe.
  Lives on the **worker thread** (process spawns / arboard init — off the hot path, as arboard is
  today).
- **Layer 2 — OSC 52** (`osc52` field): emitted **iff** the plan says so, wrapped per `Osc52Wrap`.
  Consumed on the **main thread** in `drain_clipboard_intents` (it writes to the terminal handle).

Both threads read the *same* `ProviderPlan`: the main thread uses `.osc52`, the worker uses `.layer1`.
`Osc52Wrap` deliberately unifies two decisions — the emission-policy decision (suppress when a local
owner already persists) and the multiplexer-wrapping decision (how to frame the bytes) — into one
value. `osc52 == None` means "a persisting local owner has it; don't double-write"; `Some(Tmux|Screen|Bare)`
means "emit, and this is the wrapping this environment needs."

---

## 3. Detection: one pure, testable function

Environment is captured in an injected snapshot (the M3 `Fs`-seam pattern: real env in production,
hand-built in tests), so all detection is unit-testable with no real env and no spawning:

```rust
enum Os { Linux, MacOs, Windows }

struct ClipEnv {
    tmux: bool,    // $TMUX present
    screen: bool,  // $STY present
    ssh: bool,     // $SSH_TTY || $SSH_CONNECTION present
    wayland: bool, // $WAYLAND_DISPLAY present
    x11: bool,     // $DISPLAY present
    wsl: bool,     // $WSL_DISTRO_NAME present
    os: Os,        // compile-time target
    // binary presence probe over $PATH; hand-stubbed in tests
    present: fn(&str) -> bool,   // "wl-copy" | "xclip" | "xsel" | "win32yank.exe" | "clip.exe"
}

fn resolve_provider(env: &ClipEnv, forced: ClipboardProvider) -> ProviderPlan
```

**`forced` short-circuits (the override, §5):**
- `Native` → `{ layer1: Arboard, osc52: None }`
- `Osc52`  → `{ layer1: Null,    osc52: Some(wrap_for(env)) }`
- `Off`    → `{ layer1: Null,    osc52: None }`  (register-only)
- `Auto`   → the logic below

**`Auto` — `layer1` selection (first match):**
1. `wsl` → `WinYank` if `present("win32yank.exe")` else `ClipExe`
2. `wayland` → `WlCopy` if `present("wl-copy")` else `Arboard`
3. `x11` → `Xclip` if `present("xclip")` else `Xsel` if `present("xsel")` else `Arboard`
4. `os == MacOs || os == Windows` → `Arboard`  (OS-owned clipboard; persists natively)
5. else → `Null`

**`Auto` — `osc52` classification:** classify in this **precedence order** (first match wins), then map
to a wrap:
1. `env.tmux || env.screen || env.ssh` → class `NeedsOsc52`. Checked FIRST, regardless of a present
   Linux helper: inside a multiplexer or over SSH the helper may own a clipboard the user cannot see,
   while OSC 52 reaches the terminal the user is actually looking at.
2. else `layer1 ∈ { WlCopy, Xclip, Xsel, WinYank, ClipExe }`, or `Arboard` on macOS/Windows → class
   `LocalPersisting`. (No SSH-server ambiguity here: reaching this arm means we are local.)
3. else (`layer1` fell to `Arboard` *as a Linux fallback*, or `Null`) → class `NeedsOsc52`.

Then map: `LocalPersisting → osc52 = None`; `NeedsOsc52 → osc52 = Some(wrap_for(env))`, where
`wrap_for(env)` is `Tmux` if `env.tmux`, else `Screen` if `env.screen`, else `Bare`.

> The precedence resolves the one overlap — a persisting helper (e.g. `WlCopy`) *and* a multiplexer/SSH:
> rule 1 wins, so we emit OSC 52 anyway. A local Wayland session with no SSH/multiplexer hits rule 2 and
> suppresses OSC 52 (the helper persists it).

`resolve_provider` is the primary test surface: a table sweeping every `ClipEnv` combination × every
`ClipboardProvider`, asserting the exact `ProviderPlan`. No spawns, no terminal.

---

## 4. OSC 52 emission and wrapping

`osc52_set` gains the wrap parameter; base64 (our valid-by-construction encoder) and the
`OSC52_MAX_ENCODED` (100_000) cap are unchanged:

```rust
fn osc52_set(text: &str, wrap: Osc52Wrap) -> Option<Vec<u8>>  // None when over the encoded cap
```

- **Bare:**   `ESC ] 52 ; c ; <b64> ESC \`  (bytes: `1b 5d 35 32 3b 63 3b … 1b 5c` — as today)
- **Tmux:**   `ESC P tmux; <bare-sequence, every 0x1B doubled> ESC \`
- **Screen:** `ESC P <bare-sequence> ESC \`

Because our base64 is always valid, we never trip Ghostty's "clear the clipboard on invalid payload"
behavior (a documented footgun for naive emitters).

**Three limitations stated explicitly (not silently unsolved):**
1. **Nested tmux** — a single tmux wrap covers the common case; true tmux-in-tmux needs the *outer*
   tmux at `set-clipboard on`, which cannot be set from inside. Documented; not auto-double-wrapped.
2. **GNU screen large payloads** — screen splits DCS strings at ~768 bytes, so long payloads need
   chunked re-wrapping. The deep-research left the exact screen chunking bytes unverified (flagged open
   gap). Screen ships **best-effort**: byte-correct for typical selections; the large-payload chunking
   is a plan-time item to source from the screen manpage/source, and if it cannot be pinned by reading
   it is escalated to the human rather than guessed (per the gating rule on unverifiable claims).
3. **Per-terminal / tmux caps** — some terminals and tmux truncate below our 100k cap; we keep the cap
   and document that very large copies may not survive the OSC 52 path (the Layer-1 helper/arboard still
   carries them locally).

---

## 5. The `clipboard.provider` override

- **Config:** a new `[clipboard]` section, `provider = "auto"`, parsed into:
  ```rust
  enum ClipboardProvider { Auto, Native, Osc52, Off }   // default Auto
  struct ClipboardConfig { pub provider: ClipboardProvider }
  ```
  added to `Config` alongside `menu`, `view`, etc. (config.rs:34). Parsing mirrors the existing
  string-enum sections (e.g. `MenuBarMode`), with an unknown value warned and defaulted to `Auto`.
- **State + shared setter:** `editor.clipboard_provider: ClipboardProvider` and one setter
  `Editor::set_clipboard_provider(v)` that (a) sets the field and (b) raises a
  `clipboard_provider_dirty` flag. `drain_clipboard_intents` (which holds `clip_tx`) observes the flag
  and sends a new `ClipReq::SelectProvider(plan.layer1)` so the worker rebuilds its Layer-1 backend
  **live, no restart**; the main side recomputes `plan.osc52` from the cached `ClipEnv` + the current
  field each drain (cheap — copies are rare). Startup and profiles seed *through* the setter (the A3
  pattern); no direct field writes anywhere.
- **Drain ordering (fixes a stale-backend window):** within a single `drain_clipboard_intents` pass,
  the `clipboard_provider_dirty` `ClipReq::SelectProvider` is sent **before** any queued
  `ClipReq::Set`/`Get`, so a copy/paste issued in the same frame as a provider change hits the *new*
  backend, never the old one.
- **Commands:** `clipboard_provider_auto`, `clipboard_provider_native`, `clipboard_provider_osc52`,
  `clipboard_provider_off` (set-value primitives, `menu: None`) + `clipboard_provider_cycle` (the
  stateful menu representative, `menu: Some(Settings)`, state-in-label). Registered in `registry.rs`;
  each routes through `set_clipboard_provider`.
- **Value semantics:** `Auto` = the detection chain; `Native` = force arboard (crate); `Osc52` = force
  the terminal path (screen-sharing / capable-but-undetected terminal); `Off` = disable the system
  clipboard entirely (register-only — a genuine privacy choice).
- **Worker rebuild:** the worker's `ClipReq::SelectProvider(Layer1Choice)` drops its current backend and
  builds the new one (`CommandBackend` variant / `ArboardBackend` / `NullBackend`). Rebuild is cheap
  and off the hot path.
- **Startup availability (fixes the init-order gap):** `spawn_worker` gains an initial-plan parameter —
  `spawn_worker(msg_tx, initial: Layer1Choice)`. At startup the resolved config is run through
  `resolve_provider` to compute the initial `Layer1Choice`, which `spawn_worker` uses to build the
  first backend, so the one-shot `Msg::ClipboardAvailability` reflects the *selected* provider (not an
  unconditional arboard probe). `editor.clipboard_provider` is seeded via `set_clipboard_provider` for
  the main-side `osc52` plan; because the worker already holds the correct initial backend, the
  startup seeding does not need to drive an immediate `SelectProvider`.
- **Persistence (Save Settings round-trip):** `clipboard.provider` participates in the settings
  override machinery like every other option — `SettingsSnapshot` gains the field (+ field-guard arm),
  and `OverridesFile`, `parse_mask`, and `compute_overrides` gain a `[clipboard]` section so the value
  serializes back out through Save Settings. The current override sections are keymap/theme/view/menu/
  mouse (settings.rs:79); clipboard joins them.

---

## 6. Backends (Layer 1)

The `ClipboardBackend` trait is unchanged (`set(&mut self, &str)`, `get(&mut self) -> Option<String>`,
`Send`). New implementations:

- **`CommandBackend`** — a single DRY impl parameterized by argv:
  ```rust
  struct CommandBackend { set_argv: Vec<String>, get_argv: Option<Vec<String>> }
  ```
  `set` spawns `set_argv`, writes `text` to the child stdin, closes it, and **`wait()`s to reap the
  direct child** — the helper self-backgrounds (wl-copy/xclip fork a persisting server and the
  foreground child exits promptly), so the wait returns fast and no zombie accumulates. `get` spawns
  `get_argv`, reads stdout to completion, and reaps; returns `None` when `get_argv` is `None` (e.g.
  `clip.exe`, which is set-only → paste falls back to register).
  Constructors: `wl_copy()` (`wl-copy` / `wl-paste --no-newline`), `xclip()`
  (`xclip -selection clipboard` / `-o`), `xsel()` (`xsel -b -i` / `-b -o`), `win_yank()`
  (`win32yank.exe -i --crlf` / `-o --lf`), `clip_exe()` (`clip.exe`, no get).
- **`ArboardBackend`** — unchanged; now selected only on mac/Windows or as the Linux fallback.
- **`NullBackend`** — unchanged (register-only).
- **`FakeBackend`** — unchanged (tests).

`Get`/paste is thus symmetric with copy: the Paste command's `backend.get()` routes through the same
selected provider. Over plain SSH there is no local display, `get()` returns `None`, and paste already
falls back to the register (existing `clipboardpaste_none_falls_back_to_register` behavior). Terminal-
native paste over SSH stays covered by bracketed paste, untouched.

**WSL newline/Unicode:** `win32yank -i --crlf` / `-o --lf` handle the CRLF boundary; `clip.exe` receives
UTF-8 via stdin. (Both are flagged for confirmation in the manual matrix; `win32yank` is preferred
precisely because it is bidirectional and LF-clean.)

---

## 7. Degradation and status

- `layer1 == Null` and `osc52 == None` (i.e. `Off`, or a headless box with no helper and a non-OSC-52
  terminal): copy still succeeds into the register; the status line reflects register-only rather than
  claiming a system copy (extends the existing `"clipboard unavailable"` notice — no silent UI).
- `Msg::ClipboardAvailability(bool)` remains a one-shot startup signal (true when `layer1 != Null` or
  OSC 52 will emit).
- A **hung helper** would block the worker thread (never the input loop). Accepted risk in v1: helpers
  fail fast when no display is reachable; no spawn-timeout watchdog. Documented.

**Accepted tail (human-ratified 2026-07-07):** copy-then-quit on Linux where Layer 1 fell to the
*arboard fallback* (no helper installed) **and** the terminal is non-OSC-52 can still lose the
selection on exit. Accepted and documented: OSC 52 covers it on any capable terminal, and installing
`wl-copy`/`xclip` closes it. No fork-persist in v1.

---

## 8. Testing

**Automated (the gates):**
- `resolve_provider` **table tests** — every `ClipEnv` combination (tmux/screen/ssh/wayland/x11/wsl ×
  os × helper-presence) × every `ClipboardProvider`, asserting the exact `ProviderPlan`. The core.
- `osc52_set` **byte-exact** tests for `Bare`, `Tmux`, `Screen` (extends
  `osc52_frames_with_st_terminator`), plus the over-cap `None` (existing `osc52_skips_oversize_payload`).
- `CommandBackend` **argv-construction** tests (each constructor yields the right argv); actual spawning
  kept thin — exercised against a trivial portable binary where feasible, otherwise the worker-level
  `drain_*` tests continue to use `FakeBackend`.
- Command-surface **invariant tests** (merge gates): palette-completeness, every-option-has-a-command
  (the `SettingsSnapshot` field-guard gains a `clipboard_provider` arm), hint re-resolution.
- Live-apply: setting the provider updates the main-side `osc52` plan and enqueues
  `ClipReq::SelectProvider` (a `drain_*`-style test with a fake worker channel).

**Manual matrix (the real cost — makes C3 a Medium, not the code volume):** local Linux X11 + Wayland
(incl. GNOME/Mutter), SSH plain, SSH + tmux, nested tmux, GNU screen, WSL, macOS, Windows. Mostly
spot-checks; the PTY smoke S5 already covers OSC 52 → tmux (happy path).

---

## 9. Files touched (map)

- `wordcartel/src/clipboard.rs` — `Layer1Choice`, `Osc52Wrap`, `ProviderPlan`, `ClipEnv`,
  `resolve_provider`, `CommandBackend` (+ constructors), `osc52_set(text, wrap)`,
  `ClipReq::SelectProvider`, `spawn_worker(msg_tx, initial: Layer1Choice)` signature change + worker
  rebuild; keep `ArboardBackend`/`NullBackend`/`FakeBackend`.
- `wordcartel/src/config.rs` — `ClipboardProvider`, `ClipboardConfig`, `Config.clipboard`,
  `RawConfig` parse of `[clipboard] provider` (hand-matched string enum with unknown→warn→`Auto`, per
  the `menu.bar`/`view.scrollbar` precedent at config.rs:402/420).
- `wordcartel/src/editor.rs` — `clipboard_provider` field, `clipboard_provider_dirty` flag,
  `set_clipboard_provider` setter (beside the A3 setters at editor.rs:825).
- `wordcartel/src/registry.rs` — the five commands (four sets `menu: None`, the cycle
  `menu: Some(Settings)` with state-in-label), each via the setter (beside the scrollbar/status/menu
  families at registry.rs:459–498).
- `wordcartel/src/settings.rs` — `SettingsSnapshot.clipboard_provider` (+ field-guard arm at
  settings.rs:950 + the every-option law test at :960) **and the persistence plumbing**: `OverridesFile`,
  `parse_mask`, `compute_overrides`, and serialization each gain a `[clipboard]` section so the value
  round-trips through Save Settings (current sections keymap/theme/view/menu/mouse at settings.rs:79).
- `wordcartel/src/palette.rs` / `menu.rs` — palette carries all five commands automatically
  (palette-completeness gate); the cycle is the menu representative (menu ⊆ palette holds — only the
  cycle appears in the menu, the four sets do not).
- `wordcartel/src/app.rs` — `spawn_worker` call (app.rs:1506) passes the startup-resolved initial
  `Layer1Choice`; `drain_clipboard_intents` (app.rs:1664) recompute + ordered dirty-flag send; startup
  config→editor seeding via `set_clipboard_provider` (beside the A3 seeding at app.rs:1378).
- No `Cargo.toml` change — arboard stays `= { version = "3", default-features = false,
  features = ["wayland-data-control"] }`; helper backends use only `std::process`; no new crates.

---

## 10. Open items carried into the plan

1. **GNU screen exact DCS bytes + long-payload chunking** (§4.2) — source from the screen
   manpage/source at plan time; escalate to the human if unverifiable by reading.
2. **WSL get path** — confirm `win32yank -o --lf` vs a PowerShell `Get-Clipboard` fallback, and the
   CRLF/Unicode boundary, in the manual matrix.
3. **Per-terminal OSC 52 size caps** — keep the 100k cap; document tmux/terminal truncation. (No code
   change; a doc note + the manual matrix.)

These are point-in-time facts (tmux `set-clipboard` semantics, terminal defaults, crate versions) and
should be re-checked at implementation.
