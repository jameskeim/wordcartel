# C3 Clipboard Provider-Chain Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make cross-application clipboard copy-OUT robust across local terminals, SSH, tmux, GNU screen, Wayland/X11, macOS, Windows, and WSL, via a detection-driven provider/fallback chain with a user-settable `clipboard.provider` override.

**Architecture:** A single pure `resolve_provider(ClipEnv, forced) -> ProviderPlan` computes, from the environment plus any override, a `ProviderPlan { layer1: Layer1Choice, osc52: Option<Osc52Wrap> }`. The worker thread owns Layer 1 (external helper via `CommandBackend`, or `arboard`, or `Null`); the main thread emits wrapped OSC 52 (Layer 2) per the plan; the in-process register (`wordcartel-core`) stays the source of truth. The `clipboard.provider` option conforms to the command-surface contract (four palette-only set primitives + one menu cycle representative).

**Tech Stack:** Rust, `wordcartel` shell crate (ratatui 0.30, crossterm), `arboard` 3 (existing dep), `std::process` for helper backends. No new crates.

## Global Constraints

- **No new crates.** `arboard` stays `= { version = "3", default-features = false, features = ["wayland-data-control"] }` (Cargo.toml:22). Helper backends use `std::process` only.
- **House style:** snake_case fns, 4-space indent, hand-wrapped ~100 cols; em-dash `—` in prose comments, never `--`; no emoji in code (tests may use multibyte text). Do NOT run `cargo fmt`.
- **No `.unwrap()` on fallible/external paths;** prefer `.expect("…invariant…")`. Exhaustive matches — no catch-all `_` that would absorb a new enum variant.
- **Merge GATEs:** `cargo test -p wordcartel-core -p wordcartel` green; `cargo clippy --workspace --all-targets` clean (workspace denies `clippy::all`); `cargo build` + `cargo test --no-run` warning-free for touched crates. Deliberate clippy exceptions require an item-local `#[allow(clippy::…)]` with a one-line rationale.
- **Command-surface contract** (`docs/design/command-surface-contract.md`): every user-settable option IS a command via ONE shared setter; multi-state = set-per-state primitives (`menu: None`) + one stateful menu representative (a cycle here, `menu: Some(Settings)`, state-in-label); palette exhaustive; menu ⊆ palette; hints track the active keymap. The invariant tests (palette-completeness, every-option-has-a-command, hint re-resolution) are merge gates.
- **OSC 52:** base64 is valid-by-construction (existing `base64_encode`); `OSC52_MAX_ENCODED = 100_000` (limits.rs:19) cap unchanged; over-cap → `None` (skip emission).
- **Threading:** Layer 1 runs on the clipboard worker thread (never the input loop); `set` writes child stdin then `wait()`s to reap; `get` reads stdout then reaps.

---

## File structure

- `wordcartel/src/clipboard.rs` — detection types + `resolve_provider`, `CommandBackend`, `osc52_set(text, wrap)`, `ClipReq::SelectProvider`, `spawn_worker(msg_tx, initial: ProviderPlan)`, worker rebuild. (The provider engine lives here beside the existing backends and worker.)
- `wordcartel/src/config.rs` — `ClipboardProvider` enum, `ClipboardConfig`, `Config.clipboard`, `RawClipboard`, parse block, `clipboard_provider_str` helper.
- `wordcartel/src/editor.rs` — `clipboard_provider` field, `clipboard_provider_dirty` flag, `set_clipboard_provider` setter.
- `wordcartel/src/registry.rs` — five commands (four sets + cycle).
- `wordcartel/src/settings.rs` — `SettingsSnapshot.clipboard_provider` + field-guard arm + law assertion; `OClipboard` override section wired through `OverridesFile`/`parse_mask`/`snapshot_of`/`runtime_snapshot`/`compute_overrides`.
- `wordcartel/src/app.rs` — startup resolves the initial `ProviderPlan`, passes it to `spawn_worker`, seeds the setter + clears dirty; `drain_clipboard_intents` recompute + ordered send. (Most drain logic lives in clipboard.rs; app.rs holds the call sites.)

Task order respects dependencies: 1 (detection types) → 2 (osc52 wrap) → 3 (CommandBackend) → 4 (config) → 5 (editor setter) → 6 (registry) → 7 (settings persistence) → 8 (worker integration) → 9 (drain + startup wiring).

---

### Task 1: Detection core — types + `resolve_provider`

**Files:**
- Modify: `wordcartel/src/clipboard.rs` (add types + function near the top, after the existing `use` block ~line 11; tests into the existing `#[cfg(test)] mod tests` ~line 184)

**Interfaces:**
- Consumes: nothing (pure, self-contained).
- Produces:
  - `pub enum Layer1Choice { WlCopy, Xclip, Xsel, WinYank, ClipExe, Arboard, Null }` (derive `Clone, Copy, Debug, PartialEq, Eq`)
  - `pub enum Osc52Wrap { Bare, Tmux, Screen }` (derive `Clone, Copy, Debug, PartialEq, Eq`)
  - `pub struct ProviderPlan { pub layer1: Layer1Choice, pub osc52: Option<Osc52Wrap> }` (derive `Clone, Copy, Debug, PartialEq, Eq`)
  - `pub enum Os { Linux, MacOs, Windows }` (derive `Clone, Copy, Debug, PartialEq, Eq`)
  - `pub struct ClipEnv { pub tmux: bool, pub screen: bool, pub ssh: bool, pub wayland: bool, pub x11: bool, pub wsl: bool, pub os: Os, pub present: fn(&str) -> bool }`
  - `pub fn resolve_provider(env: &ClipEnv, forced: crate::config::ClipboardProvider) -> ProviderPlan`
  - `pub fn clip_env_from_process() -> ClipEnv` (reads real env; used by app.rs startup in Task 9)

> **Note on `ClipboardProvider`:** Task 1 references `crate::config::ClipboardProvider`, which Task 4 creates. To keep Task 1 independently compilable and testable, **define the enum in config.rs as the first step of Task 1** (move only the enum definition here; Task 4 adds the `ClipboardConfig`/parse/helper around it). The enum:
> ```rust
> // wordcartel/src/config.rs — near the other option enums (after TransientMode ~line 90)
> /// Clipboard provider selection (`[clipboard] provider`). `Auto` runs the detection
> /// chain; `Native` forces arboard; `Osc52` forces the terminal path; `Off` disables
> /// the system clipboard (in-process register only).
> #[derive(Debug, Clone, Copy, PartialEq, Eq)]
> pub enum ClipboardProvider { Auto, Native, Osc52, Off }
> ```

- [ ] **Step 1: Add the `ClipboardProvider` enum to config.rs** (the block above, after `TransientMode` at config.rs:90). This unblocks Task 1's references without pulling in Task 4's parse machinery.

- [ ] **Step 2: Write the failing tests** in `clipboard.rs` tests module (`use super::*;` is already in scope):

```rust
#[test]
fn resolve_forced_native_is_arboard_no_osc52() {
    let env = ClipEnv { tmux: false, screen: false, ssh: false, wayland: true, x11: false,
                        wsl: false, os: Os::Linux, present: |_| true };
    assert_eq!(resolve_provider(&env, crate::config::ClipboardProvider::Native),
               ProviderPlan { layer1: Layer1Choice::Arboard, osc52: None });
}

#[test]
fn resolve_forced_osc52_is_null_plus_wrapped() {
    let env = ClipEnv { tmux: true, screen: false, ssh: false, wayland: true, x11: false,
                        wsl: false, os: Os::Linux, present: |_| true };
    assert_eq!(resolve_provider(&env, crate::config::ClipboardProvider::Osc52),
               ProviderPlan { layer1: Layer1Choice::Null, osc52: Some(Osc52Wrap::Tmux) });
}

#[test]
fn resolve_forced_off_is_register_only() {
    let env = ClipEnv { tmux: false, screen: false, ssh: false, wayland: false, x11: true,
                        wsl: false, os: Os::Linux, present: |_| true };
    assert_eq!(resolve_provider(&env, crate::config::ClipboardProvider::Off),
               ProviderPlan { layer1: Layer1Choice::Null, osc52: None });
}

#[test]
fn resolve_auto_local_wayland_with_helper_suppresses_osc52() {
    let env = ClipEnv { tmux: false, screen: false, ssh: false, wayland: true, x11: false,
                        wsl: false, os: Os::Linux, present: |b| b == "wl-copy" };
    assert_eq!(resolve_provider(&env, crate::config::ClipboardProvider::Auto),
               ProviderPlan { layer1: Layer1Choice::WlCopy, osc52: None });
}

#[test]
fn resolve_auto_wayland_no_helper_falls_to_arboard_and_emits_osc52() {
    let env = ClipEnv { tmux: false, screen: false, ssh: false, wayland: true, x11: false,
                        wsl: false, os: Os::Linux, present: |_| false };
    assert_eq!(resolve_provider(&env, crate::config::ClipboardProvider::Auto),
               ProviderPlan { layer1: Layer1Choice::Arboard, osc52: Some(Osc52Wrap::Bare) });
}

#[test]
fn resolve_auto_x11_prefers_xclip_then_xsel() {
    let both = ClipEnv { tmux: false, screen: false, ssh: false, wayland: false, x11: true,
                         wsl: false, os: Os::Linux, present: |b| b == "xclip" || b == "xsel" };
    assert_eq!(resolve_provider(&both, crate::config::ClipboardProvider::Auto).layer1, Layer1Choice::Xclip);
    let only_xsel = ClipEnv { present: |b| b == "xsel", ..both };
    assert_eq!(resolve_provider(&only_xsel, crate::config::ClipboardProvider::Auto).layer1, Layer1Choice::Xsel);
}

#[test]
fn resolve_auto_tmux_forces_osc52_even_with_local_helper() {
    let env = ClipEnv { tmux: true, screen: false, ssh: false, wayland: true, x11: false,
                        wsl: false, os: Os::Linux, present: |_| true };
    // helper present (would persist) but multiplexer wins: emit, tmux-wrapped.
    let plan = resolve_provider(&env, crate::config::ClipboardProvider::Auto);
    assert_eq!(plan.layer1, Layer1Choice::WlCopy);
    assert_eq!(plan.osc52, Some(Osc52Wrap::Tmux));
}

#[test]
fn resolve_auto_ssh_no_display_is_null_plus_bare_osc52() {
    let env = ClipEnv { tmux: false, screen: false, ssh: true, wayland: false, x11: false,
                        wsl: false, os: Os::Linux, present: |_| false };
    assert_eq!(resolve_provider(&env, crate::config::ClipboardProvider::Auto),
               ProviderPlan { layer1: Layer1Choice::Null, osc52: Some(Osc52Wrap::Bare) });
}

#[test]
fn resolve_auto_macos_is_arboard_no_osc52() {
    let env = ClipEnv { tmux: false, screen: false, ssh: false, wayland: false, x11: false,
                        wsl: false, os: Os::MacOs, present: |_| false };
    assert_eq!(resolve_provider(&env, crate::config::ClipboardProvider::Auto),
               ProviderPlan { layer1: Layer1Choice::Arboard, osc52: None });
}

#[test]
fn resolve_auto_wsl_prefers_win32yank_then_clip_exe() {
    let yank = ClipEnv { tmux: false, screen: false, ssh: false, wayland: false, x11: false,
                         wsl: true, os: Os::Linux, present: |b| b == "win32yank.exe" };
    assert_eq!(resolve_provider(&yank, crate::config::ClipboardProvider::Auto).layer1, Layer1Choice::WinYank);
    let clip = ClipEnv { present: |_| false, ..yank };
    assert_eq!(resolve_provider(&clip, crate::config::ClipboardProvider::Auto).layer1, Layer1Choice::ClipExe);
}

#[test]
fn resolve_auto_screen_wraps_screen() {
    let env = ClipEnv { tmux: false, screen: true, ssh: false, wayland: false, x11: false,
                        wsl: false, os: Os::Linux, present: |_| false };
    assert_eq!(resolve_provider(&env, crate::config::ClipboardProvider::Auto).osc52, Some(Osc52Wrap::Screen));
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p wordcartel --lib resolve_ 2>&1 | tail -20`
Expected: FAIL — `cannot find type ProviderPlan` / `function resolve_provider not found`.

- [ ] **Step 4: Implement the types and function** in `clipboard.rs` (after the `use` block, ~line 11):

```rust
/// Who owns the LOCAL system clipboard (Layer 1). Selected once at worker init and on a
/// runtime provider change; `Null` = register-only (no system clipboard).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer1Choice { WlCopy, Xclip, Xsel, WinYank, ClipExe, Arboard, Null }

/// OSC 52 framing (Layer 2). `Bare` outside a multiplexer; `Tmux`/`Screen` wrap the bare
/// sequence in the multiplexer's DCS passthrough.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Osc52Wrap { Bare, Tmux, Screen }

/// The resolved plan: Layer-1 owner + whether/how to also emit OSC 52. `osc52 == None`
/// means a local owner persists the clipboard, so we suppress the redundant terminal write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderPlan { pub layer1: Layer1Choice, pub osc52: Option<Osc52Wrap> }

/// Compile-time target OS class (drives arboard-native vs helper selection).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os { Linux, MacOs, Windows }

/// Environment snapshot for provider detection. `present` probes `$PATH` for a helper
/// binary; injected so tests need no real env and no spawning.
#[derive(Clone, Copy)]
pub struct ClipEnv {
    pub tmux: bool,    // $TMUX
    pub screen: bool,  // $STY
    pub ssh: bool,     // $SSH_TTY || $SSH_CONNECTION
    pub wayland: bool, // $WAYLAND_DISPLAY
    pub x11: bool,     // $DISPLAY
    pub wsl: bool,     // $WSL_DISTRO_NAME
    pub os: Os,
    pub present: fn(&str) -> bool,
}

/// Multiplexer wrap for the current environment (tmux beats screen beats bare).
fn wrap_for(env: &ClipEnv) -> Osc52Wrap {
    if env.tmux { Osc52Wrap::Tmux } else if env.screen { Osc52Wrap::Screen } else { Osc52Wrap::Bare }
}

/// Pick the Layer-1 owner under `Auto` (first match wins).
fn auto_layer1(env: &ClipEnv) -> Layer1Choice {
    if env.wsl {
        return if (env.present)("win32yank.exe") { Layer1Choice::WinYank } else { Layer1Choice::ClipExe };
    }
    if env.wayland {
        return if (env.present)("wl-copy") { Layer1Choice::WlCopy } else { Layer1Choice::Arboard };
    }
    if env.x11 {
        return if (env.present)("xclip") { Layer1Choice::Xclip }
               else if (env.present)("xsel") { Layer1Choice::Xsel }
               else { Layer1Choice::Arboard };
    }
    match env.os {
        Os::MacOs | Os::Windows => Layer1Choice::Arboard,
        Os::Linux => Layer1Choice::Null,
    }
}

/// Whether the chosen Layer-1 owner persists the clipboard locally on its own.
fn is_local_persisting(layer1: Layer1Choice, env: &ClipEnv) -> bool {
    match layer1 {
        Layer1Choice::WlCopy | Layer1Choice::Xclip | Layer1Choice::Xsel
        | Layer1Choice::WinYank | Layer1Choice::ClipExe => true,
        // arboard persists natively only where the OS owns the clipboard.
        Layer1Choice::Arboard => matches!(env.os, Os::MacOs | Os::Windows),
        Layer1Choice::Null => false,
    }
}

/// Resolve the environment (+ any override) into a concrete plan. Pure.
pub fn resolve_provider(env: &ClipEnv, forced: crate::config::ClipboardProvider) -> ProviderPlan {
    use crate::config::ClipboardProvider as P;
    match forced {
        P::Native => ProviderPlan { layer1: Layer1Choice::Arboard, osc52: None },
        P::Osc52  => ProviderPlan { layer1: Layer1Choice::Null, osc52: Some(wrap_for(env)) },
        P::Off    => ProviderPlan { layer1: Layer1Choice::Null, osc52: None },
        P::Auto => {
            let layer1 = auto_layer1(env);
            // Precedence: multiplexer/SSH forces OSC 52 (rule 1); else a persisting local
            // owner suppresses it (rule 2); else emit (rule 3).
            let osc52 = if env.tmux || env.screen || env.ssh {
                Some(wrap_for(env))
            } else if is_local_persisting(layer1, env) {
                None
            } else {
                Some(wrap_for(env))
            };
            ProviderPlan { layer1, osc52 }
        }
    }
}

/// Build a `ClipEnv` from the real process environment.
pub fn clip_env_from_process() -> ClipEnv {
    fn var_set(k: &str) -> bool { std::env::var_os(k).is_some_and(|v| !v.is_empty()) }
    fn on_path(bin: &str) -> bool {
        let Some(paths) = std::env::var_os("PATH") else { return false };
        std::env::split_paths(&paths).any(|dir| dir.join(bin).is_file())
    }
    let os = if cfg!(target_os = "macos") { Os::MacOs }
             else if cfg!(target_os = "windows") { Os::Windows }
             else { Os::Linux };
    ClipEnv {
        tmux: var_set("TMUX"),
        screen: var_set("STY"),
        ssh: var_set("SSH_TTY") || var_set("SSH_CONNECTION"),
        wayland: var_set("WAYLAND_DISPLAY"),
        x11: var_set("DISPLAY"),
        wsl: var_set("WSL_DISTRO_NAME"),
        os,
        present: on_path,
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p wordcartel --lib resolve_ 2>&1 | tail -20`
Expected: PASS (all `resolve_*` tests).

- [ ] **Step 6: Clippy + commit**

Run: `cargo clippy -p wordcartel --all-targets 2>&1 | tail -5` → expect clean.
```bash
git add wordcartel/src/clipboard.rs wordcartel/src/config.rs
git commit -m "feat(c3): provider detection types + resolve_provider"
```

---

### Task 2: OSC 52 wrapping — `osc52_set(text, wrap)`

**Files:**
- Modify: `wordcartel/src/clipboard.rs` — change `osc52_set` (currently `osc52_set(text: &str) -> Option<Vec<u8>>` at :170) and its one caller in `drain_clipboard_intents` (:42); tests in the tests module.

**Interfaces:**
- Consumes: `Osc52Wrap` (Task 1); `OSC52_MAX_ENCODED`, `base64_encode` (existing).
- Produces: `pub fn osc52_set(text: &str, wrap: Osc52Wrap) -> Option<Vec<u8>>`.

> The existing caller `drain_clipboard_intents` calls `osc52_set(&text)`. This task changes the
> signature; keep the caller compiling and behavior-identical by passing `Osc52Wrap::Bare` there for
> now (Task 9 replaces it with the plan's real wrap).

- [ ] **Step 1: Write the failing tests** (extends the existing `osc52_frames_with_st_terminator`):

```rust
#[test]
fn osc52_bare_frames_with_st() {
    // "hi" → base64 "aGk="
    assert_eq!(osc52_set("hi", Osc52Wrap::Bare).unwrap(), b"\x1b]52;c;aGk=\x1b\\".to_vec());
}

#[test]
fn osc52_tmux_wraps_and_doubles_inner_esc() {
    // tmux DCS passthrough: ESC P tmux; <inner with every 0x1b doubled> ESC \
    let got = osc52_set("hi", Osc52Wrap::Tmux).unwrap();
    assert_eq!(got, b"\x1bPtmux;\x1b\x1b]52;c;aGk=\x1b\x1b\\\x1b\\".to_vec());
}

#[test]
fn osc52_screen_wraps_without_doubling() {
    // screen DCS passthrough: ESC P <inner> ESC \  (no ESC-doubling)
    let got = osc52_set("hi", Osc52Wrap::Screen).unwrap();
    assert_eq!(got, b"\x1bP\x1b]52;c;aGk=\x1b\\\x1b\\".to_vec());
}

#[test]
fn osc52_oversize_returns_none_for_every_wrap() {
    let big = "a".repeat(OSC52_MAX_ENCODED); // base64 grows it beyond the cap
    assert!(osc52_set(&big, Osc52Wrap::Bare).is_none());
    assert!(osc52_set(&big, Osc52Wrap::Tmux).is_none());
    assert!(osc52_set(&big, Osc52Wrap::Screen).is_none());
}
```

- [ ] **Step 2: Delete/replace the old `osc52_frames_with_st_terminator` test** if it calls the old one-arg signature (it asserts `osc52_set("hi")`); `osc52_bare_frames_with_st` above supersedes it. Remove the stale test to avoid a compile error.

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p wordcartel --lib osc52_ 2>&1 | tail -20`
Expected: FAIL — arity mismatch / new wrap tests undefined.

- [ ] **Step 4: Implement** — replace `osc52_set` (:170):

```rust
/// OSC 52 "set clipboard" sequence for `text`, framed per `wrap`. `None` when the
/// base64 payload exceeds `OSC52_MAX_ENCODED` (caller skips emission; Layer 1 still copies).
///
/// - `Bare`:   ESC ] 52 ; c ; <b64> ESC \
/// - `Tmux`:   ESC P tmux; <bare, every 0x1B doubled> ESC \   (tmux DCS passthrough)
/// - `Screen`: ESC P <bare> ESC \                              (screen DCS passthrough)
pub fn osc52_set(text: &str, wrap: Osc52Wrap) -> Option<Vec<u8>> {
    let b64 = base64_encode(text.as_bytes());
    if b64.len() > OSC52_MAX_ENCODED {
        return None;
    }
    let mut bare = Vec::with_capacity(b64.len() + 9);
    bare.extend_from_slice(b"\x1b]52;c;");
    bare.extend_from_slice(b64.as_bytes());
    bare.extend_from_slice(b"\x1b\\");
    let framed = match wrap {
        Osc52Wrap::Bare => bare,
        Osc52Wrap::Tmux => {
            // Double every ESC (0x1B) in the inner sequence, then wrap.
            let mut v = Vec::with_capacity(bare.len() + 16);
            v.extend_from_slice(b"\x1bPtmux;");
            for &byte in &bare {
                if byte == 0x1b { v.push(0x1b); }
                v.push(byte);
            }
            v.extend_from_slice(b"\x1b\\");
            v
        }
        Osc52Wrap::Screen => {
            let mut v = Vec::with_capacity(bare.len() + 4);
            v.extend_from_slice(b"\x1bP");
            v.extend_from_slice(&bare);
            v.extend_from_slice(b"\x1b\\");
            v
        }
    };
    Some(framed)
}
```

- [ ] **Step 5: Update the caller** in `drain_clipboard_intents` (:42) to pass `Osc52Wrap::Bare` (temporary; Task 9 wires the plan wrap):

```rust
if let Some(bytes) = osc52_set(&text, Osc52Wrap::Bare) {
    let _ = out.write_all(&bytes);
    let _ = out.flush();
}
```

- [ ] **Step 6: Run tests + clippy**

Run: `cargo test -p wordcartel --lib osc52_ 2>&1 | tail -20` → PASS.
Run: `cargo clippy -p wordcartel --all-targets 2>&1 | tail -5` → clean.

- [ ] **Step 7: Commit**

```bash
git add wordcartel/src/clipboard.rs
git commit -m "feat(c3): osc52_set gains tmux/screen wrapping"
```

---

### Task 3: `CommandBackend` — external-helper backend

**Files:**
- Modify: `wordcartel/src/clipboard.rs` — add `CommandBackend` beside `ArboardBackend` (~line 119); tests in the tests module.

**Interfaces:**
- Consumes: the existing `ClipboardBackend` trait (`fn set(&mut self, text: &str); fn get(&mut self) -> Option<String>;`, `: Send`, clipboard.rs:94).
- Produces:
  - `pub struct CommandBackend { set_argv: Vec<String>, get_argv: Option<Vec<String>> }`
  - constructors `wl_copy()`, `xclip()`, `xsel()`, `win_yank()`, `clip_exe()` → `CommandBackend`
  - `pub fn backend_for(choice: Layer1Choice) -> Box<dyn ClipboardBackend>` (maps a `Layer1Choice` to a boxed backend; used by the worker in Task 8)

- [ ] **Step 1: Write the failing tests**:

```rust
#[test]
fn command_backend_argv_constructors() {
    assert_eq!(CommandBackend::wl_copy().set_argv, vec!["wl-copy".to_string()]);
    assert_eq!(CommandBackend::wl_copy().get_argv,
               Some(vec!["wl-paste".to_string(), "--no-newline".to_string()]));
    assert_eq!(CommandBackend::xclip().set_argv,
               vec!["xclip", "-selection", "clipboard"].iter().map(|s| s.to_string()).collect::<Vec<_>>());
    assert_eq!(CommandBackend::xsel().set_argv,
               vec!["xsel", "-b", "-i"].iter().map(|s| s.to_string()).collect::<Vec<_>>());
    assert_eq!(CommandBackend::win_yank().get_argv,
               Some(vec!["win32yank.exe".to_string(), "-o".to_string(), "--lf".to_string()]));
    assert!(CommandBackend::clip_exe().get_argv.is_none()); // set-only
}

#[test]
fn backend_for_maps_choices() {
    // Smoke: mapping does not panic and Null yields an inert backend.
    let mut null = backend_for(Layer1Choice::Null);
    null.set("x");
    assert_eq!(null.get(), None);
}

#[test]
fn command_backend_roundtrips_via_cat_like_helper() {
    // Use a POSIX shell to emulate a clipboard: set writes to a temp file, get reads it.
    // Skips cleanly where /bin/sh is unavailable (non-unix CI).
    if !std::path::Path::new("/bin/sh").exists() { return; }
    let dir = std::env::temp_dir().join(format!("wcartel-clip-test-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let slot = dir.join("slot");
    let slot_s = slot.to_string_lossy().to_string();
    let mut b = CommandBackend {
        set_argv: vec!["/bin/sh".into(), "-c".into(), format!("cat > {slot_s}")],
        get_argv: Some(vec!["/bin/sh".into(), "-c".into(), format!("cat {slot_s}")]),
    };
    b.set("hello");
    assert_eq!(b.get().as_deref(), Some("hello"));
    let _ = std::fs::remove_dir_all(&dir);
}
```

- [ ] **Step 2: Run to verify fail**

Run: `cargo test -p wordcartel --lib command_backend 2>&1 | tail -20`
Expected: FAIL — `CommandBackend` / `backend_for` undefined.

- [ ] **Step 3: Implement** in `clipboard.rs`:

```rust
use std::io::Write as _;

/// A Layer-1 backend that shells out to an external clipboard helper. `set` writes the
/// text to the child's stdin and reaps it (helpers self-background, so the foreground
/// child exits promptly). `get` reads stdout; `None` get_argv means set-only (e.g. clip.exe),
/// so paste falls back to the register.
pub struct CommandBackend {
    set_argv: Vec<String>,
    get_argv: Option<Vec<String>>,
}

impl CommandBackend {
    pub fn wl_copy() -> Self {
        CommandBackend { set_argv: vec!["wl-copy".into()],
                         get_argv: Some(vec!["wl-paste".into(), "--no-newline".into()]) }
    }
    pub fn xclip() -> Self {
        CommandBackend { set_argv: vec!["xclip".into(), "-selection".into(), "clipboard".into()],
                         get_argv: Some(vec!["xclip".into(), "-selection".into(), "clipboard".into(), "-o".into()]) }
    }
    pub fn xsel() -> Self {
        CommandBackend { set_argv: vec!["xsel".into(), "-b".into(), "-i".into()],
                         get_argv: Some(vec!["xsel".into(), "-b".into(), "-o".into()]) }
    }
    pub fn win_yank() -> Self {
        CommandBackend { set_argv: vec!["win32yank.exe".into(), "-i".into(), "--crlf".into()],
                         get_argv: Some(vec!["win32yank.exe".into(), "-o".into(), "--lf".into()]) }
    }
    pub fn clip_exe() -> Self {
        CommandBackend { set_argv: vec!["clip.exe".into()], get_argv: None }
    }
}

impl ClipboardBackend for CommandBackend {
    fn set(&mut self, text: &str) {
        use std::process::{Command, Stdio};
        let Some((bin, args)) = self.set_argv.split_first() else { return };
        let child = Command::new(bin).args(args)
            .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
        if let Ok(mut ch) = child {
            if let Some(mut stdin) = ch.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
                // drop stdin → EOF so the helper commits and its foreground child exits.
            }
            let _ = ch.wait(); // reap the (promptly-exiting) foreground child.
        }
    }
    fn get(&mut self) -> Option<String> {
        use std::process::{Command, Stdio};
        let argv = self.get_argv.as_ref()?;
        let (bin, args) = argv.split_first()?;
        let out = Command::new(bin).args(args)
            .stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
        if !out.status.success() { return None; }
        let s = String::from_utf8_lossy(&out.stdout).into_owned();
        if s.is_empty() { None } else { Some(s) }
    }
}

/// Map a resolved Layer-1 choice to a boxed backend. `Arboard`/`Null` reuse the existing
/// backends; helpers use `CommandBackend`. arboard init failure degrades to `NullBackend`.
pub fn backend_for(choice: Layer1Choice) -> Box<dyn ClipboardBackend> {
    match choice {
        Layer1Choice::WlCopy  => Box::new(CommandBackend::wl_copy()),
        Layer1Choice::Xclip   => Box::new(CommandBackend::xclip()),
        Layer1Choice::Xsel    => Box::new(CommandBackend::xsel()),
        Layer1Choice::WinYank => Box::new(CommandBackend::win_yank()),
        Layer1Choice::ClipExe => Box::new(CommandBackend::clip_exe()),
        Layer1Choice::Arboard => match ArboardBackend::try_new() {
            Some(b) => Box::new(b),
            None => Box::new(NullBackend),
        },
        Layer1Choice::Null => Box::new(NullBackend),
    }
}
```

- [ ] **Step 4: Run tests + clippy**

Run: `cargo test -p wordcartel --lib command_backend 2>&1 | tail -20` → PASS (roundtrip runs on unix, returns early elsewhere).
Run: `cargo test -p wordcartel --lib backend_for 2>&1 | tail -10` → PASS.
Run: `cargo clippy -p wordcartel --all-targets 2>&1 | tail -5` → clean.

- [ ] **Step 5: Commit**

```bash
git add wordcartel/src/clipboard.rs
git commit -m "feat(c3): CommandBackend external-helper Layer-1 + backend_for"
```

---

### Task 4: Config — `[clipboard] provider` parse + serialize helper

**Files:**
- Modify: `wordcartel/src/config.rs` — `ClipboardConfig`, `Config.clipboard` (:34), `RawClipboard`, parse block (near :420), `clipboard_provider_str` (beside `transient_mode_str` :495). (`ClipboardProvider` enum already added in Task 1 Step 1.)

**Interfaces:**
- Consumes: `ClipboardProvider` (Task 1 Step 1).
- Produces: `Config.clipboard: ClipboardConfig`, `ClipboardConfig { provider: ClipboardProvider }` (Default `Auto`), `pub fn clipboard_provider_str(p: ClipboardProvider) -> &'static str`.

- [ ] **Step 1: Write the failing tests** (in config.rs test module — follow the existing config-parse test style):

```rust
#[test]
fn clipboard_provider_parses_all_values() {
    for (s, want) in [("auto", ClipboardProvider::Auto), ("native", ClipboardProvider::Native),
                      ("osc52", ClipboardProvider::Osc52), ("off", ClipboardProvider::Off)] {
        let (cfg, _warns) = Config::from_toml_str(&format!("[clipboard]\nprovider = \"{s}\"\n"));
        assert_eq!(cfg.clipboard.provider, want, "value {s}");
    }
}

#[test]
fn clipboard_provider_unknown_warns_and_defaults_auto() {
    let (cfg, warns) = Config::from_toml_str("[clipboard]\nprovider = \"telepathy\"\n");
    assert_eq!(cfg.clipboard.provider, ClipboardProvider::Auto);
    assert!(warns.iter().any(|w| w.contains("clipboard.provider")));
}

#[test]
fn clipboard_provider_default_is_auto() {
    let (cfg, _) = Config::from_toml_str("");
    assert_eq!(cfg.clipboard.provider, ClipboardProvider::Auto);
}

#[test]
fn clipboard_provider_str_roundtrips() {
    assert_eq!(clipboard_provider_str(ClipboardProvider::Auto), "auto");
    assert_eq!(clipboard_provider_str(ClipboardProvider::Native), "native");
    assert_eq!(clipboard_provider_str(ClipboardProvider::Osc52), "osc52");
    assert_eq!(clipboard_provider_str(ClipboardProvider::Off), "off");
}
```

> **Anchor check:** confirm the real constructor used by config tests — the extraction shows parse
> returns `(Config, warns)`. If the crate's test constructor is named differently (e.g.
> `Config::parse_str`/`load_from_str`), use that exact name; the assertions are otherwise unchanged.

- [ ] **Step 2: Run to verify fail**

Run: `cargo test -p wordcartel --lib clipboard_provider 2>&1 | tail -20`
Expected: FAIL — `cfg.clipboard` field / `clipboard_provider_str` missing.

- [ ] **Step 3: Implement.** Add the config section struct (after `MenuConfig` ~line 97):

```rust
/// Clipboard configuration section (`[clipboard]`).
#[derive(Debug, Clone)]
pub struct ClipboardConfig { pub provider: ClipboardProvider }
impl Default for ClipboardConfig {
    fn default() -> Self { ClipboardConfig { provider: ClipboardProvider::Auto } }
}
```

Add the field to `Config` (config.rs:34 struct):

```rust
    pub menu: MenuConfig,
    pub clipboard: ClipboardConfig,
```

Add the raw section (beside `RawMenu` ~line 279):

```rust
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawClipboard {
    provider: Option<String>,
}
```

Add `clipboard: RawClipboard` to the `RawConfig` struct (find the struct listing `menu: RawMenu`), then the parse block (after the menu.bar block ~line 428):

```rust
// clipboard: per-field override; enum-valued string with a warning on unknowns.
if let Some(p) = raw.clipboard.provider {
    match p.as_str() {
        "auto"   => cfg.clipboard.provider = ClipboardProvider::Auto,
        "native" => cfg.clipboard.provider = ClipboardProvider::Native,
        "osc52"  => cfg.clipboard.provider = ClipboardProvider::Osc52,
        "off"    => cfg.clipboard.provider = ClipboardProvider::Off,
        other => warns.push(format!("clipboard.provider \"{other}\" invalid; using auto")),
    }
}
```

Add the serialize helper (beside `transient_mode_str` ~line 495):

```rust
/// "auto"/"native"/"osc52"/"off" — round-trips `ClipboardProvider` for the overrides mirror.
pub fn clipboard_provider_str(p: ClipboardProvider) -> &'static str {
    match p {
        ClipboardProvider::Auto => "auto",
        ClipboardProvider::Native => "native",
        ClipboardProvider::Osc52 => "osc52",
        ClipboardProvider::Off => "off",
    }
}
```

- [ ] **Step 4: Run tests + clippy**

Run: `cargo test -p wordcartel --lib clipboard_provider 2>&1 | tail -20` → PASS.
Run: `cargo clippy -p wordcartel --all-targets 2>&1 | tail -5` → clean.

- [ ] **Step 5: Commit**

```bash
git add wordcartel/src/config.rs
git commit -m "feat(c3): [clipboard] provider config parse + serialize helper"
```

---

### Task 5: Editor field + `set_clipboard_provider` setter

**Files:**
- Modify: `wordcartel/src/editor.rs` — add fields near the clipboard block (:390) and the setter beside the A3 setters (:825); its constructor/default init.

**Interfaces:**
- Consumes: `ClipboardProvider` (config).
- Produces:
  - `editor.clipboard_provider: crate::config::ClipboardProvider`
  - `editor.clipboard_provider_dirty: bool`
  - `pub fn set_clipboard_provider(&mut self, provider: crate::config::ClipboardProvider)`
  - `pub fn clear_clipboard_provider_dirty(&mut self)` (used by startup seeding, Task 9)

- [ ] **Step 1: Write the failing test** (editor.rs test module):

```rust
#[test]
fn set_clipboard_provider_sets_field_and_dirty() {
    let mut e = Editor::empty_for_test(); // use the crate's existing test constructor
    e.clipboard_provider_dirty = false;
    e.set_clipboard_provider(crate::config::ClipboardProvider::Osc52);
    assert_eq!(e.clipboard_provider, crate::config::ClipboardProvider::Osc52);
    assert!(e.clipboard_provider_dirty, "setter raises the dirty flag");
    e.clear_clipboard_provider_dirty();
    assert!(!e.clipboard_provider_dirty, "explicit clear resets it");
}
```

> Use whatever the crate's real `Editor` test constructor is (the A3 setter tests used one —
> e.g. `Editor::empty_for_test()` or similar). Match it exactly.

- [ ] **Step 2: Run to verify fail**

Run: `cargo test -p wordcartel --lib set_clipboard_provider 2>&1 | tail -20`
Expected: FAIL — field/method missing.

- [ ] **Step 3: Implement.** Add fields in the clipboard block (editor.rs:392, after `clipboard_notice_shown`):

```rust
    /// Active clipboard provider selection (seeded from `[clipboard] provider`; changed by
    /// the `clipboard_provider_*` commands). Drives `resolve_provider`.
    pub clipboard_provider: crate::config::ClipboardProvider,
    /// Set when the provider changed at runtime and the worker must rebuild its Layer-1
    /// backend; consumed (and cleared) by `drain_clipboard_intents`.
    pub clipboard_provider_dirty: bool,
```

Initialize both in the `Editor` constructor (find where `clipboard_notice_shown` is set, e.g. `clipboard_notice_shown: false,`) and add:

```rust
            clipboard_provider: crate::config::ClipboardProvider::Auto,
            clipboard_provider_dirty: false,
```

Add the setter beside `set_menu_bar_mode` (editor.rs:855):

```rust
/// Set the clipboard provider and mark the worker for a Layer-1 rebuild. The single setter
/// the `clipboard_provider_*` commands AND startup seeding call (contract law 6). Startup
/// clears the dirty flag after seeding (the worker already holds the correct initial backend).
pub fn set_clipboard_provider(&mut self, provider: crate::config::ClipboardProvider) {
    self.clipboard_provider = provider;
    self.clipboard_provider_dirty = true;
}

/// Clear the provider-dirty flag without a rebuild (startup seeding path).
pub fn clear_clipboard_provider_dirty(&mut self) {
    self.clipboard_provider_dirty = false;
}
```

- [ ] **Step 4: Run tests + clippy**

Run: `cargo test -p wordcartel --lib set_clipboard_provider 2>&1 | tail -20` → PASS.
Run: `cargo clippy -p wordcartel --all-targets 2>&1 | tail -5` → clean.

- [ ] **Step 5: Commit**

```bash
git add wordcartel/src/editor.rs
git commit -m "feat(c3): editor clipboard_provider field + shared setter"
```

---

### Task 6: Registry commands (four sets + cycle)

**Files:**
- Modify: `wordcartel/src/registry.rs` — register the five commands beside the menu-bar family (~line 500).

**Interfaces:**
- Consumes: `set_clipboard_provider` (Task 5); `MenuCategory::Settings`, `MenuMark::Value`, `register`/`register_stateful` (existing).
- Produces: commands `clipboard_provider_auto`, `clipboard_provider_native`, `clipboard_provider_osc52`, `clipboard_provider_off` (palette-only), `clipboard_provider_cycle` (menu Settings representative).

- [ ] **Step 1: Write the failing test** (registry.rs test module):

```rust
#[test]
fn clipboard_provider_commands_registered_with_correct_menu_tags() {
    let reg = Registry::builtins();
    for id in ["clipboard_provider_auto", "clipboard_provider_native",
               "clipboard_provider_osc52", "clipboard_provider_off"] {
        let e = reg.entry(id).expect(id);
        assert_eq!(e.meta.menu, None, "{id} is palette-only");
    }
    let cyc = reg.entry("clipboard_provider_cycle").expect("cycle");
    assert_eq!(cyc.meta.menu, Some(MenuCategory::Settings), "cycle is the Settings menu representative");
    assert!(cyc.meta.state.is_some(), "cycle carries state-in-label");
}
```

> Use the registry's real lookup accessor (the every-option test uses `reg.resolve_name(id)`;
> the extraction shows entries carry `meta`). If there is no `entry(id) -> &CommandEntry`
> accessor, assert via `resolve_name`/the existing meta accessor used elsewhere in registry tests.

- [ ] **Step 2: Run to verify fail**

Run: `cargo test -p wordcartel --lib clipboard_provider_commands 2>&1 | tail -20`
Expected: FAIL — commands not registered.

- [ ] **Step 3: Implement** — register beside the menu-bar family (registry.rs:500):

```rust
// Clipboard provider: set-per-state (palette-only) + 4-state cycle representative
// (Settings, state-in-label). C3 command-surface conformance.
use crate::config::ClipboardProvider;
r.register("clipboard_provider_auto",   "Clipboard: Auto",   None, |c| { c.editor.set_clipboard_provider(ClipboardProvider::Auto);   CommandResult::Handled });
r.register("clipboard_provider_native", "Clipboard: Native", None, |c| { c.editor.set_clipboard_provider(ClipboardProvider::Native); CommandResult::Handled });
r.register("clipboard_provider_osc52",  "Clipboard: OSC 52", None, |c| { c.editor.set_clipboard_provider(ClipboardProvider::Osc52);  CommandResult::Handled });
r.register("clipboard_provider_off",    "Clipboard: Off",    None, |c| { c.editor.set_clipboard_provider(ClipboardProvider::Off);    CommandResult::Handled });
r.register_stateful("clipboard_provider_cycle", "Clipboard", Some(MenuCategory::Settings),
    |e| MenuMark::Value(match e.clipboard_provider {
        ClipboardProvider::Auto => "Auto", ClipboardProvider::Native => "Native",
        ClipboardProvider::Osc52 => "OSC 52", ClipboardProvider::Off => "Off" }),
    |c| { let next = match c.editor.clipboard_provider {
              ClipboardProvider::Auto => ClipboardProvider::Native,
              ClipboardProvider::Native => ClipboardProvider::Osc52,
              ClipboardProvider::Osc52 => ClipboardProvider::Off,
              ClipboardProvider::Off => ClipboardProvider::Auto };
          c.editor.set_clipboard_provider(next); CommandResult::Handled });
```

- [ ] **Step 4: Run tests + clippy** — including the palette-completeness invariant:

Run: `cargo test -p wordcartel --lib clipboard_provider_commands 2>&1 | tail -20` → PASS.
Run: `cargo test -p wordcartel --lib palette 2>&1 | tail -10` → palette-completeness stays green (new commands auto-appear).
Run: `cargo clippy -p wordcartel --all-targets 2>&1 | tail -5` → clean.

- [ ] **Step 5: Commit**

```bash
git add wordcartel/src/registry.rs
git commit -m "feat(c3): clipboard.provider commands (4 sets + Settings cycle)"
```

---

### Task 7: Settings persistence — snapshot field + overrides round-trip

**Files:**
- Modify: `wordcartel/src/settings.rs` — `SettingsSnapshot` field (:37) + field-guard arm + law assertion (:950/:960); `OClipboard` in `OverridesFile` (:79); `parse_mask` (:215); `snapshot_of`/`runtime_snapshot` (:150/:170); `compute_overrides` (:289).

**Interfaces:**
- Consumes: `ClipboardProvider`, `clipboard_provider_str` (config); `editor.clipboard_provider`; `diff_key`/`some_if` (existing settings helpers); the `clipboard_provider_cycle`/`clipboard_provider_auto` commands (Task 6, for the law test).
- Produces: `SettingsSnapshot.clipboard_provider`, `OClipboard`, round-trip of `[clipboard] provider` through Save Settings.

- [ ] **Step 1: Write the failing tests** (settings.rs test module):

```rust
#[test]
fn clipboard_provider_round_trips_through_overrides() {
    // A runtime value differing from baseline appears in the computed overrides.
    let baseline = snapshot_of(&crate::config::Config::default(), "tokyo-night");
    let mut runtime = baseline.clone();
    runtime.clipboard_provider = crate::config::ClipboardProvider::Osc52;
    let ov = compute_overrides(&runtime, &baseline, &OverridesFile::default(), &OverridesFile::default());
    assert_eq!(ov.clipboard.and_then(|c| c.provider).as_deref(), Some("osc52"));
}

#[test]
fn clipboard_provider_matching_baseline_is_omitted() {
    let baseline = snapshot_of(&crate::config::Config::default(), "tokyo-night");
    let runtime = baseline.clone(); // provider == Auto == default
    let ov = compute_overrides(&runtime, &baseline, &OverridesFile::default(), &OverridesFile::default());
    assert!(ov.clipboard.is_none(), "unchanged provider writes no [clipboard] section");
}
```

> Match the real `compute_overrides` signature from the extraction — it takes `(runtime, baseline,
> existing, mask)`. If arg names/order differ, use the exact real signature.

- [ ] **Step 2: Run to verify fail**

Run: `cargo test -p wordcartel --lib clipboard_provider_round 2>&1 | tail -20`
Expected: FAIL — `SettingsSnapshot.clipboard_provider` / `OClipboard` missing (may be a compile error, which counts as failing).

- [ ] **Step 3: Implement.**

Add the snapshot field (settings.rs:55, after `canvas`):

```rust
    /// Clipboard provider selection persisted as "auto"/"native"/"osc52"/"off".
    pub clipboard_provider: crate::config::ClipboardProvider,
```

Add the override section struct + field (settings.rs:79 area, beside `OMouse`):

```rust
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OClipboard {
    #[serde(skip_serializing_if = "Option::is_none")] pub provider: Option<String>,
}
```

Add to `OverridesFile`:

```rust
    #[serde(skip_serializing_if = "Option::is_none")] pub clipboard: Option<OClipboard>,
```

In `parse_mask`'s local `MaskFile` add `clipboard: Option<OClipboard>` and thread it into the returned `OverridesFile { …, clipboard: mask.clipboard }` (provider is a plain per-key predicate — no provenance collapse like theme).

In `snapshot_of` add: `clipboard_provider: cfg.clipboard.provider,`.
In `runtime_snapshot` add: `clipboard_provider: editor.clipboard_provider,`.

In `compute_overrides`, beside the menu section:

```rust
// --- clipboard — per-key mask predicate ---
let rt_cp   = crate::config::clipboard_provider_str(runtime.clipboard_provider).to_string();
let base_cp = crate::config::clipboard_provider_str(baseline.clipboard_provider).to_string();
let provider = diff_key(
    &rt_cp, &base_cp,
    existing.clipboard.as_ref().and_then(|c| c.provider.as_ref()),
    mask.clipboard.as_ref().and_then(|c| c.provider.as_ref()).is_some(),
);
let has_provider = provider.is_some();
let clipboard = some_if(OClipboard { provider }, has_provider);
```

…and add `clipboard` to the final `OverridesFile { … }` the function returns.

Add the field-guard arm (settings.rs:950 destructure) and the law assertion (settings.rs:960):

```rust
            // …existing fields…
            chrome_disposition: _, canvas: _, clipboard_provider: _,
        } = s;
```
```rust
    assert!(has("clipboard_provider_cycle") && has("clipboard_provider_auto"), "clipboard_provider");
```

- [ ] **Step 4: Run tests + clippy** (the every-option law test must stay green):

Run: `cargo test -p wordcartel --lib clipboard_provider_round 2>&1 | tail -20` → PASS.
Run: `cargo test -p wordcartel --lib every_persisted_setting_has_a_command 2>&1 | tail -10` → PASS.
Run: `cargo clippy -p wordcartel --all-targets 2>&1 | tail -5` → clean.

- [ ] **Step 5: Commit**

```bash
git add wordcartel/src/settings.rs
git commit -m "feat(c3): persist clipboard.provider through Save Settings overrides"
```

---

### Task 8: Worker integration — `SelectProvider` + initial plan

**Files:**
- Modify: `wordcartel/src/clipboard.rs` — `ClipReq::SelectProvider`, `spawn_worker(msg_tx, initial: ProviderPlan)`, worker loop rebuild + plan-based availability.

**Interfaces:**
- Consumes: `ProviderPlan`, `Layer1Choice` (Task 1), `backend_for` (Task 3), `Msg::ClipboardAvailability` (existing).
- Produces: `ClipReq::SelectProvider(Layer1Choice)` variant; new `spawn_worker` signature.

- [ ] **Step 1: Write the failing tests** (clipboard.rs tests):

```rust
#[test]
fn spawn_worker_reports_available_for_null_plus_osc52() {
    // plain-SSH plan: Null layer1 but OSC 52 enabled → available == true.
    let (tx, rx) = std::sync::mpsc::channel::<crate::app::Msg>();
    let plan = ProviderPlan { layer1: Layer1Choice::Null, osc52: Some(Osc52Wrap::Bare) };
    let clip = spawn_worker(tx, plan);
    match rx.recv().expect("availability msg") {
        crate::app::Msg::ClipboardAvailability(a) => assert!(a, "Null+OSC52 is available"),
        other => panic!("expected availability, got {other:?}"),
    }
    let _ = clip.send(ClipReq::Shutdown);
}

#[test]
fn spawn_worker_reports_unavailable_for_null_no_osc52() {
    let (tx, rx) = std::sync::mpsc::channel::<crate::app::Msg>();
    let plan = ProviderPlan { layer1: Layer1Choice::Null, osc52: None };
    let clip = spawn_worker(tx, plan);
    match rx.recv().expect("availability msg") {
        crate::app::Msg::ClipboardAvailability(a) => assert!(!a, "Null+no-OSC52 is unavailable"),
        other => panic!("expected availability, got {other:?}"),
    }
    let _ = clip.send(ClipReq::Shutdown);
}

#[test]
fn select_provider_rebuilds_backend_to_null_then_get_is_none() {
    let (tx, rx) = std::sync::mpsc::channel::<crate::app::Msg>();
    // start with a Null owner (deterministic, no real system clipboard dependency).
    let clip = spawn_worker(tx, ProviderPlan { layer1: Layer1Choice::Null, osc52: None });
    let _ = rx.recv(); // availability
    let _ = clip.send(ClipReq::SelectProvider(Layer1Choice::Null));
    clip.send(ClipReq::Get { id: 7, buffer_id: crate::editor::BufferId(0) }).unwrap();
    match rx.recv().expect("paste msg") {
        crate::app::Msg::ClipboardPaste { id, text, .. } => { assert_eq!(id, 7); assert_eq!(text, None); }
        other => panic!("expected paste, got {other:?}"),
    }
    let _ = clip.send(ClipReq::Shutdown);
}
```

> Use the crate's real `BufferId` constructor (the extraction shows `crate::editor::BufferId`; match
> its real tuple/newtype form).

- [ ] **Step 2: Run to verify fail**

Run: `cargo test -p wordcartel --lib spawn_worker 2>&1 | tail -20`
Expected: FAIL — `spawn_worker` arity / `SelectProvider` variant.

- [ ] **Step 3: Implement.** Add the `ClipReq` variant (clipboard.rs:19):

```rust
pub enum ClipReq {
    Set(String),
    Get { id: u64, buffer_id: crate::editor::BufferId },
    SelectProvider(Layer1Choice),
    Shutdown,
}
```

Replace `spawn_worker` (clipboard.rs:62–92):

```rust
/// Spawn the long-lived clipboard worker with an initial resolved plan. The Layer-1
/// backend is built from `initial.layer1`; availability reflects the whole plan
/// (`layer1 != Null || osc52.is_some()`), so a plain-SSH plan (Null + OSC 52) reports
/// available. `SelectProvider` rebuilds the backend live on a runtime provider change.
pub fn spawn_worker(msg_tx: Sender<crate::app::Msg>, initial: ProviderPlan) -> Sender<ClipReq> {
    let (tx, rx) = std::sync::mpsc::channel::<ClipReq>();
    std::thread::Builder::new()
        .name("wcartel-clipboard".into())
        .spawn(move || {
            let available = initial.layer1 != Layer1Choice::Null || initial.osc52.is_some();
            let _ = msg_tx.send(crate::app::Msg::ClipboardAvailability(available));
            let mut backend: Box<dyn ClipboardBackend> = backend_for(initial.layer1);
            while let Ok(req) = rx.recv() {
                match req {
                    ClipReq::Set(s) => backend.set(&s),
                    ClipReq::Get { id, buffer_id } => {
                        let text = backend.get().filter(|s| !s.is_empty());
                        let _ = msg_tx.send(crate::app::Msg::ClipboardPaste { id, buffer_id, text });
                    }
                    ClipReq::SelectProvider(choice) => { backend = backend_for(choice); }
                    ClipReq::Shutdown => break,
                }
            }
        })
        .expect("spawn clipboard worker");
    tx
}
```

- [ ] **Step 4: Run tests + clippy**

Run: `cargo test -p wordcartel --lib spawn_worker 2>&1 | tail -20` → PASS.
Run: `cargo test -p wordcartel --lib select_provider 2>&1 | tail -10` → PASS.
Run: `cargo clippy -p wordcartel --all-targets 2>&1 | tail -5` → clean.

> **Expected breakage:** `spawn_worker`'s call site (app.rs:1506) now fails to compile (arity). That is
> fixed in Task 9. To keep THIS task's commit green in isolation, temporarily update the call site to
> `spawn_worker(msg_tx.clone(), crate::clipboard::ProviderPlan { layer1: crate::clipboard::Layer1Choice::Null, osc52: None })`
> — Task 9 replaces it with the real startup-resolved plan.

- [ ] **Step 5: Commit**

```bash
git add wordcartel/src/clipboard.rs wordcartel/src/app.rs
git commit -m "feat(c3): worker takes initial ProviderPlan + SelectProvider rebuild"
```

---

### Task 9: Drain + startup wiring (the live path)

**Files:**
- Modify: `wordcartel/src/clipboard.rs` — `drain_clipboard_intents` recompute + ordered send (:34); a cached `ClipEnv` plumbed in.
- Modify: `wordcartel/src/app.rs` — startup resolves the initial `ProviderPlan`, passes it to `spawn_worker` (:1506), seeds the setter + clears dirty (near the A3 seeding :1378), and passes the cached env + editor to the drain (:1664).

**Interfaces:**
- Consumes: `resolve_provider`, `clip_env_from_process`, `ProviderPlan`, `Osc52Wrap`, `ClipReq::SelectProvider`, `osc52_set(text, wrap)` (Tasks 1/2/8); `set_clipboard_provider`/`clear_clipboard_provider_dirty` (Task 5).
- Produces: the fully wired live behavior (provider changes rebuild the worker; OSC 52 emits with the plan's wrap only when the plan says so; provider switch takes effect before same-frame copy/paste).

- [ ] **Step 1: Write the failing tests** (clipboard.rs tests — the drain already has fake-channel tests to mirror):

```rust
#[test]
fn drain_emits_wrapped_osc52_when_plan_says_so() {
    // Env: tmux + Null layer1 → plan.osc52 == Some(Tmux). A copy emits the tmux-wrapped bytes.
    let mut e = crate::editor::Editor::empty_for_test();
    e.clipboard_provider = crate::config::ClipboardProvider::Osc52; // forces Null + wrap
    e.clipboard_sync_request = Some("hi".into());
    let env = ClipEnv { tmux: true, screen: false, ssh: false, wayland: false, x11: false,
                        wsl: false, os: Os::Linux, present: |_| false };
    let (clip_tx, _clip_rx) = std::sync::mpsc::channel();
    let (msg_tx, _msg_rx) = std::sync::mpsc::channel();
    let mut out: Vec<u8> = Vec::new();
    drain_clipboard_intents(&mut e, &env, &mut out, &clip_tx, &msg_tx);
    assert_eq!(out, b"\x1bPtmux;\x1b\x1b]52;c;aGk=\x1b\x1b\\\x1b\\".to_vec());
}

#[test]
fn drain_suppresses_osc52_when_plan_none() {
    // Native forces layer1 Arboard, osc52 None → no terminal write.
    let mut e = crate::editor::Editor::empty_for_test();
    e.clipboard_provider = crate::config::ClipboardProvider::Native;
    e.clipboard_sync_request = Some("hi".into());
    let env = ClipEnv { tmux: false, screen: false, ssh: false, wayland: false, x11: true,
                        wsl: false, os: Os::Linux, present: |_| false };
    let (clip_tx, _clip_rx) = std::sync::mpsc::channel();
    let (msg_tx, _msg_rx) = std::sync::mpsc::channel();
    let mut out: Vec<u8> = Vec::new();
    drain_clipboard_intents(&mut e, &env, &mut out, &clip_tx, &msg_tx);
    assert!(out.is_empty(), "osc52 suppressed → nothing written to the terminal");
}

#[test]
fn drain_sends_select_provider_before_set_when_dirty() {
    let mut e = crate::editor::Editor::empty_for_test();
    e.clipboard_provider = crate::config::ClipboardProvider::Native;
    e.clipboard_provider_dirty = true;
    e.clipboard_sync_request = Some("hi".into());
    let env = ClipEnv { tmux: false, screen: false, ssh: false, wayland: false, x11: true,
                        wsl: false, os: Os::Linux, present: |_| false };
    let (clip_tx, clip_rx) = std::sync::mpsc::channel();
    let (msg_tx, _msg_rx) = std::sync::mpsc::channel();
    let mut out: Vec<u8> = Vec::new();
    drain_clipboard_intents(&mut e, &env, &mut out, &clip_tx, &msg_tx);
    // First message is the provider rebuild, THEN the Set.
    match clip_rx.recv().unwrap() { ClipReq::SelectProvider(_) => {}, o => panic!("want SelectProvider first, got {o:?}") }
    match clip_rx.recv().unwrap() { ClipReq::Set(s) => assert_eq!(s, "hi"), o => panic!("want Set, got {o:?}") }
    assert!(!e.clipboard_provider_dirty, "dirty cleared after send");
}
```

- [ ] **Step 2: Run to verify fail**

Run: `cargo test -p wordcartel --lib drain_ 2>&1 | tail -20`
Expected: FAIL — `drain_clipboard_intents` arity (now takes `&ClipEnv`).

- [ ] **Step 3: Implement.** Change `drain_clipboard_intents` (clipboard.rs:34) to take the cached env and do the ordered work:

```rust
/// Called by run() after reduce, before the frame draw. `env` is the process clipboard
/// environment (cached at startup). Sends a provider rebuild BEFORE any queued Set/Get so a
/// same-frame provider change takes effect immediately; emits OSC 52 only when the resolved
/// plan calls for it, wrapped for the environment. Never blocks (unbounded channel).
pub fn drain_clipboard_intents(
    editor: &mut crate::editor::Editor,
    env: &ClipEnv,
    out: &mut impl std::io::Write,
    clip_tx: &Sender<ClipReq>,
    msg_tx: &Sender<crate::app::Msg>,
) {
    let plan = resolve_provider(env, editor.clipboard_provider);

    // Runtime provider change → rebuild the worker's Layer-1 backend FIRST.
    if editor.clipboard_provider_dirty {
        let _ = clip_tx.send(ClipReq::SelectProvider(plan.layer1));
        editor.clear_clipboard_provider_dirty();
    }

    if let Some(text) = editor.clipboard_sync_request.take() {
        if let Some(wrap) = plan.osc52 {
            if let Some(bytes) = osc52_set(&text, wrap) {
                let _ = out.write_all(&bytes);
                let _ = out.flush();
            }
        }
        if clip_tx.send(ClipReq::Set(text)).is_err() {
            editor.status = "clipboard unavailable".to_string();
        }
    }
    if let Some(pi) = editor.clipboard_get_pending.take() {
        if clip_tx.send(ClipReq::Get { id: pi.id, buffer_id: pi.buffer_id }).is_err() {
            editor.status = "clipboard unavailable".to_string();
            let _ = msg_tx.send(crate::app::Msg::ClipboardPaste {
                id: pi.id, buffer_id: pi.buffer_id, text: None,
            });
        }
    }
}
```

- [ ] **Step 4: Wire startup in app.rs.** Near the config→editor seeding (app.rs:1378, beside the A3 setters) add:

```rust
    editor.set_clipboard_provider(cfg.clipboard.provider);
    editor.clear_clipboard_provider_dirty(); // worker gets the initial plan below; no redundant rebuild
```

Cache the env and resolve the initial plan just before spawning the worker (app.rs:1506):

```rust
    let clip_env = crate::clipboard::clip_env_from_process();
    let initial_plan = crate::clipboard::resolve_provider(&clip_env, editor.clipboard_provider);
    let clip_tx = crate::clipboard::spawn_worker(msg_tx.clone(), initial_plan);
```

Update the drain call (app.rs:1664) to pass the cached env:

```rust
    crate::clipboard::drain_clipboard_intents(&mut editor, &clip_env, guard.terminal().backend_mut(), &clip_tx, &msg_tx);
```

> `clip_env` is `Copy` (all fields are `Copy`, `present` is a `fn` pointer), so passing `&clip_env`
> each frame is free and it stays owned by the run loop.

- [ ] **Step 5: Run the full shell suite + clippy + smoke**

Run: `cargo test -p wordcartel --lib drain_ 2>&1 | tail -20` → PASS.
Run: `cargo test -p wordcartel-core -p wordcartel 2>&1 | tail -15` → all green.
Run: `cargo clippy --workspace --all-targets 2>&1 | tail -5` → clean.
Run: `bash scripts/smoke/run.sh 2>&1 | tail -3` → quote the one-line summary (mandatory-run/advisory-pass; a red S5 is advisory, not a blocker).

- [ ] **Step 6: Commit**

```bash
git add wordcartel/src/clipboard.rs wordcartel/src/app.rs
git commit -m "feat(c3): wire live provider chain — plan-driven drain + startup resolve"
```

---

## Self-review notes (author)

- **Spec coverage:** §2 layers → Tasks 1/3/8/9; §3 detection → Task 1; §4 wrapping → Task 2; §5 override+setter+persistence → Tasks 4/5/6/7; §6 backends → Task 3; §7 degradation (`Null`/status) → Tasks 8/9; §8 testing → per-task tests + Task 9 smoke; §9 file map → matches the task file lists.
- **Type consistency:** `ProviderPlan`, `Layer1Choice`, `Osc52Wrap`, `ClipEnv`, `resolve_provider`, `backend_for`, `spawn_worker(msg_tx, ProviderPlan)`, `drain_clipboard_intents(editor, &ClipEnv, out, clip_tx, msg_tx)`, `set_clipboard_provider`/`clear_clipboard_provider_dirty`, `clipboard_provider_str`, `OClipboard.provider` — used identically across tasks.
- **Anchors the implementer must confirm against real source (noted inline):** the `Config` test constructor name (Task 4), the `Editor` test constructor name (Tasks 5/8/9), the registry entry accessor (Task 6), the `BufferId` constructor form (Task 8), and the exact `compute_overrides` arg order (Task 7). Each is called out in-task.
- **Open items carried from spec §10 (documented, not silently unsolved):** GNU screen large-payload (>~768 byte) chunking is NOT implemented — screen wrap is byte-correct for typical selections only; nested tmux needs the outer tmux at `set-clipboard on`; per-terminal OSC 52 size caps below 100k may truncate. These are documented limitations, not tasks. If the screen chunking must be pinned, the implementer flags it to the human rather than guessing (per the gating rule on unverifiable claims).
