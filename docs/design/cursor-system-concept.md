# A Unified Cursor System

**Design Concept & Considerations for the Ratatui Prose Editor**

*Brainstorming output — an options-and-considerations brief for the implementation team.*

**Status:** Draft for team review
**Scope:** Cursor & block-marker behaviour, styling, and configuration

---

## 1. Purpose & Framing

This document collects the ideas from a design brainstorm into a single reference for the implementation team. It describes a **unified cursor system** for the prose editor: one configurable writing cursor that the author lives with all day, and a family of built-in, role-signifying cursors and markers that share the editor's theme so the whole system reads as one coherent thing.

Where a decision was reached during the brainstorm it is recorded as **Settled**. Where the intent is clear but the mechanism should be weighed against the actual codebase, it is recorded as an **Open fork** with options and trade-offs rather than a recommendation. Two forks gate most of the architecture and are called out first.

> **THE SINGLE MOST IMPORTANT CONSTRAINT**
>
> A terminal exposes exactly **one** real cursor. `SetCursorStyle` and native blink control only ever apply to that one hardware cursor. Every other cursor-like thing in this system — anchors, block markers, unfocused-pane cursors, gutter indicators — must be **painted** by the application into ordinary cells. The unified system is therefore, at its core, an abstraction that presents these two very different mechanisms (one hardware cursor vs. many painted pseudo-cursors) as a single themed, consistent model.

---

## 2. The Two Gating Decisions

### 2.1 Fork A — Hardware cursor vs. software (painted) cursor

This is the decision the rest of the system hangs from, and it is deliberately left **open for the team** to evaluate against how the editor already renders and how it fits the codebase. The requirement that pushes on it: the writer should be able to pick the cursor *shape*, adjust its *brightness/dim* with up/down, have it *follow theme colours*, and toggle *blink and blink speed* — previewed live while the picker is open.

| | **Option A — Hardware cursor** | **Option B — Software (painted) cursor** | **Option C — Hybrid** |
|---|---|---|---|
| **Mechanism** | Real terminal cursor via DECSCUSR + native blink; app never draws it. | App hides the real cursor and paints a styled cell (reversed video or explicit theme fg/bg) at the caret. | Real cursor for shape/blink, plus OSC 12 tint where the terminal supports it; graceful fallback. |
| **Brightness / dim** | ❌ Not possible. | ✅ Full control — interpolate cell bg toward editor bg. | Partial; depends on terminal. |
| **Theme colour** | Unreliable (OSC 12 only, unreadable). | ✅ Guaranteed. | Where supported only. |
| **Live WYSIWYG picker** | Up/down brightness is a dead key. | ✅ Fully live; caret morphs in place. | Mostly, with caveats. |
| **Blink** | ✅ Native, smooth, free. | App-owned timer; cheap if only the cursor cell redraws. | Native. |
| **Unfocused-terminal behaviour** | ✅ Automatic (terminal hollows/stops it). | Must detect focus (crossterm `FocusGained`/`Lost`) and handle it — recoverable, and arguably better controlled. | Automatic. |
| **Screen-reader cursor tracking** | ✅ Strong — real cursor at caret. | ⚠️ Weaker — no real cursor at caret. | Strong. |
| **Unifies with role cursors** | No — writing cursor is a special case. | ✅ Yes — one painting model for everything. | Partly. |
| **Implementation cost** | Lowest. | Moderate (blink timer, focus handling, glyph compositing). | ⚠️ Highest — cross-terminal testing matrix. |

> **CONSIDERATION FOR THE TEAM**
>
> The stated requirements (brightness control, theme colour, live preview) point toward **Option B**, which also collapses the writing cursor and all role cursors into a single rendering model. Its real costs are app-owned blink (cheap if scoped to one cell), self-managed focus handling, and weaker screen-reader tracking. A pragmatic pattern to evaluate: **Option B as the default path, with an Option-A "accessibility cursor" mode** that swaps in a real DECSCUSR cursor (surrendering brightness) for users who rely on assistive tech. The fallback is simpler than the primary path, so it is cheap insurance rather than a second full system.

### 2.2 Fork B — Marker rendering: display-only insertion

**Settled:** block markers are **display-only insertions**. Dropping the begin-marker injects a reversed `[` cell that shifts the character it lands on — and the rest of the line — one column right. Dropping the end-marker injects a reversed `]` in the column immediately *after* the end character, shifting the following text right. The markers occupy real screen columns and bracket the block inline, WordStar-style: `…text [block of words] more…`.

"Display-only" means the brackets live in the **render buffer**, not the document text. The prose is never mutated; nothing must be stripped on save, excluded from word count, or skipped by search. The cost lands squarely in the **layout layer**, which the team must account for:

- Column-to-offset mapping (click-to-position, caret movement) must know that a line may carry one or two injected, non-document cells.
- Word-wrap must decide whether an injected marker column can push a line to wrap — and behave consistently if so.
- Line-length / ruler math must treat marker columns as visual-only.

This pairs with the persistence model below: a marker's **logical home is a document offset** (a position between two characters); its **visual form is an injected cell** at that offset's screen column. Those two facts together define the entire marker subsystem.

---

## 3. The Writing Cursor (Configurable)

**Settled:** there is exactly one user-configurable cursor — the writing caret. A writer stares at it for hours, so they own its identity. Everything else is a built-in whose look signifies its role and is not user-configurable (though all role cursors inherit theme colour so the family stays cohesive).

### Configuration surface

- **Shape** — selected by left/right arrow through a menu of shapes. On the hardware path the honest menu is block / underline / bar, each steady or blinking. The software path additionally unlocks half-block, hollow/outline block, and custom widths. *Team to confirm the shape menu against the chosen fork.*
- **Brightness / dim** — up/down arrows while the picker is open. Realisable only on the software path (interpolate the cursor colour between full theme accent and the editor background). This is a *global property of the writing cursor only*; role cursors derive their own fixed brightness by design.
- **Blink** — enable/disable and adjustable speed. Native on the hardware path; an app-owned timer on the software path.
- **Colour** — follows theme colours automatically; not a separate manual control.

### Picker interaction — open question for the team

Whether the picker is a **modal settings panel** (open, adjust, close) or a **live inline mode** (the real caret morphs in context as you arrow through options) is largely decided *by Fork A*: the software path makes a fully live, WYSIWYG inline picker natural; the hardware path cannot preview brightness at all and leans toward a plain panel. Recommend deciding the picker style *after* Fork A.

---

## 4. Block Markers (WordStar Model)

The block mechanism follows the WordStar `^KB` / `^KK` model rather than modern shift-drag selection. The defining trait: markers are **persistent, independent landmarks**. The writer drops a begin-marker, then keeps editing — typing, scrolling, working elsewhere — and drops the end-marker later, possibly much later. The block is whatever falls between the two markers once both exist.

### Settled behaviours

- **Glyphs** — reversed `[` (begin) and reversed `]` (end): asymmetric and self-describing, so a lone begin-marker is never ambiguous about which end it is.
- **Insertion semantics** — display-only, per Fork B: begin `[` shifts its landing character right; end `]` sits in the column after the end character and shifts what follows.
- **Persistence** — markers stay visible as durable document landmarks until explicitly cleared. They are not transient selection scaffolding.
- **No pre-region fill** — with only the begin-marker down there is no region, so nothing between it and the caret is highlighted. Region styling appears only once *both* markers exist, and is handled by the editor's existing selection-highlight system (a separate concern from the markers themselves).
- **Caret-on-marker collision** — when the free-moving caret shares or neighbours a marker cell, the two *alternate-blink out of phase*: caret visible while marker dims, then marker visible while caret dims, so neither is lost and the overlap reads as intentional. Trivial on the software path (one A/B toggle or two out-of-phase timers).

> **PERSISTENCE HAS A DATA-MODEL CONSEQUENCE**
>
> Because markers survive edits that move them, a marker must be anchored to a **logical document position** (offset / tracked point that updates on every edit), *not* a fixed (x, y) screen cell. Insert three paragraphs above a begin-marker and it must ride along with its text. The system also needs a **clear affordance** (cf. WordStar `^KH`): decide clear-one vs. clear-both and whether re-pressing the drop key toggles the marker off.

### Open items for the team

1. **Marker count** — the working assumption is one begin + one end pair at a time. *Verify against the existing code*; earlier work may already have fixed this.
2. **Off-screen markers** — because the caret wanders freely, a persistent marker will often scroll out of view while the writer still "owes" it a partner. Needs a gutter / edge indicator showing a marker lives off-screen and in which direction. This merges with the scroll/off-screen-caret indicator in §6.
3. **Column / rectangular block mode** (WordStar `^KN`) — in scope now or a later concern?

---

## 5. Role Cursors, Panes & Modals

### Background-pane cursor

**Settled (with a maybe):** in a split/multi-pane layout the unfocused pane shows **no caret by default**. An optional *hollow block* may be offered as an "inactive position" indicator — off by default, available as a setting. The software path makes this inactive look fully controllable.

### Modal / overlay behaviour

**Settled:** when a modal or overlay appears, **all painted cursors and markers in the background editor vanish** — the live writing caret and any persistent block markers underneath disappear cleanly while the modal owns focus. They reappear, unchanged, when the modal closes (markers return because their logical positions persist; nothing about the document state was lost, only its rendering).

The modal's own input field (search, command palette, go-to-line, save-as) **uses the configured writing-cursor style**, so the writer's chosen cursor follows them into every input context — a small but strong cohesion win.

| Context | Cursor shown | Style source |
|---|---|---|
| Focused editor pane | Writing caret (live) | User configuration |
| Unfocused pane | None by default; optional hollow block | Built-in role look |
| Begin / end block marker | Reversed `[` / `]` | Theme colour, fixed brightness |
| Under an open modal | Nothing — all background cursors/markers hidden | — |
| Modal input field | Writing caret | User configuration (inherited) |

---

## 6. Wider Cursor Uses — Census

Beyond the writing caret and block markers, a prose editor shows cursor-like feedback in several other places. The items below round out the system so nothing is designed into a corner later. Each is marked with its brainstorm status; the ones marked **team decides** were deferred for the implementation team to place in or out of scope.

| Feature | What it is | Status |
|---|---|---|
| **Writing caret** | The configurable live cursor. | In scope — settled |
| **Block markers** | Persistent reversed `[` and `]` landmarks. | In scope — settled |
| **Background-pane cursor** | Position in an unfocused split pane. | In scope — off by default |
| **Modal input caret** | Cursor inside search / palette / dialogs. | In scope — inherits writing cursor |
| **Scroll / off-screen indicator** | Gutter or edge marker when the caret (or a block marker) is off-screen. Merges with off-screen markers (§4). | Team decides |
| **Find / match highlight** | Search hits — a "where things are" marker adjacent to the cursor family. | Team decides — same theme family or separate system? |
| **Multi-cursor / multiple carets** | Simultaneous edit points. If ever on the roadmap, the painted model should reserve for it now. | Team decides — roadmap? |
| **Ghost / completion caret** | Preview caret for inline AI or snippet completions (relevant given the CLI fiction-agent work). | Team decides — roadmap? |
| **Idle / away state** | After N seconds of stillness the caret dims or stops blinking — gentle for a staring-and-thinking prose tool. | Team decides |

---

## 7. Making It Cohesive

The whole system reads as one thing only if a few principles hold across every cursor. These are the design invariants to protect during implementation:

1. **One painting model.** If Fork A resolves to software cursors, every visible cursor and marker is a painted cell drawn by the same subsystem, themed from the same palette. No hardware cursor survives in the default path (it is hidden at startup and used only in the accessibility mode).
2. **One theme source.** All cursors and markers draw their colour from the active theme. The writer configures only the writing caret's shape/brightness/blink; role cursors derive fixed, distinct looks from the same palette so they are recognisably siblings.
3. **Distinct-but-related roles.** Each role cursor must be visually self-describing (begin vs. end, active vs. inactive) while still belonging to the family. Asymmetric marker glyphs and the reserved hollow-block inactive look both serve this.
4. **Logical anchoring.** Persistent things (block markers) anchor to document positions, not screen cells, so they survive edits and scrolling and can be signalled when off-screen.
5. **Focus discipline.** Only the focused context shows a live cursor. Unfocused panes go quiet; a modal blanks the background entirely; the modal's own field carries the writer's configured caret forward.
6. **One collision rule.** Overlaps (caret on a marker) resolve one consistent way — out-of-phase alternate blink — rather than ad-hoc per feature.

---

## 8. Decision Log & Next Steps

### Settled in the brainstorm

- One configurable writing cursor; all other cursors are built-in and role-signifying.
- Block markers follow the WordStar model: persistent reversed `[` / `]` landmarks, cleared explicitly, no region fill before both are down.
- Markers are display-only insertions that shift text right (Fork B settled).
- Caret-on-marker overlap resolves via out-of-phase alternate blink.
- Unfocused pane: no caret by default (optional hollow block).
- A modal hides all background cursors and markers; the modal's input uses the configured writing cursor.

### Open forks & questions for the team

1. **Fork A — resolve hardware vs. software vs. hybrid cursor** (gates the picker style, brightness, blink implementation, and the whole rendering model). Consider software-primary with a hardware accessibility fallback.
2. **Confirm the shape menu** once Fork A is chosen (block/underline/bar vs. the richer painted set).
3. **Verify marker count** (one pair vs. multiple) against the existing code.
4. **Decide column/rectangular block mode** in or out of scope.
5. **Place the census items** — off-screen indicator, find/match highlight, multi-cursor, ghost/completion caret, idle state — in or out of scope.
6. **Confirm the layout-layer plan** for display-only injected columns (offset mapping, word-wrap, ruler math).

---

*End of concept document. Prepared as a discussion brief; every "team decides" item is intentionally left open for evaluation against the codebase.*
