# Startup splash / welcome screen — design

**Date:** 2026-07-09 · **Status:** approved (brainstorm) · **Pipeline:** full gated (this touches the command
surface) — design → Codex spec review → plan → Codex plan review → subagent execution → Fable + Codex gates →
merge. **Origin:** user request "create a splash screen."

## Problem / goal

`wcartel` launches straight into the editor with no identity moment. Give it a **hybrid splash**: a branded
startup screen (wordmark + version + tagline) that also serves as a lightweight **welcome** when launched with no
file (a few orientation hints). It must not fight the app's top priorities — instant typing, no silent UI waits,
idle is free — and it must be dismissible and turn-off-able.

## Decisions (locked in brainstorm)

1. **Kind:** hybrid — branded at startup, doubles as welcome when no file is given.
2. **Show/dismiss:** shows on every launch (when enabled); dismissed by the **first key press or mouse click**,
   which is **consumed** (discarded). No auto-timeout (would force an idle wake — rejected to preserve idle-is-free).
3. **Content:** styled-text (NOT ASCII art) wordmark + version + tagline + **three** **active-keymap-resolved**
   hints — **Command palette**, **Open file**, **Quit** (commands `palette`/`open`/`quit`; "Help" dropped — no such
   command exists). An unbound hint (no chord in the active keymap) is **omitted**.
4. **Opt-out:** `view.splash` config option (default `true`), exposed as a **command** (contract-required), plus a
   `--no-splash` CLI flag for a single launch.
5. **Recovery wins:** if a swap-recovery prompt is pending at launch, the splash is **suppressed** (never bury a
   "recover your work?" prompt behind branding).

## Design

### 1. Behavior

The splash is a **full-frame overlay** painted over the first frame. It is shown iff **all** hold at launch:

- `cfg.view.splash == true` (default), AND
- `--no-splash` was NOT passed, AND
- no startup recovery prompt was opened (`editor.prompt.is_none()` after the recovery-on-open block in `run()`).

`run()` sets `editor.splash = Some(Splash::new())` after the recovery-on-open block and before the first
`first_frame_settle` + draw (`app.rs` ~ lines 555–699).

**Dismiss.** While `splash.is_some()`, the first `Msg::Input(Event::Key(press))` or `Msg::Input(Event::Mouse(down))`
clears it and is consumed. **Everything else passes through** — `Msg::Tick`, background job results
(`JobDone`/`FilterDone`/`DiagnosticsDone`/…), and `Msg::Input(Event::Resize)` — so startup warmup (pandoc/Harper),
the timer subsystems, and resize-reheal all keep working while the splash is up. This preserves idle-is-free (a
splashed idle launch blocks on the 3600 s fallback like any other).

**Hybrid.** The splash is identical with or without a file; with no file the empty scratch behind it makes it read
as the welcome. Dismissing reveals the file (if any) or the blank scratch.

### 2. Content (styled text, centered, theme-aware)

```
                     wordcartel
                       v0.1.0
             Everyone needs a cover story

         Ctrl-P   Command palette
         Ctrl-O   Open file
         Ctrl-Q   Quit

                  press any key
```

- **Wordmark** `wordcartel` — theme **accent, bold**. **Version** `v{CARGO_PKG_VERSION}` (→ `v0.1.0`), normal.
  **Tagline** "Everyone needs a cover story" — **dim**. **Footer** "press any key" — dim.
- **Hints** (3): `palette` "Command palette", `open` "Open file", `quit` "Quit". Each chord resolves against the
  **active keymap** via `keymap::KeyTrie::chord_for(CommandId)` (`keymap.rs:194`, returns the shortest chord display,
  blank when unbound) — the existing "hints track the active keymap" machinery, so chords are honest under CUA *or*
  WordStar. A hint whose command is **unbound** in the active keymap is **omitted** (no dangling label).
- **Layout:** vertically + horizontally centered block; reuse `render()`'s tiny-terminal discipline. **Degradation**
  as height shrinks: drop hints → tagline → version, always keeping the wordmark; below the existing `w<4 || h<2`
  guard, paint nothing. Never panic on any size (assert in tests).
- **Canvas:** paints over the full frame including the status row (a splash owns the screen). Fill respects
  `CanvasMode` like the rest of render (opaque base_bg fill under fg-only text).

### 3. Architecture — the overlay seam (mirrors palette/menu/theme_picker)

- **State:** new field `editor.splash: Option<Splash>` (`editor.rs`, alongside `palette`/`menu`/… ~ lines 411–445).
- **Module:** new `wordcartel/src/splash.rs` — the `Splash` struct (minimal; content is derived at paint from
  theme + keymap + version, so `Splash` holds little/no dynamic state), `intercept`, and the paint helper.
- **Interception:** `splash::intercept(msg, editor, …) -> Handled` inserted at the **TOP** of `reduce`'s chain,
  BEFORE `marks::intercept` (`app.rs:233`). Contract: `splash.is_none()` → `Pass(msg)` immediately; else key/click →
  `editor.splash = None; Done(!editor.quit)`; any other msg → `Pass(msg)`. (One added line in `reduce`; ties to the
  H10 `watch` note — the chain grows by one fixed stage, still bounded.)
- **Paint:** a branch in `render_overlays::paint(frame, editor, cs)` (`render_overlays.rs:37`, already the last
  paint step in `render()`), drawing the full-frame splash when `editor.splash.is_some()`.
- **Config:** add `pub splash: bool` to `ViewConfig` (`config.rs:118`), default `true` (a plain bool like
  `word_count`/`wrap_guide`, NOT a `TransientMode`). Add TOML parse under `[view] splash`. Seeded into
  `editor.view_opts` at startup like the other view options.
- **CLI:** add `pub no_splash: bool` to `Cli` (`config.rs:7`) + a `"--no-splash"` arm in `parse_cli` (mirrors
  `--no-config`). `run()` gates the splash on `!cli.no_splash`.
- **Command:** register `toggle_splash` (label e.g. "Startup Splash", `MenuCategory::View`, **stateful** →
  `MenuMark::OnOff(view_opts.splash)`), mirroring the existing `toggle_word_count`. It flips the **persisted**
  `view_opts.splash` through **one shared setter** (`Editor::set_splash` — the same setter startup-seed and any
  profile use); it affects the **next** launch (the splash is a startup behavior), and its status says so, e.g.
  `splash: on (takes effect next launch)`.

### 4. Command-surface contract conformance (App law — stated per the contract)

`docs/design/command-surface-contract.md` governs this effort because it adds a user-settable option and shows
keybinding hints:

- **Every option is a command:** `view.splash` is user-settable → the `toggle_splash` command IS its setter path.
- **Registry = single source of truth; palette exhaustive; menu ⊆ palette:** `toggle_splash` is registered, so it
  appears in the palette automatically and in the **View** menu.
- **One shared setter:** `Editor::set_splash` — command, startup-seed, and profiles all call it; no direct field
  writes.
- **Hints track the active keymap:** the splash's hint chords come from `KeyTrie::chord_for` against the active
  keymap; switching preset re-resolves them.
- **Merge gates:** the contract's invariant tests (palette-completeness, every-option-has-a-command, hint
  re-resolution) must stay green with the new command/option.

The splash overlay itself is dismiss-only (not a settable option), so no command beyond the toggle.

### 5. Testing

- **Unit (`splash.rs`, `config.rs`, `registry.rs`):** shown iff enabled ∧ ¬`--no-splash` ∧ ¬recovery-pending;
  `intercept` dismisses-and-consumes on key and on mouse-down, and `Pass`es `Tick`/background/`Resize`; paint
  renders wordmark/version/tagline and degrades correctly at shrinking sizes with no panic at `1×1`…`4×2`;
  `toggle_splash` flips `view_opts.splash` via `set_splash` and persists; hints resolve to different chords under
  CUA vs WordStar (`chord_for`).
- **e2e (`e2e.rs`):** launch → splash on first frame → key dismisses → editor visible; `--no-splash` → no splash on
  first frame; recovery-pending launch → recovery prompt shown, no splash.
- **Command-surface invariant tests:** extended/covered so `toggle_splash` satisfies palette-completeness +
  every-option-has-a-command + hint re-resolution.
- **Smoke (advisory):** the existing PTY suite still `8/8` (startup path changes).

### 6. Non-goals

- No ASCII-art logo (styled text only; additive later). No auto-timeout (dismiss-on-interaction only — preserves
  idle-is-free). No recent-files list on the welcome (separate feature; the hints cover orientation). No animation.
  No first-run-only logic (shows every launch until disabled).

## Resolved decisions (from spec review, 2026-07-09)

1. **Hints = 3, no `help`.** The registry has no `help` command, so the hint set is **palette / open / quit**
   (all real, registered commands). "Help" is dropped rather than advertising a non-existent command; a `help`
   command + hint can be an additive follow-up if a help surface is built later.
2. **Unbound hints are omitted** (not shown with a blank chord) — no dangling labels. In the default CUA keymap all
   three are bound; the omit rule only bites under a custom keymap that unbinds one.
