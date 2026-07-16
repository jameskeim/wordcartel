# H21 — Input-overlay dispatch table (`OverlayId` + `OVERLAYS` seam)

**Status:** spec (pre-Codex-gate)
**Effort:** H21 · debt · UNGATED · size M
**Branch (proposed):** `effort-h21-overlay-dispatch-table`
**Author:** Fable (standing author thread)
**Date:** 2026-07-15
**Arc:** second item of "unify ad-hoc surfaces" (A17 shipped 2026-07-15; this is its structural twin)

---

## 0. One-paragraph summary

The shell has **11 sibling overlay `Option<T>` fields** on `struct Editor` whose *routing* —
is-active, key-input, mouse, render — is written **by hand across seven-plus enumerations in five
files** with **no compiler-enforced exhaustiveness**. An overlay omitted from one enumeration leaks
keystrokes to the buffer (or clicks through) while a modal is visibly up — a **silent-UI /
correctness** class. H21 introduces one `overlays.rs` module — an exhaustive `enum OverlayId` + a
`static OVERLAYS: &[OverlayRow]` fn-pointer table (`{ id, name, is_active, close, intercept, mouse,
render }`) mirroring `timers.rs`'s `SUBSYSTEMS` — and collapses every hand-parallel enumeration into
one table-driven loop. The fields **stay a flat XOR set** (this is the *dispatch* axis, not
data-clustering — H13's note stands). Behavior is preserved **except one deliberate, tested delta**
(splash becomes active on the mouse path, closing an under-splash dwell-arming quirk). This lands
**before Effort P's panel work** so a plugin panel is one static row, not a retrofit.

---

## 1. Grounded problem statement

### 1.1 The 11 overlays (all `Option<crate::…::…State>` on `struct Editor`, `editor.rs`)

| field | state type |
|---|---|
| `search` | `crate::search_overlay::SearchState` |
| `minibuffer` | `crate::minibuffer::Minibuffer` |
| `palette` | `crate::palette::Palette` |
| `outline` | `crate::outline_overlay::OutlineOverlay` |
| `theme_picker` | `crate::theme_picker::ThemePicker` |
| `file_browser` | `crate::file_browser::FileBrowser` |
| `menu` | `crate::menu::MenuView` |
| `prompt` | `crate::prompt::Prompt` |
| `splash` | `crate::splash::Splash` |
| `diag` | `crate::diag_overlay::DiagOverlay` |
| `cursor_picker` | `crate::cursor_picker::CursorPicker` |

`marks` (`editor.pending_mark: Option<MarkPending>`) is **NOT** one of the 11 — it is chord state,
handled specially (see §1.3).

### 1.2 The hand-parallel enumerations (verified against the tree — this is the smell)

1. **`editor.rs::has_active_input_overlay`** — 11-way `.is_some()` OR-chain, doc-commented
   *"EXHAUSTIVE by design: a new input surface must be added here (no catch-all)"*. The exhaustiveness
   is a hand-maintained comment, not a compiler guarantee.
2. **`app.rs::reduce_dispatch` — the intercept chain (= item H10)** — **12 stages** (H10's backlog
   prose says "10-stage" — **stale; correct to 12**): in order `splash, marks, menu, palette,
   theme_picker, cursor_picker, file_browser, prompts, minibuffer, search_ui, diag_overlay,
   outline_overlay`. Each stage is
   `let msg = match crate::X::intercept(...) { Handled::Done(k) => return k, Handled::Pass(m) => m };`.
   Of these, `marks` is the non-overlay pre-stage and the other 11 are the overlays' input axis.
3. **`mouse.rs::route_overlay`** — 10 per-overlay `if editor.X.is_some() { … }` branches
   (`palette, menu, theme_picker, cursor_picker, file_browser, outline, diag, prompt, minibuffer,
   search`; splash excluded — its mouse-Down is consumed upstream by `splash::intercept`). The
   palette and menu branches also embed **Down-left close blocks** (each clears its own field +
   `search` + `diag`, on distinct non-geometric predicates) — a close-list site catalogued in
   §2.4.2 item 4.
4. **`mouse.rs::no_overlay_open`** — a **second, independent is-active predicate** (10-way
   `.is_none()` AND-chain) that **omits `splash`** and thus disagrees with
   `has_active_input_overlay`. Gates both overlay routing and dwell-timer arming.
5. **`render_overlays.rs::paint`** — 8 paint-owned surfaces: `splash` (full-frame early-return),
   `palette`, `outline`, `theme_picker`, `cursor_picker`, `file_browser`, `menu` (bar+dropdown block),
   `diag`. `search`/`minibuffer`/`prompt` do **not** paint here — they paint on the status row in
   `render.rs`.
6. **`render.rs`** — a partial 5-way enumeration
   `let has_overlay = editor.search.is_some() || editor.minibuffer.is_some() ||
   editor.prompt.is_some() || editor.diag.is_some() || editor.outline.is_some();` plus the status-row
   overlay text arms (search bar / minibuffer / prompt) and the Phase-12 caret arms.
7. **`editor.rs::open_*` + `app.rs::dispatch_overlay_command` + the registry `"menu"` command** —
   three families of hand-null lists holding the single-overlay XOR invariant. Each `open_*` method
   nulls its input/picker siblings (e.g. `open_search`, `open_diag`, `open_theme_picker`,
   `open_cursor_picker`, `open_file_browser`, `open_palette`, `open_minibuffer`, `open_prompt`,
   `open_outline`, `open_buffer_switcher`); `dispatch_overlay_command` nulls exactly 5
   (`palette, menu, theme_picker, cursor_picker, file_browser`); and the registry `"menu"` command
   nulls 9 siblings then toggles `menu`. (Note: **only `open_prompt` also nulls `splash`** — splash's
   XOR otherwise rests on control-flow, being startup-only and dismissed before any opener runs; see
   §2.4.1.) A forgotten null = two overlays simultaneously "open".
8. **`render.rs` B11 census test** (`has_active_input_overlay_true_for_every_surface`) — a
   hand-maintained proto-completeness test (opens each of the 11, asserts `has_active_input_overlay`).
   This is the embryo of the A17-style sweep and becomes redundant with the table's own sweep.

**Adding a 12th overlay today** edits: the struct, `has_active_input_overlay`, `no_overlay_open`,
the intercept chain, `route_overlay`, `render_overlays::paint` (and/or `render.rs`), and the
`open_*`/`dispatch_overlay_command` null lists — **~6 files, nothing compiler-forced.**

### 1.3 `marks` is a pre-stage, not an overlay

`marks::intercept` fires when `editor.pending_mark.is_some()`: it **consumes** key messages while
`pending_mark` is set (returns `Handled::Done` for any `Msg::Input(Key)`, Press or not), but only a
`KeyEventKind::Press` resolves the mark (`Char(c)` sets/jumps) or cancels it (`Esc`, or any other key
→ clear `pending_mark`); non-key messages pass through. It also **hard-gates**
`mouse::handle` at its very top (`if editor.pending_mark.is_some() || !editor.mouse_capture {
return; }`). It is chord state with no render surface and no `Option<…State>` overlay struct.
**Decision:** `marks` stays an explicit stage in `reduce_dispatch`, *outside* the OVERLAYS table,
and its mouse hard-gate stays in `mouse::handle`. It is out of the table by construction. **But its
position is load-bearing:** the real chain fires `splash` FIRST, then `marks`, then the rest — so
`marks` sits *after the splash row and before the remaining rows*, not before the whole loop (§2.6).

### 1.4 The signature heterogeneity (the central structural challenge)

The 11 overlay `intercept` fns split into two signature shapes:

- **`menu::intercept` and `palette::intercept`** (7 params):
  `(msg: Msg, editor: &mut Editor, reg: &Registry, keymap: &KeyTrie, ex: &dyn Executor,
  clock: &dyn Clock, msg_tx: &Sender<Msg>) -> Handled`.
  They need `reg`+`keymap` for **row rebuild** (`palette::rebuild_rows` /
  `menu::grouped_commands` — query filtering + `keymap.chord_for(id)` chord display) and for
  `app::dispatch_overlay_command(editor, reg, keymap, …)`.
- **The other 9** (`splash, theme_picker, cursor_picker, file_browser, prompts, minibuffer,
  search_ui, diag_overlay, outline_overlay`) (5 params):
  `(msg: Msg, editor: &mut Editor, ex: &dyn Executor, clock: &dyn Clock, msg_tx: &Sender<Msg>)
  -> Handled`.

A uniform fn-pointer table cannot hold two signatures. Resolution: **§2.2 — a shared `DispatchCtx`.**

---

## 2. Design

### 2.1 New module `overlays.rs` (mirrors `timers.rs`)

```
// overlays.rs — input-overlay dispatch hub. Static fn-pointer table; one row per overlay,
// keyed by an exhaustive OverlayId. Collapses the ~7 hand-parallel enumerations (is-active,
// intercept-chain, mouse-routing, render, XOR-close) into one table + delegating loops.
// Extracted from editor.rs/app.rs/mouse.rs/render_overlays.rs (Effort H21).
//
// Plugin-forward (same pattern timers.rs reserves for plugin timers, which shipped as ONE
// static row reading dynamic `Editor::pending_plugin_timers`): a future plugin panel is ONE
// static `OverlayId::PluginPanel` row whose slots read dynamic `editor.plugin_panel` state —
// content submitted edge-triggered / version-stamped / capped by the P3 pump, painted by a
// builtin Rust painter, keys forwarded to Lua as events. The row is static; the content is
// dynamic. NO `PluginPanel` variant ships in H21 (dead code; defeats exhaustiveness).
```

Contents:

```rust
/// Every input overlay, exhaustive. A new overlay is a new variant; the compiler then
/// forces it into OVERLAYS via `row()` and into every table-derived consumer.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum OverlayId {
    Splash, Menu, Palette, ThemePicker, CursorPicker, FileBrowser,
    Prompt, Minibuffer, Search, Diag, Outline,
}

impl OverlayId {
    /// All variants, in intercept-chain order (the historical fire order minus `marks`).
    pub(crate) const ALL: &'static [OverlayId] = &[
        OverlayId::Splash, OverlayId::Menu, OverlayId::Palette, OverlayId::ThemePicker,
        OverlayId::CursorPicker, OverlayId::FileBrowser, OverlayId::Prompt,
        OverlayId::Minibuffer, OverlayId::Search, OverlayId::Diag, OverlayId::Outline,
    ];

    /// The table row for this id. An EXHAUSTIVE match — a new variant fails to compile
    /// until it is placed here (the exhaustiveness guarantee that closes the silent-UI leak).
    pub(crate) fn row(self) -> &'static OverlayRow { /* exhaustive match → &OVERLAYS[i] */ }
}
```

> **Order note.** `ALL` / `OVERLAYS` preserve **today's intercept-chain order** (`splash, menu,
> palette, theme_picker, cursor_picker, file_browser, prompts, minibuffer, search_ui, diag_overlay,
> outline_overlay`), with `marks` interleaved after the splash row (§2.6). This is the spine for the
> **intercept, is-active, mouse, and close** axes — order there only affects which single intercept
> fires (XOR ⇒ ≤1 active). **The render axis does NOT share this order** — today's paint sequence is a
> *different* permutation (palette before menu, outline mid-list, diag last), and it is observable
> because the always-painted menu-bar chrome sits between two overlay paints. The render fold
> therefore iterates a **separate `RENDER_ORDER` permutation**, not `OVERLAYS` — see §2.3.2. Do not
> assume one array serves both.

```rust
/// One overlay's routing slots. `is_active`/`close` are always present; `intercept`/`mouse`
/// are always present (a no-op-pass overlay still answers the axis); `render` is a RenderSite.
pub(crate) struct OverlayRow {
    #[allow(dead_code)] // read by guardrail tests (select-by-name) + reserved plugin identity
    pub(crate) name: &'static str,
    pub(crate) id: OverlayId,
    pub(crate) is_active: fn(&Editor) -> bool,
    pub(crate) close: fn(&mut Editor),
    pub(crate) intercept: fn(Msg, &mut Editor, &DispatchCtx) -> Handled,
    pub(crate) mouse: fn(&mut Editor, MouseEvent, Rect, &DispatchCtx),
    pub(crate) render: RenderSite,
}

pub(crate) static OVERLAYS: &[OverlayRow] = &[ /* one row per OverlayId, in ALL order */ ];
```

### 2.2 `DispatchCtx` — the shared dispatch context (resolves §1.4)

```rust
/// The non-editor dispatch context, bundled so every overlay `intercept`/`mouse` fn shares
/// ONE signature. The editor is passed SEPARATELY as `&mut Editor` — deliberately EXCLUDED
/// from this struct to avoid a `&mut` aliasing tangle in the table loop (contrast
/// `registry::Ctx`, which OWNS `editor: &'a mut Editor` because command bodies want it in one
/// bundle; the overlay loop cannot, because it holds `&mut editor` to call `(row.intercept)`).
pub(crate) struct DispatchCtx<'a> {
    pub(crate) reg: &'a Registry,
    pub(crate) keymap: &'a KeyTrie,
    pub(crate) ex: &'a dyn Executor,
    pub(crate) clock: &'a dyn Clock,
    pub(crate) msg_tx: &'a Sender<Msg>,
}
```

- Field grounding: `reg`/`keymap`/`ex`/`clock`/`msg_tx` are exactly the params
  `reduce_dispatch`/`mouse::handle` already thread. `Registry` and `KeyTrie` are `crate::registry`
  / `crate::keymap` types. `Sender<Msg>` is `std::sync::mpsc::Sender<crate::app::Msg>`.
- `registry::Ctx` contrast is real and load-bearing: `Ctx` holds `msg_tx` **by value** (owned
  `Sender`, because `dispatch_filter` moves a clone into a `'static` thread). `DispatchCtx` holds
  `msg_tx` **by reference** — it never outlives the loop iteration, so a borrow suffices and avoids a
  clone per stage.
- **Every** overlay `intercept` migrates to `fn(Msg, &mut Editor, &DispatchCtx) -> Handled`. The 9
  five-param handlers ignore `ctx.reg`/`ctx.keymap`; `menu`/`palette` read them. The `_ex/_clock/
  _msg_tx` unused params in `splash::intercept` today become `_ctx` field ignores.

### 2.3 `RenderSite` — the render axis (Q3 = A, minimal)

```rust
/// Where an overlay paints. Every OverlayId answers this axis (the completeness test asserts
/// it) WITHOUT forcing false uniformity: the frame-owned surfaces carry a painter fn; the
/// status-row trio carry a marker (their painting stays in render.rs, untouched).
pub(crate) enum RenderSite {
    /// Painted full-frame by `render_overlays`, gated on the overlay's own `is_active`. The paint
    /// SEQUENCE is `RENDER_ORDER` (§2.3.2) — a permutation distinct from OVERLAYS/intercept order.
    Frame(fn(&mut Frame, &mut Editor, &ChromeStyles)),
    /// Painted on the shared status row inside `render.rs` (search bar / minibuffer / prompt).
    /// NOT relocated by H21 — the marker exists only so the axis is exhaustive (absent from
    /// RENDER_ORDER, which covers only the Frame overlays).
    StatusRow,
}
```

Mapping (grounded against `render_overlays::paint` + `render.rs`):

| OverlayId | RenderSite | painter today |
|---|---|---|
| Splash | `Frame` | `splash::paint` (full-frame early-return) |
| Palette | `Frame` | palette block in `render_overlays::paint` |
| Outline | `Frame` | outline block |
| ThemePicker | `Frame` | theme_picker block |
| CursorPicker | `Frame` | cursor_picker block |
| FileBrowser | `Frame` | file_browser block |
| Diag | `Frame` | diag block |
| Menu | `Frame` | **dropdown** portion only (see §2.3.1) |
| Prompt | `StatusRow` | `render.rs` status-row prompt arm |
| Minibuffer | `StatusRow` | `render.rs` status-row minibuffer arm |
| Search | `StatusRow` | `render.rs` status-row search-bar arm |

#### 2.3.1 The menu bar/dropdown split (the one delicate render extraction)

`render_overlays::paint` fuses **two** things in one block gated on `editor.menu_bar_rows() == 1`:
(a) the **always-on menu bar** (full-width bar background + one label per category — painted even
when `editor.menu` is `None`, i.e. it is **chrome**), and (b) the **dropdown** (painted only when a
category is open). The menu **bar is OUT of scope** (chrome, stays put); only the **dropdown** is the
overlay surface. Therefore:

- The always-on menu-bar paint (background + labels + the `menu_bar_rows()==1` gate) **remains a
  standalone step** in `render_overlays::paint`, byte-identical, run unconditionally as today (it is
  not table-dispatched — it is chrome, and it must paint when `editor.menu` is `None`).
- The **dropdown** paint (the `if let Some(drop_rect) = menu_dropdown_rect(...)` sub-block) becomes
  the `Menu` row's `RenderSite::Frame` painter, invoked by the table render loop when
  `editor.menu.is_some()` and a category is open.
- This split is the single non-mechanical render change; it is behavior-preserving (same rects, same
  styles, same order) and the plan must handle it carefully. **Flagged** in §9 (flag 2), and its
  z-order is pinned by `RENDER_ORDER` in §2.3.2.

**OUT of this cut (Q3 = A):** `render.rs`'s 5-way `has_overlay` enumeration and the Phase-12 / B11
caret arms are **not** touched. The status-row trio keep their `render.rs` painting verbatim; the
table only records that they render at `StatusRow`.

#### 2.3.2 `RENDER_ORDER` — the render axis has its OWN order (Codex round-3 Important)

The intercept/is-active/mouse/close spine (`OVERLAYS` in `ALL` order) and the paint order are
**different permutations** — verified against the two real sources:

- **Intercept order** (`app.rs::reduce_dispatch`): splash, [marks], **menu, palette**, theme_picker,
  cursor_picker, file_browser, prompt, minibuffer, search, diag, **outline**.
- **Frame paint order** (`render_overlays::paint`): splash (early-return), **palette**, **outline**,
  theme_picker, cursor_picker, file_browser, **menu bar+dropdown**, diag.

They differ (palette *before* menu in paint but *after* in intercept; outline mid-paint but
intercept-last; diag intercept-11th but paint-last). A single loop over `OVERLAYS` in intercept order
**cannot** reproduce today's paint order — the original "one order preserves both" claim was wrong.

**Resolution (route 1 — explicit permutation; chosen over proving order-independence).** The render
fold iterates a dedicated constant, distinct from `OVERLAYS`:

```rust
/// Frame-paint order — a permutation over the Frame-site overlays ONLY (the StatusRow trio
/// are absent; they paint in render.rs). DISTINCT from OVERLAYS/intercept order (§2.3.2).
/// Grounded verbatim against today's render_overlays::paint block sequence.
pub(crate) static RENDER_ORDER: &[OverlayId] = &[
    OverlayId::Splash, OverlayId::Palette, OverlayId::Outline, OverlayId::ThemePicker,
    OverlayId::CursorPicker, OverlayId::FileBrowser, OverlayId::Menu, OverlayId::Diag,
];
```

`render_overlays::paint` becomes: splash early-return (RENDER_ORDER[0], as today); then walk
`RENDER_ORDER[1..]` and, for each id whose `is_active`, dispatch its `RenderSite::Frame` painter.

**Why route 1, not route-2 order-independence.** XOR *does* guarantee ≤1 Frame overlay active per
frame (§5 test 2a pins it), so inter-overlay z-order is indeed moot — but that is **not sufficient**,
because the always-painted **menu-bar chrome** (§2.3.1, out of scope, painted even when `menu` is
`None`) sits at a fixed point in the sequence, and the single active overlay's z-order *relative to
that chrome* is observable: today palette/outline/theme_picker/cursor_picker/file_browser paint
**before** the bar chrome (i.e. the bar can overpaint their top row), while **diag** paints **after**
it. An explicit `RENDER_ORDER` reproduces exactly where each overlay sits relative to the bar-chrome
step; proving-and-pinning that interaction per-overlay (route 2) is more clever and more fragile for
no gain. So `RENDER_ORDER` is the vehicle; the XOR fact is merely *why* the permutation only needs to
fix overlay-vs-bar position, not inter-overlay ordering.

**The bar-chrome step's position.** The always-on menu-bar chrome (§2.3.1) is a standalone
unconditional paint, positioned **at the `Menu` slot of the `RENDER_ORDER` walk** — i.e. after the
`FileBrowser` painter and before the `Diag` painter, exactly where the fused menu block sits in
`render_overlays::paint` today. Concretely: when the walk reaches `OverlayId::Menu`, paint the
bar chrome unconditionally, then (if `menu.is_active`) the dropdown `Frame` painter. This keeps
`palette/outline/theme_picker/cursor_picker/file_browser` under the bar and `diag` over it —
byte-identical z-order.

### 2.4 `close_all` — the XOR-close axis

```rust
/// Close every overlay (hold the single-overlay XOR invariant). Replaces the sibling-null
/// lists in every `open_*`, in `dispatch_overlay_command`, in the registry `"menu"` command,
/// and in `route_overlay`'s two Down-left close arms. (NOT the save.rs post-replace
/// stale-clears — those are content-staleness cleanups, not XOR closes — see §2.4.2.)
pub(crate) fn close_all(editor: &mut Editor) {
    for row in OVERLAYS { (row.close)(editor); }
}
```

- Each row's `close` fn sets its own field to `None` (e.g. `|e| e.splash = None`).

#### 2.4.1 What the openers actually null today (grounded correction)

The openers do **NOT** each null all 11 siblings. Verified against `editor.rs`: of the openers
(`open_minibuffer`, `open_prompt`, `open_palette`, `open_search`, `open_diag`, `open_outline`,
`open_theme_picker`, `open_file_browser`, `open_cursor_picker`, `open_buffer_switcher`), **only
`open_prompt` clears `self.splash`** — every other opener leaves `splash` untouched. So the
overlays' XOR is held by **two different mechanisms**, not one uniform null-list:

- **The 10 input/picker overlays** are mutually nulled by each opener's sibling-null list.
- **`splash` is startup-only:** it is set exactly once at launch (no prompt pending) and is dismissed
  by the *first* key/click (`splash::intercept`) before any `open_*` can run. Its XOR therefore rests
  on **control-flow** (it's already `None` by the time any opener fires), not on every opener nulling
  it. `open_prompt` nulls it merely because a quit-confirm prompt can be raised while the splash is
  still up.

**`close_all`'s treatment of `splash` is therefore a DELIBERATE, benign superset.** `close_all` nulls
`splash` unconditionally (it iterates every row). At every real call site splash is already `None`
(startup-only, already dismissed), so this changes nothing observable at runtime — and it is
*arguably more correct*, making the XOR uniform rather than resting on a control-flow argument. This
is an intentional widening, **not** a silent behavior change that matters at runtime; stated here so
Codex reads it as chosen, not accidental.

#### 2.4.2 Close-list migration inventory (complete)

The full set of sites that hand-null sibling overlays, all migrating to `overlays::close_all`:

1. **Every `open_*` in `editor.rs`** (the 10 above). Each `open_X` becomes
   `overlays::close_all(self); self.pending_keys.clear(); self.pending_mark = None; self.X =
   Some(...);`. The `pending_keys.clear()` + `pending_mark = None` lines are **retained verbatim**
   (they are chord/pending-key state, not overlay fields — `close_all` does not touch them). Net
   effect vs today: identical for the 10 input overlays; for `splash`, the benign superset of §2.4.1.
2. **`app.rs::dispatch_overlay_command`** — clears exactly **5** fields today (`palette`, `menu`,
   `theme_picker`, `cursor_picker`, `file_browser` — verified). Replace with
   `overlays::close_all(editor)`: a **safe widening** — it additionally nulls the other overlays that
   XOR already guarantees are `None` at that call site (a command dispatched from palette/menu cannot
   have another overlay live). **Flagged** for Codex to confirm no path reaches it with two overlays
   live (§9 flag 1).
3. **The registry `"menu"` command in `registry.rs`** (missed in the first draft — Codex Minor 1).
   It manually clears **9** siblings (`palette`, `prompt`, `minibuffer`, `search`, `diag`, `outline`,
   `theme_picker`, `cursor_picker`, `file_browser`) + `pending_keys.clear()` + `pending_mark = None`,
   then **TOGGLES** `menu` (`menu = if menu.is_some() { None } else { Some(empty()) }`). Migration
   must preserve the toggle: capture `let was_open = c.editor.menu.is_some();` **before**
   `overlays::close_all` (which nulls `menu` too), then set
   `c.editor.menu = if was_open { None } else { Some(crate::menu::empty()) };` plus the retained
   `pending_keys.clear()`/`pending_mark = None`. Does **not** null `splash` today; `close_all`'s
   superset here is the same benign §2.4.1 case.
4. **`mouse.rs::route_overlay` — two Down-left close blocks** (Codex round-2 Important; predicates
   corrected round-3). Both fire only on `MouseEventKind::Down(MouseButton::Left)`, but their close
   **predicates differ and are NOT the same "outside geometry" test** — each must be preserved
   *exactly* (grounded against the real `route_overlay` body + `chrome_geom`):
   - **Palette block** (`if editor.palette.is_some()`): the `else if !inside` arm. `hit` is
     `chrome_geom::palette_row_at(area, p, col, row)` (a **row-hit**, `None` if the click is on no
     row); `inside` is a **pure geometry** bounds test against `chrome_geom::palette_overlay_rect`.
     The close fires iff **Down-left AND `hit == None` AND `!inside`** — i.e. a click that hits no row
     *and* lands geometrically outside the palette rect. (A click inside the rect but on no row —
     `hit == None`, `inside == true` — does **nothing** and keeps the palette open; the migration
     must not close there.) Clears `palette`, `search`, `diag` today.
   - **Menu block** (`if editor.menu.is_some()`): the final `else` arm. `bar_hit` is
     `chrome_geom::menu_bar_layout` category hit-testing; `row_action` is
     `chrome_geom::menu_dropdown_row_at(...)` mapped to a `MenuRowAction`. **Crucially,
     `menu_dropdown_row_at` returns `None` for non-action cells that are still INSIDE the painted
     dropdown** (e.g. the overflow-indicator bottom row it reserves — it mirrors the painter's
     `overflows` condition). The close fires iff **Down-left AND `bar_hit == None` AND
     `row_action == None`** — this is **NOT** an "outside the bar+dropdown geometry" test: clicking a
     non-actionable cell *inside* the dropdown (the overflow indicator) closes the menu today. Clears
     `menu`, `search`, `diag`.
   Migration (under the mouse-axis fold, §4 step 3): keep each arm's predicate computed **exactly as
   today** — palette: compute `hit` + `inside`, close on `hit == None && !inside`; menu: compute
   `bar_hit` + `row_action`, close on both `None` — and replace only the explicit field-nulls with
   `overlays::close_all(editor)`. This is the same **safe widening** as items 2–3: `search`/`diag`
   are the only non-active siblings the arms null today, and XOR guarantees the rest are already
   `None` while palette/menu is the open overlay, so `close_all` is behavior-identical on the *fields*
   while the *guard* is preserved verbatim. The scroll / row-hit / bar-switch / dispatch branches of
   these blocks are the mouse *behavior* migrating to the per-overlay `mouse` slot (§2.5) and are
   unaffected by this close-list note.

**Explicitly EXCLUDED from the inventory (do NOT migrate to `close_all`):** `save.rs::reload_from_disk`
and `save.rs::load_recovered` each set `editor.search = None; editor.diag = None;` **after**
`replace_buffer` (wholesale buffer replacement). These are deliberate **post-buffer-replace
stale-overlay cleanups** — a search/diag overlay pinned to now-replaced content — **not** overlay-open
XOR enforcement. They must stay as-is; a grep-driven migration must skip both sites. (They null only
`search`/`diag`, never the picker/menu overlays, precisely because they are content-staleness clears,
not single-overlay-invariant clears.)

- **Note on the mouse path** (`mouse.rs` click-open-menu — `editor.menu =
  Some(crate::menu::empty_at(order_idx))`): the click path sets `menu` directly while guarded by
  `no_overlay_open`. After H21 it should call `overlays::close_all(editor)` first for consistency
  (behavior-identical under the `no_overlay_open` guard, which already guarantees no sibling is open).
  The plan decides whether to fold this site; it is low-risk either way.

### 2.5 Consumer collapses

| consumer (today) | after H21 |
|---|---|
| `editor.rs::has_active_input_overlay` (11-way OR) | `OverlayId::ALL.iter().any(\|id\| (id.row().is_active)(self))` |
| `mouse.rs::no_overlay_open` (10-way AND, omits splash) | `!OverlayId::ALL.iter().any(\|id\| (id.row().is_active)(editor))` — **now includes splash (Q4 delta)** |
| `app.rs::reduce_dispatch` 11 overlay stages | splash row fires first, then the `marks` stage, then `for id in &OverlayId::ALL[1..] { match (id.row().intercept)(msg, editor, &ctx) { Done(k)=>return k, Pass(m)=>msg=m } }` — preserves `splash → marks → rest` (§2.6) |
| `mouse.rs::route_overlay` 10 branches | find active row, call `(row.mouse)(editor, ev, area, &ctx)` |
| `editor.rs::open_*` null lists + `dispatch_overlay_command` 5 nulls + registry `"menu"` 9 nulls | `overlays::close_all(editor)` (menu command keeps its toggle — §2.4.2 item 3) |
| `render_overlays::paint` per-overlay blocks | walk **`RENDER_ORDER`** (§2.3.2, a distinct permutation — NOT `OVERLAYS`), dispatch `RenderSite::Frame` painters when active; menu-bar chrome stays a standalone unconditional step at the `Menu` slot; status-row trio untouched |
| `render.rs` B11 census test | superseded by the §5 sweep (removed or thinned) |

### 2.6 The intercept-loop structure — preserving `splash → marks → [others]` (grounded)

**The real chain order matters and constrains the fold.** In `app.rs::reduce_dispatch` the first two
stages are, in this exact order: **`splash::intercept` first, then `marks::intercept`**, then the
other overlays. Verified against the tree (`reduce_dispatch`'s stage list: `splash`, `marks`, `menu`,
`palette`, …). So `marks` is **NOT** a pre-stage before the whole loop — `Splash` is a table row
(row 0 of `OVERLAYS`/`ALL`), and running a marks-before-loop structure would fire marks *ahead* of
splash and **invert today's precedence** (splash dismissal must win before a pending-mark key is
consumed as a mark letter).

**Splash stays a table row** (its is-active / mouse / render / close all live in the table — pulling
its input back out to a hand-written pre-stage would reintroduce exactly the hand-parallelism H21
kills). The fix is to place `marks` correctly **between the splash row and the remaining rows**:

```rust
let ctx = DispatchCtx { reg, keymap, ex, clock, msg_tx };
let mut msg = msg;
// Row 0 (Splash) fires FIRST — its input intercept lives in the table, preserving today's
// "splash dismissal wins before anything else" precedence.
msg = match (OverlayId::Splash.row().intercept)(msg, editor, &ctx) {
    Handled::Done(k) => return k, Handled::Pass(m) => m };
// `marks` is a pre-stage for the REMAINING rows — it sits AFTER splash, BEFORE the rest,
// matching the real chain order (splash → marks → menu → palette → …). It is NOT a table row
// (chord state, no overlay struct — §1.3).
msg = match crate::marks::intercept(msg, editor, &ctx) {
    Handled::Done(k) => return k, Handled::Pass(m) => m };
// The remaining rows (everything after Splash) loop in ALL order.
for id in &OverlayId::ALL[1..] {   // skip Splash (already fired above)
    msg = match (id.row().intercept)(msg, editor, &ctx) {
        Handled::Done(k) => return k,
        Handled::Pass(m) => m,
    };
}
// … the post-interceptor `match msg { … }` tail is unchanged.
```

This preserves the exact real precedence `splash → marks → [menu, palette, theme_picker,
cursor_picker, file_browser, prompts, minibuffer, search_ui, diag_overlay, outline_overlay]`. The
`&ALL[1..]` slice is the clean formulation; the plan may instead give `OverlayId` a helper (e.g.
`ALL_AFTER_SPLASH`) if that reads better, provided the `splash-first / marks-second / rest` order is
preserved and asserted.

`DispatchCtx` borrows are immutable and disjoint from `&mut editor`, so no aliasing conflict.
`Handled::Pass(Msg)` already moves the message by value (see the `Handled` doc-comment — the palette
Paste arm and the prompt stage bind `msg` by value), so the by-value rebind is faithful. `marks` also
migrates to the `DispatchCtx` signature for uniformity even though it is not a table row.

> **Ordering guardrail (add to §5 test 1):** assert `OverlayId::ALL[0] == OverlayId::Splash` — the
> `&ALL[1..]` skip and the splash-first precedence both depend on Splash being row 0.

---

## 3. The deliberate behavior delta (Q4 = A)

**Splash becomes active on the mouse path.** Today `has_active_input_overlay` counts splash but
`no_overlay_open` (mouse) does **not**. Consequence, grounded: `splash::intercept` deliberately
passes mouse-`Moved`/`Scroll` (only Key-Press / Mouse-Down / Paste are consumed, for idle-is-free),
so a mouse move while the splash is up falls through to `mouse::handle`, where `no_overlay_open`
returns `true` (splash omitted) and the **menu-bar / scrollbar dwell timers arm under the splash** —
they can then fire a menu-bar reveal immediately after the splash is dismissed.

After H21, both predicates derive from the same table and **splash's `is_active` is `true`**, so:

- `mouse::handle`'s `if !no_overlay_open(editor) { route_overlay(...); return; }` now routes to the
  splash row's `mouse` slot, which **consumes all mouse events** (Down already dismisses, per today's
  `splash::intercept` Down arm; Moved/Scroll are consumed-without-effect). The dwell-arming block
  below is unreachable while splash is up.
- The splash row's `mouse` fn preserves the dismiss-on-Down semantics (delegates to the same
  dismissal `editor.splash = None` + returns) and swallows Moved/Scroll.

This is an **intentional, tested** delta (§5 test 3): a guardrail asserts no menu-bar/scrollbar dwell
timer arms while `editor.splash.is_some()`. It is the exact silent-UI-adjacent class H21 exists to
close, surfaced by unifying the two predicates.

> **Idle-is-free preserved.** `splash::intercept` still passes `Tick` and background messages
> (only the *mouse* axis changes). The splash's `is_active` gating the mouse path does not add any
> wall-clock work; the loop still blocks when nothing is animating.

---

## 4. Scope

**IN:**
- `overlays.rs`: `OverlayId` (+ `ALL`, `row()`), `OverlayRow`, `OVERLAYS`, `DispatchCtx`,
  `RenderSite`, `close_all`.
- Fold **both** is-active predicates (`has_active_input_overlay`, `no_overlay_open`) onto the table.
- Fold **H10** — the 11 overlay intercept stages become table dispatch; the splash row fires first,
  then the `marks` stage, then a loop over the remaining rows (preserves the real `splash → marks →
  rest` order — §2.6).
- Fold mouse routing (`route_overlay` → find-active + one `(row.mouse)` call).
- Fold the XOR close axis (`close_all` replacing the `open_*` null lists + `dispatch_overlay_command`
  nulls + the registry `"menu"` command's 9 nulls, preserving its toggle + `route_overlay`'s two
  Down-left close arms, each guard preserved verbatim; optional mouse-path `menu` open site).
  **Excludes** the `save.rs`
  post-buffer-replace stale-clears (§2.4.2) — not XOR closes.
- Render: a `RENDER_ORDER` walk (§2.3.2, a permutation distinct from `OVERLAYS`) dispatches the
  `Frame` painters (7 clean + the extracted menu **dropdown**, with the menu-bar chrome as a fixed
  standalone step at the `Menu` slot); `StatusRow` marker for the search/minibuffer/prompt trio.
- The completeness sweep tests (§5).
- The deliberate Q4 splash-mouse delta.

**OUT:**
- Moving status-row painting or the Phase-12 / B11 caret arms out of `render.rs`; the `render.rs`
  5-way `has_overlay` enumeration stays.
- The always-on menu **bar** (chrome; stays in `render_overlays::paint`).
- Any `wc.panel` Lua API or `OverlayId::PluginPanel` variant (Q5 = A — documented, not shipped).
- C5's save-picker (a *future* consumer that proves the seam).
- H13 field clustering — the 11 fields stay a flat XOR set on `Editor` (do **not** wrap in a
  sub-struct).

**Sequencing within the effort (each step behavior-preserving and independently green, except the
Q4 delta which lands with its guardrail):**
1. `overlays.rs` skeleton: `OverlayId` + `ALL` + `row()` + `OverlayRow` + `DispatchCtx` +
   `RenderSite` + `is_active`/`close` slots + `close_all`; migrate `has_active_input_overlay` and
   `no_overlay_open` to the table (Q4 delta lands here, with its guardrail test).
2. Input-chain fold: migrate all 11 `intercept` fns (+ `marks`) to the `DispatchCtx` signature;
   replace `reduce_dispatch`'s 12 stages with the splash-row-first / marks-second / loop-the-rest
   structure (§2.6), preserving the `splash → marks → rest` order.
3. Mouse fold: migrate `route_overlay`'s branches to per-overlay `mouse` fns (including the palette/
   menu Down-left close arms → `close_all`, each guard preserved verbatim — §2.4.2 item 4); add the
   splash
   mouse slot.
4. Close/XOR fold: `close_all` into every `open_*`, `dispatch_overlay_command`, and the registry
   `"menu"` command (keeping the menu toggle — §2.4.2 item 3). Leave the `save.rs` stale-clears alone.
5. Render slot: walk `RENDER_ORDER` (§2.3.2) dispatching `Frame` painters + the menu bar/dropdown
   split (bar chrome as the fixed step at the `Menu` slot); wire `StatusRow`.
6. Completeness sweep: the §5 tests; retire the B11 census test.

---

## 5. Guardrail / completeness tests (first-class spec items)

Live in `overlays.rs` `#[cfg(test)]` (mirroring `timers.rs`'s guardrail tests, which select a
subsystem by `name`), plus the existing e2e/render suites for behavior parity.

1. **Enum↔table bijection + render-axis coverage + splash-first ordering.**
   `OverlayId::ALL.len() == OVERLAYS.len()`; every `id.row().id == id`; `OVERLAYS` order == `ALL`
   order; names are unique; and **`OverlayId::ALL[0] == OverlayId::Splash`** (the `&ALL[1..]`
   intercept skip and the `splash → marks → rest` precedence both depend on Splash being row 0 —
   §2.6). **Render coverage (Codex round-3):** every `OverlayId` has a `RenderSite` (exhaustive by
   `row()`), and `RENDER_ORDER` (§2.3.2) contains **exactly** the ids whose `RenderSite` is `Frame`
   — no `Frame` overlay missing from the paint walk (a silent unpainted overlay), no `StatusRow`
   overlay wrongly in it, and `RENDER_ORDER[0] == OverlayId::Splash` (the paint early-return depends
   on it). The exhaustive `row()` match already makes the compiler force a new variant into
   `OVERLAYS`; this test pins the ordering/identity/render-coverage invariants the compiler can't.
2. **Per-overlay open → exactly-one-active + key-consumed + click-consumed sweep.** For each
   `OverlayId`: open it via its real `open_*` (or the equivalent constructor), then assert
   **(a)** exactly one row's `is_active` is `true` (XOR — proves `close_all`/open nulling); **(b)** a
   `KeyEventKind::Press` routed through `reduce_dispatch` is **consumed** (returns without reaching
   the buffer — no keystroke leak; the silent-UI guarantee); **(c)** a mouse `Down` routed through
   `mouse::handle` is **consumed** (does not fall through to editor gestures). This generalizes and
   subsumes the `render.rs` B11 census.
3. **Q4 guardrail — no dwell-arming under splash.** With `editor.splash = Some(...)`, drive a
   mouse-`Moved` (row 0 and right-edge) through `mouse::handle` under `MenuBarMode::Auto` /
   `TransientMode::Auto`; assert `editor.mouse.menu_reveal_due` and
   `editor.mouse.scrollbar_reveal_due` stay `None` (splash consumes the event; the dwell block is
   unreachable). This pins the deliberate delta and fails if splash is dropped from the table.

Behavior-parity backstops (existing suites, not new): the `wordcartel/src/e2e.rs` in-process
journeys and the render tests continue to pass unchanged (except the retired B11 census), proving the
five folds are behavior-preserving.

---

## 6. Command-surface-contract conformance

**Largely N/A — H21 touches no command registrations, no user-settable options, and no keybinding
hints.** Specifically:

- The `open_*` **commands** in `registry.rs` (`palette`, `open_theme_picker`, `open_file_browser`,
  `open_diag`, `open_outline`, `cursor_picker`, buffer-switcher, splash on/off/toggle, etc.) keep
  their registry entries **verbatim** — H21 refactors the *routing* of the overlays they open, not
  the commands themselves. The single-setter law-6 shapes (e.g. `set_splash`) are untouched.
- **Palette-is-an-overlay stays conformant.** The palette is both a command target and one of the 11
  overlays; H21 keeps `palette::intercept`/`rebuild_rows` reading `reg`+`keymap` (now via
  `DispatchCtx`), so palette exhaustiveness / chord display / `dispatch_overlay_command` all behave
  identically. The contract's palette-completeness and every-option-has-a-command invariant tests are
  unaffected (no command added or removed).
- No menu ⊆ palette change, no hint re-resolution change.

The spec therefore asserts: **H21 does not amend the command-surface contract; its invariant tests
remain green as-is.**

---

## 7. Relationships & bookkeeping

- **H10 (intercept-chain boilerplate)** — folded by H21 (Q1 = A). Mark **shipped-by-H21 at merge**
  in `backlog.toml`; its prose "10-stage" is **stale — it is 12 stages** (correct when archiving).
- **A17 (SHIPPED 2026-07-15)** — the twin. H21 imitates its rigor: a mechanical completeness sweep as
  source of truth, honest scoping, shell-only.
- **C5 (file interface, needs-design)** — sequenced *after* H21; its save/write picker registers as a
  future consumer of this seam (concrete proof the table admits a new overlay cleanly).
- **E8 (view lenses)** — DISTINCT; view lenses are not input/render overlays. Not conflated.
- **H13 (75-field god-object)** — the 11 overlay fields stay a flat XOR set (H13's own note); H21 is
  the dispatch axis only.
- **Effort P (SHIPPED)** — plugin panels are the strongest forward pressure; the module header
  documents the static-row/dynamic-state landing (Q5 = A). No code ships for it now.
- **Timers precedent** — `timers.rs`'s `SUBSYSTEMS` reserved a `Vec` upgrade for plugin timers, but
  plugin timers shipped as **one static row** (`plugin_timer_deadline`) reading dynamic
  `Editor::pending_plugin_timers`. H21 follows that proven static-row/dynamic-state shape exactly —
  no dynamic `Vec<OverlayRow>`.

---

## 8. Module-structure / laws conformance

- **Dispatchers delegate, they don't implement.** `overlays.rs` is a THIN seam (the `OVERLAYS` /
  `RENDER_ORDER` data tables + their delegating folds — intercept loop, mouse find-active, `close_all`
  loop, render walk). `reduce_dispatch`, `route_overlay`, `render_overlays::paint`, and the `open_*`
  bodies **shrink** (hand-enumerations → loops/`close_all`). New behavior (a 12th overlay) enters as
  a **row**, not a hub edit — the Open–Closed seam the CLAUDE.md rule mandates.
- **No silent UI** — the sweep (§5 test 2b/2c) proves every active overlay consumes its keys/clicks;
  the Q4 fix closes the last omitted-arm gap.
- **Anti-regrowth gates** — `clippy::too_many_lines` and `module_budgets.rs`: the loops must keep the
  hub functions under budget; `overlays.rs`'s table is a flat data literal (mark with a reasoned
  `#[allow(clippy::too_many_lines)]` only if the `OVERLAYS` literal or `row()` match exceeds 100
  lines, matching the `render_overlays::paint` precedent).
- **`#![forbid(unsafe_code)]`** — H21 is SHELL-only; `wordcartel-core` untouched. No `unsafe`.
- **Idle-free / O(visible)** — the dispatch loops are O(overlays)=O(11) per event, never
  O(document); no new wall-clock work (§3 note).
- **Exhaustiveness = the win** — `OverlayId::row()`'s exhaustive match compiler-forces every new
  variant into the table, and the sweep forces it to answer every axis.

---

## 9. Open flags for the human / Codex (claims a source read can't fully settle)

1. **`dispatch_overlay_command` widening (§2.4).** Replacing its 5 explicit nulls with `close_all`
   additionally nulls overlays that XOR guarantees are already `None` at that call site. I read no
   call site that relies on a non-nulled overlay surviving, but this rests on the XOR invariant
   holding at every `dispatch_overlay_command` entry — **Codex should confirm** no path reaches it
   with two overlays live.
2. **Menu bar/dropdown split + render z-order (§2.3.1 / §2.3.2).** The one non-mechanical render
   extraction and the highest render-regression-surface step. It is behavior-preserving by
   construction (same rects/styles/sequence), but the plan must (a) extract the dropdown sub-block
   cleanly while leaving the always-on bar chrome in place, and (b) position the bar-chrome step at
   the `Menu` slot of the `RENDER_ORDER` walk so `palette/outline/theme_picker/cursor_picker/
   file_browser` stay *under* the bar and `diag` stays *over* it (the observable overlay-vs-chrome
   z-order). `RENDER_ORDER` is a separate permutation from `OVERLAYS` precisely to make this exact
   — the plan should diff the resulting paint sequence against today's `render_overlays::paint`
   block order byte-for-byte.
3. **Q4 delta is a real behavior change** (§3). The under-splash dwell-arming is a *runtime*
   observation (mouse-Moved passing through `splash::intercept` while `no_overlay_open` omits splash);
   I could not exercise it without running the app. The guardrail test (§5 test 3) both pins the fix
   and would surface if today's behavior was actually relied upon. Human confirms this is a desired
   delta (already resolved Q4 = A) — flagged for visibility, not re-litigation.
4. **Order-observability.** Intercept order (`OVERLAYS`) is observable only for the single firing
   intercept, and render order (`RENDER_ORDER`) only for the single active overlay's z-position
   relative to the always-painted menu-bar chrome — both because XOR ⇒ ≤1 overlay active. If any path
   can have two overlays simultaneously `is_some()` (which would make intercept order OR inter-overlay
   paint order matter), sweep test 2a fails — so the XOR assumption underpinning both the `OVERLAYS`
   and `RENDER_ORDER` simplifications is guarded, and is called out here.
