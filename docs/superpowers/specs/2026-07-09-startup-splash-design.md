# Startup splash / welcome screen — design

**Date:** 2026-07-09 · **Status:** approved (brainstorm); **Codex spec review GO (round 3, 2026-07-09)** — ready for
implementation plan. · **Pipeline:** full gated (this touches the command surface) — design → Codex spec review ✓ →
plan → Codex plan review → subagent execution → Fable + Codex gates → merge. **Origin:** user request "create a
splash screen."

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

`run()` sets `editor.splash = Some(Splash::new(&keymap, env!("CARGO_PKG_VERSION")))` (see §3 for construction)
after the recovery-on-open block and the `std::mem::take` of the keymap, and before the first
`first_frame_settle` + draw (`app.rs` ~ lines 613–699).

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
- **Hints** (3): `palette` "Command palette", `open` "Open file", `quit` "Quit". Each chord comes from
  `keymap::KeyTrie::chord_for(CommandId) -> Option<String>` (`keymap.rs:194`; returns `None` when unbound — blank
  display is a caller convention, cf. `palette.rs:81` `unwrap_or_default()`), the existing "hints track the active
  keymap" machinery, so chords are honest under CUA *or* WordStar. **A hint whose `chord_for` is `None` is omitted**
  (no dangling label). **Resolution timing (Codex-Critical fix):** `render_overlays::paint(frame, editor, cs)` has
  **no keymap** in its signature, and `run()` moves the keymap out of `editor` via `std::mem::take` (`app.rs:613`)
  into a loop-local before the first draw — so hints CANNOT be resolved at paint time. Instead they are resolved
  **once, at `Splash` construction** (§3), from the loop-local keymap, and the resolved `(chord, label)` pairs are
  stored in the `Splash`. This is correct and honest: the splash is startup-only and dismissed on the first key, so
  the active keymap cannot change while it is shown — one-shot resolution == active-keymap resolution here.
- **Layout:** vertically + horizontally centered block; reuse `render()`'s tiny-terminal discipline. **Degradation**
  as height shrinks: drop hints → tagline → version, always keeping the wordmark; below the existing `w<4 || h<2`
  guard, paint nothing. Never panic on any size (assert in tests).
- **Canvas:** paints over the full frame including the status row (a splash owns the screen). Fill respects
  `CanvasMode` like the rest of render (opaque base_bg fill under fg-only text).

### 3. Architecture — the overlay seam (mirrors palette/menu/theme_picker)

- **State:** new field `editor.splash: Option<Splash>` (`editor.rs`, alongside `palette`/`menu`/… ~ lines 411–445).
- **Module:** new `wordcartel/src/splash.rs` — the `Splash` struct + `Splash::new(keymap, version)` (resolves the 3
  hint chords via `chord_for` at construction and stores the surviving `(chord, label)` pairs; `version` captured
  from `env!("CARGO_PKG_VERSION")`), plus `intercept` and the paint helper. **`Splash` carries its resolved content
  so paint needs no keymap** (the Critical fix). Wordmark/tagline/footer are static; theme faces are read from
  `editor.theme` at paint.
- **Interception:** `splash::intercept(msg, editor, …) -> Handled` inserted at the **TOP** of `reduce`'s chain,
  BEFORE `marks::intercept` (`app.rs:233`). Signature matches the peer intercepts (the `(msg, editor, ex, clock,
  msg_tx)` shape — splash needs no `reg`/`keymap`). Contract: `splash.is_none()` → `Pass(msg)`; else key/mouse-down →
  `editor.splash = None; Done(!editor.quit)`; any other msg → `Pass(msg)`. (One added stage in `reduce`; ties to the
  H10 `watch` note — the chain grows by one fixed stage, still bounded.)
- **Paint:** a branch in `render_overlays::paint(frame, editor, cs)` (`render_overlays.rs:37`, verified the last
  paint step in `render()` at `render.rs:736`), drawing the full-frame splash from the pre-resolved `Splash` content
  when `editor.splash.is_some()`.
- **Startup (`run()`):** set `editor.splash = Some(Splash::new(&keymap, env!("CARGO_PKG_VERSION")))` using the
  **loop-local `keymap`** (after the `std::mem::take` at `app.rs:613`, before `first_frame_settle`/first draw at
  `app.rs:698`–`699`), gated on `cfg.view.splash && !cli.no_splash && editor.prompt.is_none()` (recovery-on-open sets
  `editor.prompt` via `open_prompt`, verified `app.rs:551/568/575` → `editor.rs:656`; no other startup prompt precedes
  the first draw).
- **Config:** add `pub splash: bool` to `ViewConfig` (`config.rs:125`), default `true` (a plain bool like
  `word_count`/`wrap_guide`). Parsing is **serde `RawView` + manual folding into `ViewConfig`**, NOT direct serde —
  add the field to `RawView` and its fold site (`config.rs:307` and the fold at `config.rs:400`). Startup seed is
  already covered by `editor.view_opts = cfg.view.clone()` (`app.rs:484`) — no new seeding code.
- **CLI:** add `pub no_splash: bool` to `Cli` (`config.rs:7`) + a `"--no-splash"` arm in `parse_cli` (`config.rs:22`,
  mirrors `--no-config`). `run()` gates the splash on `!cli.no_splash`.
- **Commands (set-per-state, Codex-Important fix):** the contract treats a settable option as needing **deterministic
  set-per-state primitives + a convenience toggle + a stateful menu representative** (the `status_line_on` /
  `status_line_auto` / `toggle_status_line` pattern, `registry.rs:489` — NOT the bare `toggle_word_count`, which has
  no setter and is the weaker pattern). So register: **`splash_on`** and **`splash_off`** as **palette-only**
  (`menu: None`, deterministic — matching `status_line_on`/`status_line_auto` at `registry.rs:489`–`490`/`492`), and
  **`toggle_splash`** as the **stateful View-menu representative** (`MenuCategory::View`,
  `MenuMark::OnOff(view_opts.splash)`). The contract requires exactly this shape — set-per-state primitives are
  `menu: None`, one stateful representative carries the menu (`command-surface-contract.md:56`). All three call **one
  shared setter** `Editor::set_splash(bool)` (mirrors `set_scrollbar_mode`/`set_status_line_mode`, `editor.rs:838`/
  `847`; commands call the setter as at `registry.rs:490`). The startup `view_opts.clone()` seed (`app.rs:484`) is the
  established seed path, left as-is. Status notes it affects the **next** launch, e.g. `splash: on (takes effect next launch)`.
- **Settings persistence (Codex-Important fix — config parse alone does NOT persist it):** `Save Settings` writes an
  overrides file via the `settings.rs` snapshot/override machinery, so `view.splash` must be threaded through **all**
  of these sites: the `SettingsSnapshot` field itself (`settings.rs:37`) + its per-persisted-field command-test line
  (`settings.rs:33`); `OView` field (`settings.rs:110`); `snapshot_of` (`settings.rs:159`); `runtime_snapshot`
  (`settings.rs:181`); the `compute_overrides` view diff (`settings.rs:365`) **and** the final `OView` construction /
  `any_view` inclusion (`settings.rs:410`/`413`); and the command guard (`settings.rs:990`). `parse_overrides`
  /`parse_mask` need **no** bespoke field — they deserialize `OView`/`OverridesFile` directly (`settings.rs:207`
  /`226`), so adding the `OView` field covers them. Missing the `SettingsSnapshot` field or the final `OView`
  construction = persistence silently drops.

### 4. Command-surface contract conformance (App law — stated per the contract)

`docs/design/command-surface-contract.md` governs this effort because it adds a user-settable option and shows
keybinding hints:

- **Every option is a command, with set-per-state primitives:** `view.splash` is user-settable → deterministic
  `splash_on` / `splash_off` primitives **plus** the convenience `toggle_splash` (the `status_line_*` pattern,
  `registry.rs:489`), so automation/profiles/plugins can set an absolute state, not just flip.
- **Registry = single source of truth; palette exhaustive; menu ⊆ palette:** all three commands are registered → in
  the palette automatically; `toggle_splash` is the stateful **View**-menu representative (`MenuMark::OnOff`).
- **One shared setter:** `Editor::set_splash(bool)` — the single write path all three commands (and profiles) call.
- **Hints track the active keymap:** the splash's hint chords come from `KeyTrie::chord_for` against the keymap
  active at construction (startup); the splash never outlives a preset change, so one-shot resolution is faithful.
- **Merge gates:** the contract's invariant tests (palette-completeness, every-option-has-a-command, hint
  re-resolution) must stay green with the new commands/option.

The splash overlay itself is dismiss-only (not a settable option), so no command beyond the three above.

### 5. Testing

- **Unit (`splash.rs`, `config.rs`, `registry.rs`, `settings.rs`):** shown iff enabled ∧ ¬`--no-splash` ∧
  ¬recovery-pending; `intercept` dismisses-and-consumes on key and on mouse-down, and `Pass`es `Tick`/background/
  `Resize`; `Splash::new` resolves the 3 hints and **omits an unbound one** (`chord_for` `None`) — asserted under
  CUA (all bound) and a WordStar/custom keymap (different or missing chord); paint renders wordmark/version/tagline
  and degrades correctly at shrinking sizes with no panic at `1×1`…`4×2`; `splash_on`/`splash_off`/`toggle_splash`
  all move `view_opts.splash` through `set_splash`; **`view.splash` round-trips through `snapshot_of` →
  `compute_overrides` → parse** (the persistence path, not just config parse).
- **e2e (`e2e.rs`):** launch → splash on first frame → key dismisses → editor visible; `--no-splash` → no splash on
  first frame; recovery-pending launch → recovery prompt shown, no splash.
- **Command-surface invariant tests:** extended/covered so `splash_on`/`splash_off`/`toggle_splash` satisfy
  palette-completeness + every-option-has-a-command + hint re-resolution, and the `SettingsSnapshot` per-persisted-
  field command-test line exists for `view.splash`.
- **Smoke (advisory):** the existing PTY suite still `8/8` (startup path changes).

### 6. Non-goals

- No ASCII-art logo (styled text only; additive later). No auto-timeout (dismiss-on-interaction only — preserves
  idle-is-free). No recent-files list on the welcome (separate feature; the hints cover orientation). No animation.
  No first-run-only logic (shows every launch until disabled).

## Codex spec-review log

- **Round 3 (2026-07-09): GO.** No Critical / Important / Minor. All round-2 fixes confirmed resolved against source
  (menu tagging vs `status_line_*`, the complete settings migration surface traced against `word_count`/`wrap_guide`,
  the §1/§3 `Splash::new` unification, `chord_for`/`RawView`/version claims). Spec is accurate enough to plan from.

- **Round 2 (2026-07-09): NOT-READY → folded.** No Critical (round-1 Critical/setter/minors all confirmed
  RESOLVED). **[Important]** set-per-state commands must be **palette-only** (`menu: None`); only `toggle_splash`
  carries the View menu (per `status_line_*` + contract:56) — was "all three View" → corrected. **[Important]**
  settings-migration list was incomplete — added the `SettingsSnapshot` field (`settings.rs:37`), the final `OView`
  construction / `any_view` (`settings.rs:410/413`), and the command guard (`settings.rs:990`); noted
  `parse_overrides`/`parse_mask` need nothing extra (deserialize `OView` directly). **[Minor]** §1 said `Splash::new()`
  while §3 said `Splash::new(&keymap, version)` → §1 corrected. Confirmed RESOLVED: hint construction ordering
  (loop-local keymap in scope post-`mem::take`), `Editor::set_splash` matches `set_scrollbar_mode`/`set_status_line_mode`,
  config `RawView`+fold edit points.

- **Round 1 (2026-07-09): NOT-READY → folded.** Codex cross-checked against source and found: **[Critical]** hints
  can't be keymap-resolved at `render_overlays::paint` (no keymap in signature; `run()` `mem::take`s the keymap
  before draw) → fixed by resolving hints at `Splash::new(keymap)` construction and storing them. **[Important]**
  `toggle_word_count` is not a shared-setter pattern (direct field flip) → adopt the `status_line_*` set-per-state
  shape + a real `Editor::set_splash`. **[Important]** persistence needs the full `settings.rs` migration surface
  (`OView`/`snapshot_of`/`runtime_snapshot`/`compute_overrides` + `SettingsSnapshot` test line), not just config
  parse → enumerated. **[Important]** contract wants deterministic set-per-state primitives → added
  `splash_on`/`splash_off`. **[Minor]** `chord_for` returns `None` not blank; config parse is `RawView`+manual fold
  → wording corrected. Confirmed-correct: recovery→`editor.prompt` suppression gate, first-draw ordering,
  `open`/`quit`/`palette` ids + CUA binds, `render_overlays::paint` is last paint step, Resize/job/timer pass-through.

## Resolved decisions (from spec review, 2026-07-09)

1. **Hints = 3, no `help`.** The registry has no `help` command, so the hint set is **palette / open / quit**
   (all real, registered commands). "Help" is dropped rather than advertising a non-existent command; a `help`
   command + hint can be an additive follow-up if a help surface is built later.
2. **Unbound hints are omitted** (not shown with a blank chord) — no dangling labels. In the default CUA keymap all
   three are bound; the omit rule only bites under a custom keymap that unbinds one.
