# C1 — the writing caret (B8 + B11): scoping memo

**Phase:** scoping (pre-brainstorm) · **Date:** 2026-07-13 · **Author:** Fable (design thread)
**Inputs:** `docs/design/cursor-system-concept.md`, `docs/design/cursor-system-concept-review.md`,
backlog items B8 (SM, needs-design) + B11 (S, folded in), the live tree at `main`.
**Settled (not re-litigated here):** hardware caret only (painted mode CUT 2026-07-13); DECSCUSR
shape/blink as a contract-conformant option; picker on the theme-picker model; restore on exit AND
panic; B11 folded in; B12/B13 are C2's (noted below only where code overlaps).

---

## 1. Grounding — the real code surface

All claims verified against the tree on 2026-07-13 (symbols, not line anchors, except where a line
pins a specific arm).

### 1.1 How the terminal cursor is controlled today

- **One placement site.** `wordcartel/src/render.rs::place_cursor(frame, editor, area, edit_top,
  edit_height, status_row, tg)` — render phase 12, called at `render.rs:310` from `render()`.
  Exactly three arms, all `frame.set_cursor_position(Position { .. })` (ratatui 0.30):
  1. `editor.search` open → caret on the status row at the focused field position
     (`chrome_geom::search_field_prefix_cols` is the shared painter/hit-test column source).
  2. `editor.minibuffer` open → status row at `prompt + text` char columns.
  3. otherwise → editor caret at `nav::screen_pos(editor)`, D2-clamped to `tg.text_width`.
- **Visibility is per-frame ratatui semantics:** if no `set_cursor_position` happens during a
  `draw`, the hardware cursor is hidden for that frame. Arm guards (row out of view, over-wide
  fields) already rely on this. There is no explicit Show/Hide call in the render path.
- **No DECSCUSR / `SetCursorStyle` anywhere in the tree.** `term.rs` imports only
  `crossterm::cursor::Show` (restore paths). The caret's shape/blink today is whatever the user's
  terminal defaults to. crossterm is `0.28` (`wordcartel/Cargo.toml:16`), which has
  `crossterm::cursor::SetCursorStyle` = { `DefaultUserShape`, `BlinkingBlock`, `SteadyBlock`,
  `BlinkingUnderScore`, `SteadyUnderScore`, `BlinkingBar`, `SteadyBar` } — the full DECSCUSR 0–6
  surface. **Fact that shapes the option design: DECSCUSR encodes blink INTO the shape code**
  (odd = blinking, even = steady; 0 = terminal-default). There is no independent blink escape and
  no blink-*speed* control at all.
- **The write seam already exists as a pattern.** `chrome.rs::reconcile_mouse_capture<W:
  std::io::Write>(editor, backend, applied: &mut bool)` — called once per loop iteration in
  `app.rs::run` (app.rs:863) with `guard.terminal().backend_mut()` — is an **edge-triggered,
  latch-guarded terminal-capability reconcile**: it writes escapes only when
  `editor.mouse_capture != *applied`, and updates the latch only on IO success. Unit-tested with a
  `Vec<u8>` backend. A `reconcile_cursor_style` is a structural sibling: zero writes at rest, one
  write per real change. This satisfies the idle-free / edge-triggered law by construction — a
  naive per-frame DECSCUSR write is neither needed nor acceptable.
- **Out-of-band escape precedent #2:** `clipboard.rs` writes OSC 52 bytes directly
  (`out.write_all`) via the same drain-stage-with-backend pattern (app.rs:859).

### 1.2 The theme picker — the clone target

- **State:** `theme_picker.rs::ThemePicker { query, selected, rows: Vec<String>, scroll_top,
  original: Theme, previewed: Option<String> }`, held as `Editor::theme_picker:
  Option<ThemePicker>` (editor.rs:521).
- **Summon:** registry command `"theme"` / label `Select Theme…` in `MenuCategory::View`
  (registry.rs:348) → `Editor::open_theme_picker()` (editor.rs:848), which enforces the
  overlay XOR (opening it closes palette/outline/…; tested `open_theme_picker_enforces_xor`).
- **Input:** `theme_picker.rs::intercept(msg, editor, ex, clock, msg_tx) -> Handled` — one stage
  of `reduce`'s flat intercept chain (the registration seam; adding a sibling picker = adding one
  stage, not growing a dispatcher). Handles paste-swallow, Esc, Enter, list-nav
  (`list_window::apply_list_nav`), Backspace, chars. Query editing is end-only push/pop.
- **Preview lifecycle:** every selection/query change funnels through
  `theme_cmds::preview_selected_theme(editor)` (the "single funnel") → `editor.apply_theme(...)` +
  records `tp.previewed`. **Esc** → `editor.apply_theme(tp.original)` (restore). **Enter** →
  `theme_cmds::commit_theme_picker(editor)` → sets `editor.theme_identity =
  ThemeIdentity::Builtin(n)`.
- **Persistence:** identity flows into `settings.rs::SettingsSnapshot` and out through the diff
  law when `editor.settings_save_requested` is set (`settings::perform_settings_save`, run-loop
  arm at app.rs:846).
- **Render:** `render_overlays.rs::paint` (~lines 181–230): `palette_overlay_rect`, a bordered
  box, the `> {query}` line as a plain `Paragraph`, a `list_window`-windowed row list.
  Mouse wheel/click support lives in `mouse.rs` (calls `preview_selected_theme` too).
- **Transfer note for the cursor picker:** theme preview works purely through editor state → next
  draw. Cursor-style preview additionally needs a terminal WRITE — but with the reconcile seam
  that is free: preview = set the desired-style field; the run-loop reconcile (which runs after
  every reduce, before the next block) emits the escape the same iteration. Esc-restore = set the
  field back to `original`. The funnel shape carries over unchanged.

### 1.3 The multi-state option pattern — scrollbar, traced end-to-end

The wiring C1's option(s) must mirror (scrollbar = the 3-state exemplar the registry's own
comments name as the pattern):

1. **Set-per-state primitives, palette-only:** registry.rs:633–635 — `scrollbar_off/auto/on`,
   `r.register(id, label, None, handler)`, each calling the shared setter.
2. **Stateful menu representative with state-in-label:** registry.rs:636–642 —
   `r.register_stateful("cycle_scrollbar", "Scrollbar", Some(MenuCategory::View), |e|
   MenuMark::Value(..), handler)` cycling Off→Auto→On.
3. **One shared setter:** `Editor::set_scrollbar_mode(mode)` (editor.rs:949) — called by the
   commands, by the density profile bundle (`density.rs:65`), and by startup config apply
   (`app.rs:535`). Law 6.
4. **Config field:** `config.rs::ViewConfig.scrollbar: TransientMode` (ViewConfig at
   config.rs:154).
5. **Persistence:** `settings.rs::SettingsSnapshot.view_scrollbar` (settings.rs:46) +
   `OView.scrollbar: Option<String>` overrides key (settings.rs:120) + snapshot-from-config
   (settings.rs:174) + snapshot-from-editor (settings.rs:196) + a diff-law entry
   (settings.rs:404–408).
6. **LAW-2 gate:** `settings.rs::every_persisted_setting_has_a_command` (settings.rs:1003)
   **exhaustively destructures the snapshot** (settings.rs:1010) — a new field fails the test
   until it is both destructured and command-mapped (settings.rs:1028). This is the recurrence
   guard C1's new fields will trip on purpose.
7. **Hint re-resolution:** LAW-7 test `keymap.rs::hints_reresolve_on_preset_switch`
   (keymap.rs:1136). C1 likely ships no default chord, so this is inherited-for-free, but any
   binding added must respect it.

### 1.4 Exit + panic restore — exactly three sites

All in `wordcartel/src/term.rs`; each currently ends `execute!(io::stdout(), DisableMouseCapture,
DisableBracketedPaste, LeaveAlternateScreen, Show)`:

1. **`TerminalGuard::Drop`** (term.rs:65–70) — the clean-exit path (RAII, also covers `?` early
   returns from `run`).
2. **`TerminalGuard::new` error rollback** (term.rs:50–54) — setup failure after raw mode.
3. **`install_panic_hook`'s hook body** (term.rs:96–120) — main-thread-gated (M4), runs
   `recovery::dump_on_panic()` then the restore sequence, then chains the previous hook.

A caret-style restore (`SetCursorStyle::DefaultUserShape`, DECSCUSR 0) is one added element in
each of these three `execute!` chains. Honesty caveat: DECSCUSR cannot be *queried*, so we can
only restore to "terminal default," not to whatever style a wrapper shell had set — the same
compromise Helix/Neovim accept. If C1 adopts an "unmanaged" state where we never write DECSCUSR
(Fork 3), an unconditional restore in `Drop`/panic is still harmless — but a latch-aware
"restore only if we ever wrote" is trivially available since the applied-latch exists anyway.
The PTY smoke suite's S7 (panic → restore) is the natural advisory eyeball for this.

### 1.5 B11 — the modal caret wart, precisely

- `place_cursor` arm 3 has **no overlay gating**: while any centered overlay is open, the
  hardware caret stays parked at the *editor* caret's text-area cell, blinking through the modal.
- Four overlays render a `> {query}` line as a plain `Paragraph` and never call
  `set_cursor_position`: **palette** (render_overlays.rs ~82–87), **outline** (~145–150),
  **theme picker** (~206–211), **file browser** (~268–273). Their query fields look caret-less.
- **Cursor positions within the query:** the palette has a real mid-string cursor
  (`palette.rs:23 cursor: usize`, with left/right/insert editing); outline / theme-picker /
  file-browser query edits are end-only push/pop → caret is always at end-of-query.
- **Fix-site geometry:** `render_overlays::paint` runs at render.rs:312, AFTER `place_cursor`
  (render.rs:310), and ratatui's last `set_cursor_position` in a frame wins. So each overlay can
  place its own caret (it owns `query_area` locally) and it will override arm 3 — but arm 3 must
  ALSO be gated for the **caret-less input owners**, where the correct behavior is a *hidden*
  caret (= simply don't place): `menu` (MenuView), `prompt` (y/n modal — message on the status
  row, no text field), `splash`, `diag` overlay. Search and minibuffer already place correctly.
- Full census of input-owning surfaces on `Editor`: `search`✓, `minibuffer`✓, `palette`✗,
  `outline`✗, `theme_picker`✗, `file_browser`✗, `menu`(hide), `prompt`(hide), `splash`(hide),
  `diag`(hide). ✓ = caret already correct; ✗ = needs a query-field caret.
- **B12/B13 overlap check:** none. B12/B13 live in `row_spans_placed`/theme faces (painted
  cells); C1 touches `place_cursor`/`render_overlays`/term lifecycle. No shared code beyond
  `render.rs` the file. Note only: with C1's hardware caret unchanged, the caret will natively
  overlay B13's future styled boundary cells — the concept's collision rule resolves itself.

### 1.6 Anti-regrowth seams and budgets

- `wordcartel/tests/module_budgets.rs` hub budgets: `app.rs` 1000, `render.rs` 900, `timers.rs`
  400, `plugin/host.rs` 400, `plugin/pump.rs` 350. `clippy::too_many_lines` threshold 100.
- C1's landing zones respect the seams: registry rows are **data-table rows** (the sanctioned
  growth spot); the picker is a **new module** (`cursor_picker.rs`, cloned from
  `theme_picker.rs`) registering one intercept stage; the reconcile is a sibling fn in
  `chrome.rs` (or a small `cursor_style.rs` module — spec decision); term.rs edits are three
  one-line chain extensions; `place_cursor` gains a guard + the overlays gain local placements
  (bounded, non-dispatcher code). `app.rs` grows by ~2 lines (one reconcile call + one latch
  local) against a 1000-line budget — must be checked at plan time but is not at risk.

---

## 2. Scope

### 2.1 IN

1. **Desired-style state + one shared setter** on `Editor` (shape / blink; exact decomposition =
   Fork 2), mirroring `set_scrollbar_mode`.
2. **Config + persistence:** new `ViewConfig`-or-new-section fields, `SettingsSnapshot` fields,
   `OverridesFile` keys, diff-law entries; trips and satisfies the LAW-2 exhaustive destructure.
3. **Commands:** set-per-state primitives (palette-only) + stateful menu representative(s) with
   state-in-label + the picker-open command (`Caret…`-style row in View, beside `Select Theme…`).
4. **`reconcile_cursor_style<W: Write>`** — edge-triggered, latch-guarded, one run-loop call
   site; `Vec<u8>`-backend unit tests (mirrors `reconcile_mouse_capture`).
5. **Cursor picker** cloned from `theme_picker.rs`: XOR summon, list rows, live preview through
   the desired-state funnel, Esc-restore, Enter-commit, mouse support, windowing.
6. **Restore on exit + panic:** `SetCursorStyle::DefaultUserShape` in the three term.rs sites.
7. **B11:** query-field carets in the four ✗ overlays + arm-3 gating (hide) for
   menu/prompt/splash/diag.
8. **Tests:** LAW-2 + palette-completeness (automatic once registered), cycle/set command tests,
   reconcile latch tests, `TestBackend::cursor_position` assertions for every input-owning
   surface (the render.rs:846 precedent), an e2e journey (open picker → preview → Esc → shape
   unchanged; → Enter → persisted), and a no-write-at-rest guardrail (reconcile called twice,
   second emits zero bytes).

### 2.2 OUT (with reasons)

- **Painted cursor / brightness / theme-colored caret** — CUT by decision 2026-07-13.
- **Blink speed** — does not exist in DECSCUSR; would require a painted caret. Out, permanently
  on this path.
- **OSC 12 caret color** — discouraged in the review (compat matrix + theme-stance conflict);
  stays out.
- **B12/B13 marker visuals** — C2's; disjoint code (§1.5).
- **Idle caret dimming (§6 census)** — painted-path machinery; out with the cut.
- **Focus events / unfocused-pane looks** — no panes; terminal natively hollows the hardware
  caret on focus loss. Nothing to build.
- **Per-context style switching machinery beyond the Fork-1 resolution** — see Fork 1; the lean
  keeps DECSCUSR churn out of modal open/close.
- **Typewriter / focus / measure / marked-block interaction** — verified no interaction:
  those are scroll/paint behaviors; the hardware caret is positioned identically under all of
  them, and natively overlays any painted cell. N/A, stated for the record.

### 2.3 Size verdict: **SM — confirmed**

Every mechanism has a shipped template: the escape write (`reconcile_mouse_capture`), the picker
(`theme_picker.rs`, ~215 lines incl. tests), the option wiring (scrollbar's 7-step trace), the
restore (three one-line chain edits). Zero `wordcartel-core` changes; zero layout/hot-path
changes. The only genuinely novel design is the desired/applied latch semantics (small) and the
B11 sweep, which is enumerable (four placements + four gates). It is not S because of the
contract surface area (fields × keys × diff-law × commands × tests) and the ten-surface caret
census, each cheap but each mandatory.

### 2.4 Risk

- **Hot path:** none. The reconcile is post-reduce, latch-guarded; DECSCUSR is written only on
  real change. Idle stays free (no new timers, native blink). The guardrail test in 2.1(8) pins
  this.
- **Terminal compat:** DECSCUSR is honored by all mainstream terminals (xterm, VTE, Konsole,
  Alacritty, kitty, WezTerm, foot, Windows Terminal); unsupported terminals ignore it silently —
  a graceful no-op with zero detection possible or needed (bracketed-paste precedent). tmux
  translates via `Ss`/`Se` terminfo — modern tmux passes it through; some outer-terminal configs
  need `terminal-overrides`, which is the user's tmux config, not ours. Blink honoring varies
  (some desktops force-disable cursor blink globally) — best-effort, document it.
- **Restore honesty:** only `DefaultUserShape` is restorable (§1.4). Accepted industry-wide.
- **Testability:** DECSCUSR bytes bypass the ratatui buffer, so e2e `TestBackend` journeys can't
  see them — covered instead by `Vec<u8>` reconcile unit tests + the S7 smoke advisory eyeball.

### 2.5 Command-surface-contract obligations (concrete)

- **Law 2:** every new persisted field (shape, blink) gets commands; the LAW-2 destructure test
  enforces.
- **Law 3:** all new commands appear in the palette (automatic via registration); **the picker
  must not be the only door** to any state — the set-per-state primitives guarantee it.
- **Law 4:** menu gets the curated subset: the cycle representative(s) + the picker-open row.
- **Law 6:** one setter; startup config apply and any future profile call the same setter.
- **Rule 8:** multi-state option = set-per-state primitives (`menu: None`) + stateful
  representative with state-in-label (`MenuMark::Value`/`OnOff`).
- **Law 7:** hints re-resolve — inherited; no new default chords planned (palette/menu access).
- Spec and plan will each carry an explicit conformance statement (this memo is the draft of it).

### 2.6 Dependencies / ordering

- **None blocking.** No dependency on C2/B13 (disjoint), on Effort P (commands are nullary,
  P-compatible per rule 10), or on any open item. B10 (EOF caret clamp) touches
  `nav::caret_line`, adjacent to but not overlapping `place_cursor` — no conflict.
- C1 **absorbs B8 and closes B11**; backlog bookkeeping at ship time: mark both shipped, move
  prose sections to the archive.

---

## 3. Brainstorming forks (ordered; one at a time in session)

### Fork 1 — the "state axis": which caret states get distinct shapes?

The backlog phrase "set-per-state" means per option-*value* primitives (`caret_block`,
`caret_beam`, …), NOT per editor-state styling — worth stating up front. The real question:
does the caret change shape by *context*?

- **A. One global style.** The configured shape/blink applies everywhere the caret appears —
  text area, search bar, minibuffer, overlay query fields. DECSCUSR is written on option change
  only; modal open/close writes nothing.
- **B. Two contexts: editing vs. field-input.** E.g. block in prose, bar in query/search/
  minibuffer fields. Requires a DECSCUSR write on every modal open/close and two option
  surfaces (or a fixed built-in field style).
- **C. Ship A, structure for B.** Single style now; the desired-style state is one field, but
  the reconcile reads a `fn desired_caret_style(&Editor)` derivation so a context map can slot
  in later without rewiring.

**Recommendation: A** (with C's derivation-fn shape as free implementation hygiene, not a
promise). This is a modeless editor; the concept itself settles "the modal's input field uses
the configured writing-cursor style" — which is exactly A's behavior at zero cost. B doubles the
option surface and adds escape churn for a distinction no one asked for.

### Fork 2 — option decomposition: one 6-state style or shape × blink?

- **A. Two orthogonal options:** `caret_shape` ∈ {block, beam, underline, (+default — Fork 3)}
  and `caret_blink` ∈ {on, off}. Commands: 3–4 shape sets + 2 blink sets (palette-only), a
  `cycle_caret_shape` (Value label) + `toggle_caret_blink` (OnOff) in the menu. Two config keys.
- **B. One combined 6-state option** (block/beam/underline × steady/blinking), one cycle.
  Matches DECSCUSR's encoding literally, but a 6-state cycle is a bad menu representative and
  the config value is uglier (`"blinking-beam"`).

**Recommendation: A.** DECSCUSR's combined encoding is a wire format, not a UX; the two-option
shape mirrors how every editor (VS Code, Helix, kitty) exposes it, cycles stay short, and the
reconcile trivially composes the pair into the right escape code.

### Fork 3 — an "unmanaged / terminal default" state?

- **A. Yes — and it is the shipped default.** `caret_shape = default` means we never emit
  DECSCUSR (and with the latch, never wrote → teardown restore optional). Users who ignore the
  feature keep today's exact behavior; users who set a shape get restore-on-exit.
- **B. No — always manage,** shipping some concrete default (blinking block?), always restoring
  on exit.

**Recommendation: A.** The app has never touched caret style; silently starting to (B) changes
every user's caret on upgrade and violates least surprise. A also gives a clean escape hatch on
weird terminals. (Blink under `default` shape: the blink option only applies when a shape is
managed — the spec must say so.)

### Fork 4 — default shape/blink for the managed states (what the picker highlights / cycle
enters first)

Only meaningful once a user *leaves* `default`.

- **A. Blinking block** — the most common terminal factory default; least jarring first step.
- **B. Steady beam** — modern GUI-editor feel.

**Recommendation: A** for the cycle's first managed stop and the picker's suggested row —
smallest delta from what most users already see. (Config default is `default`, per Fork 3.)

### Fork 5 — picker preview mechanism

- **A. Live-drive only:** selection sets the desired state; the run-loop reconcile morphs the
  real caret — which is sitting right in the picker's own query field, visible. Esc restores.
- **B. Descriptive rows only:** glyph mocks ("▮ Block · blinking"), no live write.
- **C. Both:** rows carry descriptive glyphs (they must render *something* anyway) AND the live
  caret morphs.

**Recommendation: C.** It is A plus better row labels — no extra machinery — and it stays honest
in DECSCUSR-ignoring terminals, where the rows still describe what Enter will persist.

### Fork 6 — fallback when the terminal ignores DECSCUSR

- **A. Silent best-effort no-op** (bracketed-paste precedent): we write, terminal ignores,
  nothing breaks; a doc note covers tmux `terminal-overrides`.
- **B. Detection + degraded UI:** no reliable DECSCUSR capability query exists; any detection is
  heuristics (env sniffing) that the theme system deliberately avoids.

**Recommendation: A.** B is unimplementable honestly; the no-silent-UI concern doesn't apply
because nothing hangs or lies — the option simply has no visible effect on that terminal, same
as a color theme on a mono terminal.

### Fork 7 — where the B11 caret logic lives

- **A. Overlays own their carets; arm 3 gains a hide-guard.** Each of the four query overlays
  calls `frame.set_cursor_position` on its own `query_area` (it owns the geometry; paint runs
  after `place_cursor`, last write wins); `place_cursor` arm 3 is gated to not place while any
  input-owning surface is active (menu/prompt/splash/diag thereby get a *hidden* caret).
- **B. Centralize everything in `place_cursor`:** new arms per overlay. Keeps one placement
  site but forces overlay-rect geometry (currently local to `render_overlays`) to be recomputed
  or exported.

**Recommendation: A.** The geometry argument decides it: `query_area` exists only inside each
overlay's paint arm, and duplicating `palette_overlay_rect` math in `place_cursor` is drift bait
(the codebase's own painter/hit-test single-source lesson, `search_field_prefix_cols`). The
hide-guard in arm 3 stays a one-expression census of input owners — testable in one table test.

### Fork 8 (surfaced by grounding) — restore discipline: unconditional or latch-aware?

- **A. Unconditional `DefaultUserShape` in all three term.rs restore sites.** Simplest; but a
  user on Fork-3 `default` (we never wrote) still gets their caret forced to terminal-default on
  exit — usually invisible, but it *would* stomp a style set by the user's shell/tmux hooks.
- **B. Latch-aware: restore only if we ever wrote.** The panic hook needs access to the
  applied-latch (an `AtomicBool`/`OnceLock` beside the hook, set by the reconcile) — small but
  real cross-thread plumbing in `term.rs`.

**Recommendation: B.** It is the honest inverse of edge-triggered writing ("never wrote →
nothing to restore"), the cost is one static flag, and it keeps Fork 3's promise airtight
("`default` means wcartel never touches your caret — including on the way out").

---

## 4. Pointer index (for the spec phase)

| Surface | File : symbol |
|---|---|
| Caret placement | `wordcartel/src/render.rs::place_cursor` (called render.rs:310; overlays paint at :312) |
| Escape-write seam | `wordcartel/src/chrome.rs::reconcile_mouse_capture` + call site `app.rs::run` (app.rs:863) |
| Picker template | `wordcartel/src/theme_picker.rs::{ThemePicker, rebuild_rows, intercept}` |
| Preview/commit funnel | `wordcartel/src/theme_cmds.rs::{preview_selected_theme, commit_theme_picker}` |
| Summon + XOR | `registry.rs` `"theme"` row (registry.rs:348); `editor.rs::open_theme_picker` (editor.rs:848) |
| Option pattern | `registry.rs:633–642` (scrollbar rows); `editor.rs::set_scrollbar_mode` (editor.rs:949) |
| Persistence | `settings.rs::SettingsSnapshot` (:46), `OView` (:120), diff law (:404), LAW-2 test (:1003) |
| Config | `config.rs::ViewConfig` (:154) |
| Restore sites | `term.rs::TerminalGuard::{new rollback :50, Drop :65}`, `install_panic_hook` (:96) |
| B11 render sites | `render_overlays.rs::paint` query arms (~:82, :145, :206, :268); `palette.rs:23 cursor` |
| Budgets | `wordcartel/tests/module_budgets.rs` (app 1000 / render 900); clippy too_many_lines 100 |
