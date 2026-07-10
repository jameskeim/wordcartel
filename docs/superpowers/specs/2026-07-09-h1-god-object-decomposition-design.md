# H1 (round 2) — god-object decomposition: `app.rs` + `render.rs`

**Status:** DRAFT — awaiting Codex spec review.
**Effort:** H1 round 2 (engineering-health H1; `needs-design` → this spec). Branch
`effort-h1-god-object-decomposition`.
**Date:** 2026-07-09 · **Facts as of:** `745698d` (app.rs = 5,521 lines, ~1,946 production
[1–1946] + tests from 1948; render.rs = 3,432 lines, ~1,048 production [1–1048] + tests
from 1049).
**Prior art:** the 2026-07-04 H1 pass (`docs/superpowers/specs/2026-07-04-wordcartel-h1-app-decomposition-design.md`,
commits `4e12212`/`61ddd12`/`4cb7cd3`/`5c908f3`) extracted four cohesive leaves
(`jobs_apply.rs`, `prompts.rs`, `session_restore.rs`, `search_ui.rs`) as verbatim moves but
deliberately left the two hubs (`reduce`, `run`) monolithic — and app.rs regrew +814 lines
in ~3 days because every new feature wires into exactly those hubs
(`docs/engineering-health.md` §H1). This round cuts the hubs.

---

## 1. Goal

A **behavior-IDENTICAL** reorganization of the two god-objects into focused files, plus a
plugin-forward timer seam and a readable `reduce` skeleton, landing BEFORE Effort P (the
1.0 Lua-plugin capstone). Effort P's plugin message-arms and event hooks wire into exactly
`reduce`/registry territory; its diff must land in files whose main content IS that
machinery, not at line 1,100 of a six-topic file.

No feature changes, no bug fixes, no behavior changes. Every move is either a verbatim
cut-and-paste or a mechanically-equivalent restructuring whose equivalence is pinned by
tests written first. Safety net: the compiler + the shell test suite (~925 tests) + the
e2e `reduce → advance → render` journeys (`wordcartel/src/e2e.rs`) + golden
TestBackend render tests + the PTY smoke suite (mandatory-run, advisory-pass).

### Locked decisions (settled with the user — not re-opened here)

1. **Timer seam = static fn-pointer table** (`timers.rs`, §4). No traits, no borrow
   friction. The static slice upgrades to a `Vec` when Effort P needs dynamic (plugin)
   timer registration — recorded in the module doc (§4.5).
2. **Render scope = verbatim helper moves ONLY** (§7): geometry/hit-test →
   `chrome_geom.rs`, status builders → `render_status.rs`. The 522-line `render()` body
   split (render.rs:475–996) is **out of scope** (§9).
3. **Reduce depth = go deep** (§5–§6): per-stage handlers in feature modules + the
   `fold_and_continue` micro-epilogue helper, THEN the `Input(Key)` arm extraction and the
   overlay list-nav unification — each preserving every per-overlay side effect.

---

## 2. Command-surface contract conformance

**N/A — this effort does not change the command surface.** Checked stage by stage against
`docs/design/command-surface-contract.md`: the registry (`registry.rs`), the palette row
builder (`palette::rebuild_rows`), the menu builder (`menu::build`), user-settable options
and their shared setters (`set_scrollbar_mode` / `set_status_line_mode` / …), and
keybinding-hint resolution are all **untouched**. Only code that *dispatches* commands
relocates verbatim: the menu/palette interception stages that call
`dispatch_overlay_command` (app.rs:443, 492) move into `menu.rs`/`palette.rs` (§5), the
keymap-resolution arm that calls `reg.dispatch` (app.rs:1113) moves into `input.rs` (§6.1),
and `rebuild_keymap_if_requested` (which re-resolves hints after a preset switch,
app.rs:193) moves verbatim to `theme_cmds.rs` (§3.1) with an unchanged signature — the
run-loop and e2e call sites keep calling it at the same point in the cycle. The contract's
invariant tests (palette-completeness, every-option-has-a-command, hint re-resolution)
remain in place and are merge gates as always; none of them needs edits.

---

## 3. app.rs leaf extractions (verbatim moves, prior-H1 pattern)

Pattern reused from commits `4e12212`…`5c908f3`: free fns over `&mut Editor` move to a
leaf module, one module per domain, `#[cfg(test)]` tests that exercise ONLY the moved fns
ride along, callers update paths (no re-export shims — the prior H1 set this precedent;
nine satellites already import `crate::app::Msg` directly and keep doing so). One commit
per module. All modules are declared in `lib.rs` (module declarations live there).

### 3.1 `theme_cmds.rs` — theme/keymap request-flag seams + picker funnels (~95 prod lines)

| fn | today (app.rs) | moves as | external callers to update |
|---|---|---|---|
| `rebuild_keymap_if_requested` | `pub(crate)` :193–210 | `pub(crate)` | run loop :1677 (stays in app.rs, path update); e2e.rs:104, :129 |
| `rederive_theme_if_requested` | `pub(crate)` :225–245 | `pub(crate)` | run loop :1682 |
| `preview_selected_theme` | `pub(crate)` :255–274 | `pub(crate)` | mouse.rs:207, :229; theme_picker.rs:132 (test); the theme-picker stage (§5) |
| `commit_theme_picker` | `pub(crate)` :279–285 | `pub(crate)` | mouse.rs:230; the theme-picker stage (§5) |

Tests riding along: the keymap-rebuild seam tests (app.rs:5238–5296) and the rederive seam
tests (app.rs:5344–5400). Two additional tests import the moved helpers directly and MUST
repoint their `crate::app::…` `use` after the move: `rederive_respects_picker_committed_theme`
(`use crate::app::rederive_theme_if_requested;` at app.rs:5409) and
`preview_applies_ansi16_policy` (`use crate::app::preview_selected_theme;` at app.rs:5482).

### 3.2 `chrome.rs` — chrome recompute/visibility + mouse-capture reconcile (~120 prod lines)

| fn | today (app.rs) | moves as | external callers to update |
|---|---|---|---|
| `recompute_scrollbar_visible` | `pub` :1738–1763 | `pub` | `advance` :1267 + run startup :1586 (both stay in app.rs, path update) |
| `recompute_menu_bar` | `pub` :1767–1785 | `pub` | `advance` :1268 |
| `status_line_visible` | `pub` :1790–1802 | `pub` | render.rs:917, :933 |
| `recompute_status_line` | `pub` :1805–1820 | `pub` | `advance` :1269 + run startup :1587 |
| `reconcile_mouse_capture` | `pub` (generic `<W: std::io::Write>`) :1827–1847 | `pub` | run loop :1584, :1694 |

Tests riding along: the fade/dwell recompute tests (app.rs:3704–3730), the menu-bar
recompute + reconcile tests (app.rs:3737–3790), the status-line visibility tests
(app.rs:5453–5466). The doc comment on `recompute_scrollbar_visible` carrying the
**fire-order is load-bearing** warning (app.rs:1735–1737) moves with it verbatim.

### 3.3 `persist_session` → `session_restore.rs` (~48 prod lines)

`persist_session` (private, app.rs:1852–1894) + the `persist_session_for_test` shim
(:1896–1899) move to `session_restore.rs` (which already holds the restore half and whose
doc comment at session_restore.rs:35 references `persist_session` by name). Visibility:
`pub(crate)` (run() calls it from app.rs at :1701 and :1718). Tests riding along — BOTH
call `crate::app::persist_session_for_test` and repoint after the move:
`persist_session_captures_scratch_even_when_active_unnamed` (app.rs:4466–4478) and
`persist_session_clears_stale_scratch_when_oversized` (app.rs:4483–4500).

### 3.4 Two micro-leaves

- `file_browser_enter` (`pub(crate)`, app.rs:290–324) → `file_browser.rs`. Production
  callers to repoint: mouse.rs:272; the file-browser stage (§5). NOTE (Codex r2
  correction): the similarly-named test
  `file_browser_enter_on_file_opens_it_when_clean` (session_restore.rs:151) does NOT call
  `file_browser_enter` — it calls `open_into_current` directly (session_restore.rs:159),
  so it needs NO path update from this move. The reduce-level Enter path is exercised
  through `crate::app::reduce` (e.g. app.rs:4842, :4899), which is unaffected.
- `outline_jump_to` (`pub`, app.rs:326–334) → `outline_overlay.rs`. Callers: mouse.rs:323;
  the outline stage (§5); test app.rs:4065 (stays in app.rs — it exercises marks/jump
  behavior through several app fns — path update only).

### 3.5 What deliberately STAYS in app.rs

Per the prior H1 fork-2 decision (overlay glue = `reduce`'s limbs): `Msg` (:26–66) +
its `Debug` impl, `ExitReason` (:69–73), `keep_overlay_visible` (:121–124),
`hydrate_overlays` (:129–151), `dispatch_overlay_command` (:155–173),
`menu_select_for_test` (:175–187), `step` (test-only, :1237–1242), `SystemClock`
(:1248–1257), `advance` (:1266–1287), `first_frame_settle` (:1293–1297), `reduce`'s
skeleton (§5), and `run` (:1306–1727, minus the deadline block that moves to timers.rs,
§4). The residual app.rs is the Elm core: Msg + reduce-skeleton + advance + run.

---

## 4. `timers.rs` — the timed-subsystem hub (the durable anti-regrowth seam)

Today `run()`'s loop computes **exactly 8 deadline terms** inline (app.rs:1613–1650),
folds them via `crate::diagnostics_run::next_deadline(&[…])` (:1651–1660; min over the
`Some`s — diagnostics_run.rs:28), converts to a `recv_timeout` with a 3600 s fallback
(:1661–1663), and maps `Timeout → Msg::Tick` (:1664–1668). Every new timed feature edits
this block — the regrowth shape #2. The seam:

### 4.1 The table

```rust
// timers.rs — timed-subsystem hub. Static fn-pointer table; each subsystem's
// `deadline` embeds its own in-flight/pending gate so a de-gated past-due Some
// can never reach recv_timeout(0) (the swap-thrash / A3-spin class).
pub(crate) struct TimedSubsystem {
    pub(crate) name: &'static str,
    pub(crate) deadline: fn(&Editor, u64) -> Option<u64>, // (editor, now_ms)
}

pub(crate) static SUBSYSTEMS: &[TimedSubsystem] = &[
    TimedSubsystem { name: "swap",         deadline: swap_deadline },
    TimedSubsystem { name: "save_quit",    deadline: sq_deadline },
    TimedSubsystem { name: "scrollbar",    deadline: sb_deadline },
    TimedSubsystem { name: "menu_dwell",   deadline: menu_deadline },
    TimedSubsystem { name: "sb_dwell",     deadline: sb_dwell_deadline },
    TimedSubsystem { name: "status_dwell", deadline: status_dwell_deadline },
    TimedSubsystem { name: "diagnostics",  deadline: diag_deadline },
    TimedSubsystem { name: "reconcile",    deadline: reconcile_deadline },
];
```

Order = today's fold order (min is order-independent; keeping it makes the table auditable
against the old block). `name` is read by the table-driven guardrail tests (§8.2) and is
the plugin-forward identity — no dead-field clippy risk.

Each `deadline` fn is a verbatim transplant of its term **including its gate**:

- `swap_deadline` (from :1613–1619): `swap::pending(dirty, version, swapped_version) &&
  !swap_in_flight` gate around `swap::next_deadline_ms(now, last_edit_at, last_swap_at)`
  (swap.rs:63) — the comment block explaining the idle-thrash bug (:1609–1612) moves with
  it.
- `sq_deadline` (from :1620): `editor.pending_after_save.as_ref().map(|p|
  p.at_ms.saturating_add(SAVE_QUIT_TIMEOUT_MS))` — `now` unused (`_now`). This term only
  WAKES the loop; the disposition fires pre-recv (§4.3).
- `sb_deadline` (from :1623–1627): `scrollbar_until_ms > now` gate.
- `menu_deadline` (from :1631): `menu_reveal_due.or(menu_hide_due)` — at most one is
  `Some` by construction (the mouse Moved arm clears the other side; `recompute_menu_bar`
  clears a fired due) — comment :1628–1630 moves with it.
- `sb_dwell_deadline` (:1633), `status_dwell_deadline` (:1634): the `_reveal_due.or(_hide_due)` pairs.
- `diag_deadline` (from :1641–1645): `in_flight_version.is_none()` exclusion (the A3 spin
  fix; comment :1635–1640 moves with it).
- `reconcile_deadline` (from :1646–1650): same in-flight exclusion.

### 4.2 The three free fns

```rust
/// Min over the table — replaces run()'s 8-term inline block + next_deadline fold.
pub(crate) fn next_wake(editor: &Editor, now: u64) -> Option<u64> {
    SUBSYSTEMS.iter().filter_map(|s| (s.deadline)(editor, now)).min()
}

/// Verbatim body of reduce's Msg::Tick arm (app.rs:1175–1202): swap-write dispatch
/// (pending && !in_flight && due → Ctx → dispatch_swap_write), diagnostics dispatch
/// (diag_cfg.enabled && diag_due → build ignore-words set → dispatch_diagnostics),
/// reconcile dispatch (reconcile_due → dispatch_reconcile).
pub(crate) fn on_tick(editor: &mut Editor, ex: &dyn Executor, clock: &dyn Clock,
    msg_tx: &std::sync::mpsc::Sender<Msg>) { … }

/// Loop-top pre-recv step: the save-timeout disposition. Fired here, NOT in the Tick
/// arm — the sq deadline only wakes the loop (fire-site heterogeneity is behavior, §8.1-B).
pub(crate) fn pre_recv(editor: &mut Editor, now: u64) { save_timeout_tick(editor, now); }
```

`SAVE_QUIT_TIMEOUT_MS` (app.rs:1908) and `save_timeout_tick` (:1912–1941, with its
compiler-exhaustive `PostSaveAction` match) move to timers.rs verbatim; their tests
(app.rs:5036–5095, the C4 seam + quit-supersedes-close family) ride along.
`diagnostics_run::next_deadline` stays where it is untouched (it has its own unit test at
diagnostics_run.rs:101); `next_wake` does its own iterator-min rather than allocating a
slice for it.

### 4.3 The rewired loop top (app.rs)

```rust
loop {
    let now = clock.now_ms();
    crate::timers::pre_recv(&mut editor, now);
    let timeout = crate::timers::next_wake(&editor, now)
        .map(|d| std::time::Duration::from_millis(d.saturating_sub(now)))
        .unwrap_or(std::time::Duration::from_secs(3600));
    let msg = match msg_rx.recv_timeout(timeout) { /* verbatim :1664–1668 */ };
    …
}
```

Everything below the recv (`InputThreadDied` pre-reduce intercept :1671–1674, the
post-reduce epilogue :1675–1704) is untouched. `reduce`'s `Msg::Tick` arm becomes
`Msg::Tick => crate::timers::on_tick(editor, ex, clock, msg_tx),`.

### 4.4 What the table does NOT own (fire-site heterogeneity preserved)

The table owns **deadline computation only**. Fire sites remain heterogeneous, exactly as
today, because their placement is load-bearing (§8.1-B):

- `save_quit` fires loop-top pre-recv (`pre_recv`), before the message is read.
- `swap` / `diagnostics` / `reconcile` fire inside reduce's Tick arm (`on_tick`).
- The three chrome dwell pairs fire in `advance()`'s `recompute_*` calls every iteration
  pre-draw (app.rs:1267–1269 → chrome.rs after §3.2), where dwell flips `*_revealed`
  BEFORE visibility is read (the doc comment at :1735–1737).
- Arming stays distributed: mouse.rs Moved arms (dwells — mouse.rs:433–485), reduce's
  epilogue (diagnostics debounce :1212–1215), `advance()` (reconcile debounce :1277–1286).

### 4.5 Plugin-forward note (recorded per the locked decision)

The static `&[TimedSubsystem]` slice is deliberately the smallest thing that breaks the
"every timed feature edits run()'s loop" regrowth shape. When Effort P needs dynamic
(plugin) timer registration, the slice upgrades to a `Vec<TimedSubsystem>` held by the
loop (built from `SUBSYSTEMS` + plugin registrations); `next_wake`/guardrails iterate the
Vec identically. `deadline` stays a plain fn pointer for builtins; a plugin entry will
need a closure-capable representation then — that is Effort P's call, not pre-built here.

### 4.6 Zero release cost

The table is `static` (no allocation, no indirection beyond one fn-pointer call per term —
same order of work as today's inline block). Any instrumentation the guardrail tests need
beyond iterating `SUBSYSTEMS` by `name` (e.g. fire counters) MUST be `#[cfg(test)]`-gated;
release builds carry the table and nothing else.

---

## 5. `reduce` decomposition — skeleton + per-stage handlers

`reduce` (app.rs:337–1222, 886 lines) is: prologue (smoke-panic check :354–362) → **ten
interception stages** → the 14-arm normal match (:1090–1208) → the epilogue (:1209–1221:
version-change hook, then drain-fold, then `!editor.quit`). Each stage carries its own
micro-epilogue (`for o in ex.drain() { … } return !editor.quit;`) — `for o in ex.drain()`
appears **23×** in production app.rs (21 stage-internal sites + `dispatch_overlay_command`
:170 + the main epilogue :1218). Stage early-returns **skip** the version-change hook
(:1209–1216) — a deliberate asymmetry (§8.1-A).

### 5.1 The `Handled` protocol and `fold_and_continue`

```rust
// app.rs — next to Msg.
pub(crate) enum Handled {
    Done(bool),   // stage consumed the message; reduce returns this bool
    Pass(Msg),    // fall through — ownership of the message returns to the chain
}

/// The shared stage micro-epilogue: drain ready executor results, fold them in,
/// report keep-running. Factored from the 21 verbatim repetitions.
pub(crate) fn fold_and_continue(editor: &mut Editor, ex: &dyn Executor,
    clock: &dyn Clock, msg_tx: &std::sync::mpsc::Sender<Msg>) -> bool {
    for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, editor, ex, clock, msg_tx); }
    !editor.quit
}
```

`Handled::Pass(Msg)` (not `&Msg`) because two stages consume the message by value today:
the palette Paste arm binds `Msg::Input(Event::Paste(text)) = msg` (:463) and the prompt
stage matches `msg` by value (:805). Ownership threads through the chain unchanged.

**`fold_and_continue` is NOT applied where the code does not drain today** (§8.1-C): the
search stage's Esc arm (`search_cancel` then `return !editor.quit` — :926) and Alt+a arm
(`search_replace_all` then `return !editor.quit` — :929) return **without draining**; they
become `Handled::Done(!editor.quit)` verbatim. Do not "fix" this to drain — that would be
a behavior change outside this effort's charter.

### 5.2 Stage → module map (bodies move verbatim; gates move into the handlers)

| # | stage | app.rs body | destination | fn | params beyond `(msg, editor)` |
|---|---|---|---|---|---|
| 1 | pending_mark | :365–378 | `marks.rs` | `intercept` | ex, clock, msg_tx |
| 2 | menu | :383–451 | `menu.rs` | `intercept` | reg, keymap, ex, clock, msg_tx |
| 3 | palette | :456–588 | `palette.rs` | `intercept` | reg, keymap, ex, clock, msg_tx |
| 4 | theme picker | :592–700 | `theme_picker.rs` | `intercept` | ex, clock, msg_tx |
| 5 | file browser | :704–798 | `file_browser.rs` | `intercept` | ex, clock, msg_tx |
| 6 | prompt (modal) | :804–849 | `prompts.rs` | `intercept` | ex, clock, msg_tx |
| 7 | minibuffer | :854–900 | `minibuffer.rs` | `intercept` | ex, clock, msg_tx |
| 8 | search | :906–963 | `search_ui.rs` | `intercept` | ex, clock, msg_tx |
| 9 | diag overlay | :968–983 | `diag_overlay.rs` | `intercept` | ex, clock, msg_tx |
| 10 | outline | :985–1088 | `outline_overlay.rs` | `intercept` | ex, clock, msg_tx |

Signature shape (uniform where the params match; the menu/palette pair additionally takes
`reg`/`keymap` for `dispatch_overlay_command`/`rebuild_rows`):

```rust
pub(crate) fn intercept(msg: crate::app::Msg, editor: &mut Editor,
    ex: &dyn Executor, clock: &dyn Clock,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>) -> crate::app::Handled
```

Each handler opens with its own gate (`if editor.<overlay>.is_none() { return
Handled::Pass(msg); }` — transposed from today's `if editor.<overlay>.is_some() { … }`
wrapper) and ends each consuming arm with `Handled::Done(fold_and_continue(…))` (or the
verbatim no-drain `Handled::Done(!editor.quit)` — §5.1). Fall-through points (`// Non-key
msg falls through …`) become `Handled::Pass(msg)`.

Stage-specific notes (each is a verbatim-preservation obligation):

- **outline** — the buffer-mismatch pre-close (:985–988: `outline.buffer_id !=
  active().id → editor.outline = None`) runs for ANY message BEFORE the gate; it is the
  first statement of `outline_overlay::intercept`, before the `is_none()` early-Pass.
  The Enter arm's stale-version close (:1041–1046) has its own drain+return inside the
  arm — preserved as an inner `return Handled::Done(fold_and_continue(…))`.
- **prompt** — when a prompt is open, EVERY message is consumed (its match has arms for
  JobDone/FilterDone/ExportDone/TransformDone/DiagnosticsDone/ClipboardPaste/
  ClipboardAvailability and a `_ => {}` for the rest, then always drains and returns —
  :805–848). `prompts::intercept` therefore never returns `Pass` once the gate admits.
- **menu / palette / theme picker / file browser** — each intercepts Key + `Event::Paste`
  + `Msg::ClipboardPaste` (the async-paste drop guard); other messages Pass. The
  theme-picker Paste and text-edit arms call `preview_selected_theme` (→
  `theme_cmds::preview_selected_theme` after §3.1) exactly where they do today.
- **minibuffer / search / diag / outline** — intercept Key input only; ALL non-key
  messages Pass (the starvation contract — tests
  `minibuffer_does_not_starve_filterdone` :2875, `search_does_not_starve_filterdone`
  :4001, `outline_overlay_does_not_starve_background_messages` :4017).
- **pending_mark** — non-key messages Pass; a Key message drains-and-returns even when
  `k.kind` is not Press (:366–376 — the drain is per key MESSAGE, not per press).

### 5.3 The residual `reduce` skeleton (stays in app.rs)

```rust
pub fn reduce(msg: Msg, editor: &mut Editor, reg: &Registry, keymap: &crate::keymap::KeyTrie,
    ex: &dyn Executor, clock: &dyn Clock, msg_tx: &std::sync::mpsc::Sender<Msg>) -> bool {
    #[cfg(debug_assertions)] /* smoke-panic check — verbatim :354–362 */
    let msg = match crate::marks::intercept(msg, editor, ex, clock, msg_tx)
        { Handled::Done(k) => return k, Handled::Pass(m) => m };
    let msg = match crate::menu::intercept(msg, editor, reg, keymap, ex, clock, msg_tx)
        { Handled::Done(k) => return k, Handled::Pass(m) => m };
    // … palette, theme_picker, file_browser, prompts, minibuffer, search_ui,
    //   diag_overlay, outline_overlay — SAME ORDER as today (§8.1-D) …
    let before = editor.active().document.version;   // verbatim :1090
    match msg { /* the 14 normal arms — Input(Key) → §6.1; Tick → timers::on_tick (§4);
                   the rest verbatim :1091–1208 */ }
    // epilogue — verbatim :1209–1221 (version-change hook + drain-fold + !editor.quit)
}
```

The public signature of `reduce` is UNCHANGED — the e2e harness (e2e.rs:100–111,
:121–141) and ~120 direct `crate::app::reduce(…)` test call sites compile untouched.
The chain order is the behavior; the skeleton makes it readable at a glance.

---

## 6. `reduce` deeper cut (two follow-on tasks, each independently reviewable)

### 6.1 Extract the `Input(Key)` arm → `input.rs`

The normal-mode key arm (app.rs:1092–1136: Esc precedence [pending-cancel >
filter-cancel] → chord push → `keymap.resolve` → Command dispatch + `hydrate_overlays` /
Pending status / None-with-printable-fallthrough-insert) moves verbatim to `input.rs`
(today the key-translation module) as:

```rust
pub(crate) fn handle_key(k: crossterm::event::KeyEvent, editor: &mut Editor,
    reg: &Registry, keymap: &crate::keymap::KeyTrie, ex: &dyn Executor,
    clock: &dyn Clock, msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>)
```

The arm in `reduce` becomes `Msg::Input(Event::Key(k)) if k.kind == Press =>
crate::input::handle_key(k, editor, reg, keymap, ex, clock, msg_tx),`. The Esc-precedence
comment block (:1093–1097) moves with the body. `hydrate_overlays` stays `pub(crate)` in
app.rs and is called from input.rs. The version-capture/epilogue placement is unchanged —
`handle_key` runs INSIDE the normal match, so a key-driven edit still reaches the
version-change hook (unlike stage returns — the asymmetry §8.1-A is not disturbed).

### 6.2 Unify the duplicated overlay list-nav → `list_window.rs`

Four stages (palette :496–540, theme picker :621–671, file browser :727–771, outline
:995–1039) repeat the identical six-key motion block (Up/Down/PageUp/PageDown/Home/End:
`saturating_sub` / `min(len-1)` / `± list_h_for(...)` page steps / 0 / len−1, each followed
by `keep_overlay_visible`). Unification lives in `list_window.rs` (the windowing-math
module — no dependency cycle back into app.rs):

```rust
pub(crate) enum ListNav { Up, Down, PageUp, PageDown, Home, End }
/// Map a motion key to a ListNav; None for non-motion keys.
pub(crate) fn list_nav_key(code: crossterm::event::KeyCode) -> Option<ListNav>;
/// Apply the motion to (selected, scroll_top) over row_count rows in an area_h-tall
/// buffer area — the exact math of the four duplicated blocks (list_h_for + keep_visible).
pub(crate) fn apply_list_nav(nav: ListNav, area_h: u16, row_count: usize,
    selected: &mut usize, scroll_top: &mut usize);
```

Per-overlay side effects are preserved by keeping them OUTSIDE the helper, at the same
program points as today:

- **theme picker**: `preview_selected_theme(editor)` after EVERY motion arm (:627, :636,
  :645, :654, :662, :670) — the motion match calls `apply_list_nav`, drops the `tp`
  borrow, then previews, exactly as today's borrow-scoped blocks do.
- **outline**: motion arms have NO re-query; only the Backspace/Char text-edit arms
  re-snapshot `(blocks, rope)` and call `o.set_query` (:1059–1063, :1074–1078) — the
  text-edit arms are NOT part of this unification and stay verbatim.
- **palette / file browser**: text-edit arms (`rebuild_rows` / `rebuild_entries`) stay
  verbatim; only the six motion keys route through the helper.

Explicitly NOT unified (different math or different state shape — §8.1-H): the menu's
Up/Down (coarse `n.min(15)` two-layer windowing with Left/Right category switching,
:405–435) and the diag overlay's `d.up()`/`d.down()` methods (:972–973).

---

## 7. render.rs leaf extractions (verbatim helper moves ONLY)

render.rs production (1–1048) is: shared pure geometry/hit-test fns (:140–340), style
plumbing (`style_to_ratatui` :51, `style_element` :67, `role_element` :81,
`prefix_element` :100, `text_fg_or_base` :122, `ChromeStyles` :398–462), status text
builders (:349–393, :1002–1029), the interval helpers (`row_is_active` :31, `overlaps`
:38), `fold_marker_for` (:1037–1043), and the ONE monolithic painter `render()`
(:475–996) which ends by calling `render_overlays::paint` (:995 — overlay painting is
already extracted, render_overlays.rs, 452 lines).

### 7.1 `chrome_geom.rs` — shared geometry + hit-testing (13 fns, ~200 prod lines)

Moves verbatim (all currently `pub(crate)`, stay `pub(crate)`): `menu_bar_layout_cats`
(:140), `menu_bar_layout` (:153), `menu_dropdown_rect` (:158), `menu_dropdown_row_at`
(:172), `menu_area` (:197), `windowed_indicator` (:203), `palette_overlay_rect` (:216),
`palette_row_at` (:230), `theme_picker_row_at` (:245), `file_browser_row_at` (:257),
`outline_row_at` (:269), `diag_row_at` (:282), `prompt_choice_at` (:306–340).

Callers to update: mouse.rs (`crate::render::` at :112, :118, :156, :168, :172, :178,
:213, :217, :256, :260, :296, :300, :349, :353, :380, :498 + test sites :772, :805, :912,
:1205, and — Codex r2 addition — four more `palette_overlay_rect` test refs at
mouse.rs:1285, :1324, :1398, :1574), render_overlays.rs (the `use crate::render::{…}` import at :15–18 — note
`ChromeStyles` in that import list STAYS in render.rs, so the import splits), and
render.rs's own tests. The paint/hit-test twinning comments (`menu_area`'s "both MUST
derive through this helper" :192–196; `menu_dropdown_row_at`'s "mirror the paint's
overflows condition" :176–180) move with their fns — they ARE the invariant
documentation.

Tests: pure-geometry tests ride along (e.g. `palette_overlay_rect_sizes_to_row_count`
:1307–1316, the `menu_dropdown_*` windowing/hit-test family :3203–3427); tests that mix
geometry with TestBackend painting stay in render.rs and update paths. The plan enumerates
the split per test.

### 7.2 `render_status.rs` — status-line text builders (3 fns, ~75 prod lines)

Moves verbatim: `status_left_text` (:349–371, `pub(crate)`), `word_count_segment`
(:378–393, `pub(crate)`), `format_search_bar` (:1002–1029 — today **private**; becomes
`pub(crate)` since `render()` now calls it cross-module at :901; the only visibility
change in the whole render scope). Callers: render.rs `render()` (:901, :918, :935) and
the tests at :1523–1532, :3044–3055 (ride along).

**Correction to the scoping study:** `fold_marker_for` (:1037–1043, `pub`) was grouped
with the status builders there, but it is a row-loop helper consumed inside `render()`'s
editing-area loop (:648), not status chrome — it STAYS in render.rs (with its test
:1518–1519).

Also staying in render.rs: `row_is_active`/`overlaps`, the style plumbing +
`ChromeStyles` (built per-frame in `render()` and borrowed by `render_overlays::paint`),
`HEADING_GLYPHS` (:25), and `render()` itself (§9).

---

## 8. Behavior-identical invariants — the load-bearing subtleties, preserved NOT unified

Each item below is something a well-meaning cleanup would "fix"; every one is behavior.
The final whole-branch review checks each explicitly.

- **A. The skipped-epilogue asymmetry.** Stage early-returns skip the version-change hook
  (app.rs:1209–1216) — a palette-dispatched edit returns via the stage micro-epilogue
  (:584–585) WITHOUT setting `last_edit_at` or arming the diagnostics debounce; only edits
  that reach the normal match get the hook. Do NOT unify stage returns with the main
  epilogue. Pinned by a new guardrail test (§8.2-G1).
- **B. Deadline fire-site heterogeneity + fire order.** sq fires loop-top pre-recv;
  swap/diag/reconcile fire in the Tick arm; the three chrome dwell pairs fire in
  `advance()` pre-draw where dwell flips `*_revealed` BEFORE visibility is computed
  (app.rs:1735–1737). The timers table owns deadlines only (§4.4).
- **C. The search stage's no-drain returns.** Esc (:926) and Alt+a (:929) return without
  draining; the other consuming arms drain. `fold_and_continue` is applied ONLY to sites
  that drain today. Pinned by §8.2-G2.
- **D. Interception ordering.** pending_mark → menu → palette → theme_picker →
  file_browser → prompt → minibuffer → search → diag → outline, with each stage's exact
  fall-through class (Key+Paste+ClipboardPaste vs Key-only vs consume-everything for
  prompt). The existing starvation tests (:2875, :4001, :4017) and async-paste-drop tests
  (:3440–3487, :3620) are the net — the stage moves must NOT require touching them.
- **E. Anti-spin gates travel with their deadlines.** swap `pending && !in_flight`
  (:1613–1615), diag/reconcile in-flight exclusion (:1641–1650), menu-dwell
  "at most one Some" (:1628–1630), `recompute_*` clearing fired dues. Idle is free: a
  settled, no-overlay editor yields `next_wake == None` → the 3600 s block (§8.2-G3).
- **F. `InputThreadDied` is intercepted PRE-reduce** (app.rs:1671–1674); reduce's arm is a
  deliberate exhaustiveness no-op (:1205–1207). Neither moves.
- **G. e2e harness parity.** e2e.rs `step()` (:100–111) runs the literal production
  sequence `reduce → rebuild_keymap_if_requested → note_undo_eviction → advance → render`
  (`step_timed` :121–141 mirrors it). `reduce`/`advance` signatures are frozen;
  `rebuild_keymap_if_requested`'s path change (→ `theme_cmds`) touches e2e.rs:104/:129
  mechanically.
- **H. Per-overlay side effects under list-nav unification** (§6.2): theme-picker preview
  on every motion; outline re-query only on text edits; menu/diag excluded from
  unification.
- **I. Outline buffer-mismatch pre-close** (:985–988) runs for any message before the
  stage gate.
- **J. Prompt-modal background merge** (:804–849): a JobDone arriving under a modal must
  still merge (save&quit depends on it).
- **K. Golden render parity.** No paint code changes; TestBackend snapshot tests and PTY
  smoke are unchanged nets.
- **L. pending_mark drains per key MESSAGE** (incl. non-Press kinds, :366–376).

---

## 9. Out of scope

- **The `render()` body split** (render.rs:475–996, ~522 lines). Explicitly DEFERRED to
  its own immediate-next effort. Why: the row loop shares ~10 locals across a twin
  segs/placed span-builder pair (:654–847) — it is NOT a verbatim split; slicing it churns
  the golden-render tests, and it has low context overlap with the app.rs hub work (a
  different reviewer skill set: paint semantics vs. event-loop invariants). Nothing in
  this effort's module layout prejudges that split; `chrome_geom`/`render_status` shrink
  render.rs around a `render()` left byte-identical.
- Upgrading/patching `pulldown-cmark`; any behavior/UX change; any new feature.
- Moving `Msg` out of app.rs (prior-H1 fork 1 stands).

---

## 10. Task-level decomposition (ordered; each independently reviewable)

Each task is one commit (or one commit per module inside it, matching the prior-H1
pattern), TDD where a new test pins behavior first, `cargo test` + workspace clippy green
at every boundary.

1. **T1 — Guardrail pins (tests only, written against CURRENT code, all green
   pre-refactor).** (G1) `palette_dispatched_edit_skips_version_hook`: dispatch an editing
   command via palette Enter through `reduce`; assert `document.version` advanced but
   `last_edit_at` is `None` and the diag debounce is unarmed (pins §8.1-A). (G2)
   `search_esc_does_not_drain_executor`: queue an outcome in the test executor, send Esc
   with the search overlay open; assert the outcome is still queued (pins §8.1-C). (G4)
   `theme_picker_motion_previews_through_reduce`: Down-arrow via `reduce` with the picker
   open flips the active theme (pins §8.1-H) — **already covered** by
   `theme_picker_preview_pin_visible_row` (app.rs:4921–4957, confirmed Codex r2), so G4 is
   a keep-green obligation, not a new test. (G5) `outline_motion_does_not_requery`: motion
   preserves `rows`; a Char edit re-queries (pins §8.1-H).
2. **T2 — leaf `theme_cmds.rs`** (§3.1) + caller/test-path updates.
3. **T3 — leaf `chrome.rs`** (§3.2) + caller/test-path updates.
4. **T4 — leaf moves: `persist_session` → session_restore.rs; `file_browser_enter` →
   file_browser.rs; `outline_jump_to` → outline_overlay.rs** (§3.3–3.4).
5. **T5 — reduce stages, overlay group:** `Handled` enum + menu/palette/theme_picker/
   file_browser `intercept` fns (§5.2 #2–5), bodies verbatim (inline drains kept — the
   helper comes in T7). Starvation/paste-drop tests untouched.
6. **T6 — reduce stages, modal/input group:** pending_mark/prompt/minibuffer/search/diag/
   outline `intercept` fns (§5.2 #1, 6–10). Search no-drain returns verbatim (G2 stays
   green).
7. **T7 — `fold_and_continue`:** factor the helper; rewrite the drain+return sites in the
   ten stage handlers + the two glue sites to call it; the two no-drain search returns and
   the main epilogue (:1217–1221) are NOT rewritten.
8. **T8 — `timers.rs` hub** (§4): move `SAVE_QUIT_TIMEOUT_MS` + `save_timeout_tick` (+
   their tests), add the 8 deadline fns + `SUBSYSTEMS` + `next_wake` + `on_tick` +
   `pre_recv`, rewire run()'s loop top and reduce's Tick arm. New guardrails (§8.2-G3):
   `next_wake_none_when_settled` (a clean, settled, no-overlay editor → `None`) and the
   table-driven `per_subsystem_gate_yields_none` (for each named subsystem, its
   in-flight/pending gate ⇒ `None` — generalizing `diag_deadline_excluded_when_in_flight`
   :4249, which stays). `idle_buffer_does_not_thrash_the_swap_file` (:2037) stays green
   unmodified.
9. **T9 — deeper cut: `Input(Key)` arm → `input::handle_key`** (§6.1).
10. **T10 — deeper cut: overlay list-nav unification** (§6.2) — `ListNav` +
    `list_nav_key` + `apply_list_nav` in list_window.rs; the four stage handlers' motion
    arms route through it; G4/G5 stay green.
11. **T11 — render leaf `chrome_geom.rs`** (§7.1) + mouse.rs/render_overlays.rs/test path
    updates.
12. **T12 — render leaf `render_status.rs`** (§7.2; `format_search_bar` → `pub(crate)`).

Then the two final gates per the standard pipeline: Fable whole-branch review (compiling
probes; §8 checklist) + Codex pre-merge GO/NO-GO.

---

## 11. Testing & guardrails (merge gates)

- **GATE:** `cargo test` green across all suites; `cargo build`/`cargo test --no-run`
  warning-free for touched crates; `cargo clippy --workspace --all-targets` clean.
- **GATE:** the new guardrail pins G1–G5 (T1) + the timers guardrails (T8) green.
- **GATE (unchanged-net):** the starvation tests (:2875, :4001, :4017), the
  async-paste-drop family (:3440–3487, :3620), `idle_buffer_does_not_thrash_the_swap_file`
  (:2037), `continuous_editing_checkpoints_but_stays_bounded` (:2071),
  `diag_deadline_excluded_when_in_flight` (:4249), the e2e journeys, and the golden
  TestBackend render tests all pass WITHOUT modification (path-only updates where a moved
  fn is referenced are permitted; assertion changes are not).
- **GATE (contract):** the command-surface invariant tests (palette-completeness,
  every-option-has-a-command, hint re-resolution) pass unmodified.
- **Mandatory-run, advisory-pass:** `scripts/smoke/run.sh` — the pre-merge report quotes
  its one-line summary verbatim.
- **Zero release cost:** any timer instrumentation beyond the static table is
  `#[cfg(test)]`-gated (§4.6).
- House style throughout: hand-formatted dense Rust, no `cargo fmt`, tests ride along
  with their fns, no re-export shims, module docs name what moved and from where
  ("Extracted verbatim from app.rs (Effort H1 round 2)" — matching jobs_apply.rs:1–3).

## 12. Expected residual shapes (orientation, not gates)

app.rs production: ~1,946 → roughly 650–750 (Msg + glue + reduce skeleton + normal-match
non-key arms + advance + run + startup). render.rs production: ~1,048 → roughly 750–800
(render() + style plumbing + ChromeStyles + fold_marker_for). New files: timers.rs
(~220 incl. moved save-timeout tests' subject), theme_cmds.rs (~95), chrome.rs (~120),
chrome_geom.rs (~200), render_status.rs (~75), plus ~40–130 added to each of the ten
stage-handler host modules.

## 13. History

- 2026-07-09 — Initial draft (Fable), grounded on `745698d`. Corrections applied against
  the 2026-07-09 scoping study: the micro-epilogue drain appears 23× in production app.rs
  (21 stage sites + dispatch_overlay_command + the main epilogue), not 26× (26 is the
  whole-file count incl. tests); e2e `step()` also calls `rebuild_keymap_if_requested`
  between reduce and note_undo_eviction; `fold_marker_for` reclassified as a row-loop
  helper that stays in render.rs (§7.2).
- 2026-07-09 — Round-2: folded Codex migration-site completeness findings
  (theme_cmds/persist_session/file_browser/chrome_geom test-site lists); behavior design
  confirmed unchanged.
