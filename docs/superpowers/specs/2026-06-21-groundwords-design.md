# groundwords — Design Document

**Status:** Living document — design in progress
**Last updated:** 2026-06-22

> A terminal-native, markdown-first word processor written in Rust, with live
> (Obsidian-style) preview, Unix-pipe extensibility, and pandoc-powered export.

---

## 1. Overview

**groundwords** is a distraction-free terminal word processor for writing prose.
Its native document format is Markdown, it renders that Markdown *live* while you
edit (styled text with concealed markers), and it leans on existing Unix tooling
— pandoc for format conversion, and arbitrary CLI filters — instead of building
those capabilities in-core.

It is inspired by [WordGrinder](https://github.com/davidgiven/wordgrinder) (the
distraction-free terminal-word-processor feel) but takes a fundamentally
different, simpler path: where WordGrinder invents a bespoke document model and a
suite of in-house exporters, groundwords makes **plain Markdown text** the source
of truth and delegates conversion to **pandoc**.

The implementation borrows heavily from
[kiro-editor](https://github.com/rhysd/kiro-editor) (MIT), a modular Rust port of
antirez's `kilo`.

---

## 2. Goals & Non-Goals

### Goals
- **Markdown-first.** The on-disk format is plain UTF-8 `.md`. No custom/binary
  format, no migration story, fully git- and tool-friendly.
- **Live preview while editing.** Styled rendering with concealed markup, in the
  terminal, for a true word-processor feel rather than a code-editor feel.
- **Responsiveness is a first-class goal.** Typing must feel instant; no
  perceptible lag on keystrokes, and no silent waits on UI actions. Lag is a
  drag on thinking. (See §3.9 for the latency principles this implies.)
- **Distraction-free.** Word count, focus mode; minimal chrome.
- **Unix-native extensibility.** Pipe the buffer (or a selection) through any CLI
  tool and bring the result back. Pandoc export and spellcheck are *presets* of
  this one mechanism.
- **Reuse over reinvention.** Port as much of kiro as the design allows.

### Non-Goals (v1)
- Not a code editor / IDE (no LSP, no multi-language syntax highlighting).
- No bespoke export converters — pandoc owns conversion.
- No bundled conversion logic (pandoc is an external runtime dependency).
- Embedding/zero-dependency minimalism is **not** a goal (kibi's <1024-line
  constraint was explicitly rejected as a base — see §8).

---

## 3. Key Decisions & Rationale

### 3.1 Markdown is the source of truth; pandoc does conversion
- **Decision:** Buffer holds raw Markdown text; save writes `.md`; export shells
  out to pandoc.
- **Rationale:** Eliminates WordGrinder's two hardest chunks (a custom document
  format and a pile of exporters). Files stay diffable and portable. Pandoc gives
  ~40 output formats (docx, pdf, odt, html, latex…) for zero conversion code.
- **Consequence:** Pandoc is a runtime dependency, detected at startup; export
  degrades gracefully when it is absent. Core editing never depends on it.

### 3.2 Editing model: live + concealed, whole-active-line reveal
- **Decision:** Markup renders styled (bold shows bold, headings styled); the
  raw markers are concealed *except* on the line the cursor is currently on,
  which shows full raw Markdown. (Obsidian live-preview family; reveal scope =
  whole active line.)
- **Rationale:** Best writing feel of the options considered, while
  whole-line reveal keeps conceal state to a single boolean per row
  ("is this the cursor's line?"), dramatically simplifying the trickiest module.
- **Alternatives rejected:**
  - *Source view* (always raw, like a code editor): simplest, but loses the
    word-processor feel that motivates the project.
  - *Element-under-cursor reveal*: best feel, but requires per-element conceal
    tracking in the column map — deferred to backlog; the architecture allows
    tightening to this later without restructuring.
  - *Full WYSIWYG* (markers never shown, edit a rendered tree): hardest cursor
    model, lossy, drifts away from Markdown transparency.

### 3.3 Source of truth is text, not an AST
- **Decision:** The buffer is plain Markdown text. The parse tree is a *derived
  view* recomputed for rendering — we never edit an AST and re-serialize.
- **Rationale:** Keeps editing, undo, and save trivially simple (it is all text).
  Lets kiro's text-buffer + diff-based undo port over intact. Avoids
  round-trip/serialization loss.

### 3.4 Soft word-wrap is mandatory
- **Decision:** A paragraph is one logical line that soft-wraps to the viewport
  width; one logical line maps to *N* visual rows.
- **Rationale:** This is a prose tool. Code editors (kiro/kibi) scroll
  horizontally instead — this is the one place their model genuinely does not
  carry over.
- **Consequence:** The rendering layer must map logical-line → multiple visual
  rows, and this composes with conceal (both transform a line before it reaches
  the screen).

### 3.5 The `filter` primitive: pipe buffer/selection through external tools
- **Decision:** A single mechanism spawns a subprocess, feeds it text on stdin,
  and routes stdout one of three ways. Pandoc export and spellcheck are presets.

  | Disposition | stdin | stdout → | Undoable | Examples |
  |---|---|---|---|---|
  | **Filter** | selection (else whole buffer) | replaces that range | yes | `fmt -w 80`, `sort`, `aspell`, an LLM CLI |
  | **Insert** | (none) | inserted at cursor | yes | `date`, `cat snippet.md` |
  | **Export** | whole buffer | a file on disk | no (read-only) | `pandoc -o out.docx` |

- **Rationale:** Unix-editor tradition (vim `!`, Acme pipe, Kakoune `|`).
  Pandoc export and "pipe through a tool" are the *same* operation, so they share
  one well-tested module. Turns groundwords into a platform: spellcheck, reflow,
  table formatting, grammar/translation/AI — all without in-core features.
  Architecturally cheap because the subprocess plumbing was needed for pandoc
  regardless.
- **Open considerations:** runs arbitrary shell (acceptable for a personal tool;
  note it); capture stderr to surface errors; guard against very large output;
  make long-running filters cancellable.

### 3.6 Selection + system clipboard
- **Decision:** Range selection (anchor + cursor); copy/cut/paste via the
  `arboard` crate (X11 + Wayland) with an **OSC 52** terminal fallback so
  copy/paste survives over SSH.
- **Rationale:** Selection is a prerequisite for filter-on-selection. Clipboard
  integration is net-new (kiro had none).

### 3.7 Borrow kiro; rewrite only the rendering path
- **Decision:** Port kiro modules wholesale where they carry over; rewrite only
  what the live-preview + soft-wrap model forces. See §7 reuse map.
- **Rationale:** De-risks the build. The genuinely new code concentrates in the
  areas already identified as novel.

### 3.8 Rendering foundation: ratatui (rendering) + kiro core (editing)
- **Decision:** Depend on **ratatui** for the rendering/terminal layer and chrome
  widgets; **borrow kiro's core** (text buffer, diff-based undo, input parsing) by
  copying it in; drive **our own** kiro-style input loop. **crossterm** (which
  ratatui builds on) handles raw mode + key events.
- **Why these are not the same kind of thing:** kiro is an *application* whose
  source we copy and own; ratatui is a *maintained library* we depend on. They
  overlap only at the rendering layer. Borrowing kiro's core and depending on
  ratatui for rendering maximizes "well-written code we didn't write" across
  *both* layers.
- **Rationale:**
  - ratatui is the most mature, battle-tested TUI renderer in Rust — better-tested
    than a `screen.rs` we would solely maintain — and its **cell-diffing** emits
    escape sequences only for *changed cells*, so its "redraw the whole frame"
    model is full-frame in code but minimal in bytes. Plenty of very snappy TUIs
    are ratatui apps; it is not a latency risk (see §3.9).
  - Crucially, **ratatui does not own the event loop** (unlike bubbletea-rs's Elm
    runtime). We keep our own loop, so it composes cleanly with kiro's
    architecture and the custom editing surface.
  - kiro still supplies what ratatui cannot: the text buffer and the diff-based
    undo/redo.
  - ratatui's widgets make instant UI feedback (highlights, spinners, status)
    trivial to render — directly serving §3.9.
- **Alternative rejected — bubbletea-rs (+ lipgloss/glamour):** an Elm/MVU
  framework that *owns the loop*, requires Tokio, redraws full frames without the
  cell-diff guarantee, and was v0.0.9 (immature). It would supersede the kiro
  skeleton and put responsiveness at risk; lipgloss's strength (rich chrome) is
  largely wasted on a deliberately chromeless tool, and glamour cannot drive the
  editing surface (no source↔cursor mapping). Useful only as a styling *reference*.
- **Alternative rejected — fully hand-rolled screen (kiro `screen.rs`):** maximal
  byte-level control and leaner deps, but we would solely maintain the renderer
  and build all chrome from scratch, duplicating what ratatui's cell-diffing
  already does well.

### 3.9 Responsiveness / latency principles
Responsiveness is a first-class goal (§2). The rendering foundation does not
decide it — *our* architecture does. These principles are non-negotiable:
1. **Per-keystroke work is O(visible screen), not O(document).** Parsing is
   **incremental and region-scoped**: a keystroke re-parses only the current
   block/paragraph and re-renders only the visible viewport. A 200-page document
   must type as fast as a 1-page one. (Constrains `md_parse` and `layout`.)
2. **Never block the input loop.** Filters, pandoc, spellcheck, and any file I/O
   run off the hot path as subprocesses; the loop stays free to accept input.
3. **Draw synchronously on every input event** — never on a timer/debounce.
4. **Paint feedback the instant an action starts**, before doing the work
   (highlight/spinner/status). The WordGrinder frustration being designed out is
   not that an action takes a moment, but that it gives *no feedback* while it
   does — you wait in silence. Every action acknowledges immediately.

---

## 4. Architecture

Three layers: a pure, terminal-free **core** (unit-testable in isolation); a thin
side-effecting **io/shell** layer (now built on **ratatui**/**crossterm**); and the
**app** wiring. Provenance is marked: ✅ ported from kiro · 🟡 adapted from kiro ·
🟢 provided by ratatui/crossterm · 🔴 net-new.

```
groundwords/
├── core (no terminal/IO deps — pure, unit-testable)
│   ├── text_buffer   logical lines, edit ops, UTF-8           ✅ kiro
│   ├── edit_diff     edits as reversible diffs                ✅ kiro
│   ├── history       undo/redo stack over edit_diff           ✅ kiro
│   ├── selection     anchor+cursor range, clipboard ops       🔴 new
│   ├── md_parse      pulldown-cmark → styled/marker spans
│   │                 with source byte ranges                  🔴 new
│   ├── layout        logical line + conceal + soft-wrap → visual
│   │                 rows + column-map (screen col ↔ source)  🔴 new
│   └── filter        spawn subprocess, stdin/stdout, 3 modes  🔴 new
│
├── io / shell (side-effecting, thin — on ratatui/crossterm)
│   ├── input         raw mode, key events                     🟢 crossterm
│   │                 (key→action mapping is ours)             🔴 new
│   ├── render        build ratatui frame from layout's visual
│   │                 rows + styled spans; set cursor pos      🔴 new (thin)
│   │                 (cell-diff + terminal writes)            🟢 ratatui
│   ├── clipboard     arboard + OSC 52 fallback                🔴 new
│   └── color/style   styling + truecolor handling             🟢 ratatui
│
└── app
    ├── editor        lifecycle + our own input loop, owns state 🟡 kiro shape
    ├── commands      keybinds → actions (incl. filter presets)  🔴 new
    └── config        keybinds, theme, pandoc path, presets      🔴 new
```

**Loop ownership:** ratatui is immediate-mode and does **not** own the loop. Our
`editor` runs a kiro-style loop: read crossterm event → update buffer (kiro) →
incremental re-parse of the edited block (`md_parse`) → recompute affected visual
rows (`layout`) → `terminal.draw()` (ratatui cell-diffs and emits only changed
cells). This is the path that must stay O(visible screen) per keystroke (§3.9).

### The hard module: `layout`
`layout` is where conceal, soft-wrap, and the **screen↔source column map**
converge. It is a *pure function* — `(logical line text, is-active-line, viewport
width) → (visual rows, column map)` — with zero terminal involvement, so it is
heavily unit-testable. The column map exists because, when markers are concealed,
*visual column ≠ source byte offset*: pressing → past visible `bold` must move the
cursor over the hidden `**`. This module is expected to host most of the editor's
subtle bugs and therefore gets the most test attention.

### Rendering depends on the cursor
A row's rendering is **not** a pure function of the buffer alone — it depends on
cursor position (the active line reveals raw markup). Moving the cursor
invalidates **two** logical lines: the one left (re-conceal) and the one entered
(reveal). With ratatui we rebuild the frame and let its cell-diff emit only the
cells that actually changed, so cursor-move redraws stay cheap without us
hand-managing a dirty-row list.

### Parser choice
`pulldown-cmark` (pull parser, CommonMark/GFM, events carry **source offsets**) —
needed so the renderer knows exactly which bytes are markers vs content. Also
close to what pandoc ingests, so what you see maps to what exports.

---

## 5. Scope

### v1 (must-have)
- Core editing: open/save `.md`, live-concealed rendering, soft-wrap, cursor
  movement, insert/delete, **undo/redo**.
- **Selection + clipboard** (copy/cut/paste).
- **Incremental search** (find; find/replace).
- **`filter` primitive** with **pandoc export** and **spellcheck** as presets.
- **Writing aids:** word/char count, distraction-free / focus mode.

### Backlog (post-v1)
- Element-under-cursor reveal (tighter conceal granularity).
- Bundled spellcheck UX beyond the `aspell` filter preset.
- Multiple buffers / windows.
- Richer block styling (tables, footnotes, task lists rendering).
- Configurable themes beyond the default.

---

## 6. Licensing

groundwords is **MIT**, compatible with kiro's MIT.

**Obligation:** every file ported from or derived from kiro must retain rhysd's
copyright and the MIT permission notice. Practically:
- a credit header at the top of each ported/derived file, and
- kiro's full `LICENSE` text preserved at `licenses/kiro-MIT.txt`.

This is baked into the design so it is handled during implementation, not as an
afterthought.

---

## 7. kiro Reuse Map

| kiro module | Verdict | Notes |
|---|---|---|
| `text_buffer.rs` | ✅ Port ~as-is | Text storage + UTF-8 metadata = our core of truth. |
| `edit_diff.rs` | ✅ Port wholesale | Reversible-diff edits — undo backbone. |
| `history.rs` | ✅ Port wholesale | Undo/redo stack over edit_diff. |
| `editor.rs` | 🟡 Port loop shape | Our own loop; structure follows kiro; state struct grows. |
| `row.rs` | 🟡 Salvage UTF-8 logic | Keep width/byte-index caching; rendering moves to `layout`. |
| `input.rs` | 🟢→ crossterm | Replaced by crossterm events; we keep only key→action mapping. |
| `prompt.rs` | 🟢→ ratatui | Bottom-line dialogs become ratatui widgets (search, save-as, filter prompt). |
| `term_color.rs` | 🟢→ ratatui | Styling/color handled by ratatui. |
| `screen.rs` | 🟢→ ratatui | Rendering + cell-diff handled by ratatui; we build the frame. |
| `signal.rs` | 🟢→ crossterm | Resize via crossterm's resize events. |
| `highlight.rs` | 🔴 Replace → `md_parse` | Different job: markdown spans + source ranges. |
| `language.rs` | ⚪ Drop | Markdown-only; no filetype detection. |

**Net effect of the ratatui decision:** the borrowed-from-kiro set narrows to the
**pure core** (`text_buffer`, `edit_diff`, `history`, `row`'s UTF-8 logic, and the
`editor` loop shape) — the genuinely valuable, hard-to-find code. The entire
io/shell layer is now provided by maintained dependencies (ratatui/crossterm)
rather than hand-ported, which is *more* "well-written code we didn't write," not
less.

---

## 8. Paths Not Taken

### 8.1 Why not bubbletea-rs (for rendering)?
[bubbletea-rs](https://github.com/whit3rabbit/bubbletea-rs) is a Rust port of Go's
Bubble Tea — Elm/MVU, Tokio-async, with excellent styling (lipgloss-extras) and
markdown rendering (glamour). Rejected as a *foundation* (kept as a styling
*reference*) because: it **owns the event loop** (replacing the kiro skeleton),
pulls in Tokio, redraws full frames without ratatui's cell-diff guarantee, and was
v0.0.9/immature — all at odds with the responsiveness goal (§3.9). Its strongest
asset (rich chrome) is largely wasted on a deliberately chromeless tool, and
glamour cannot drive the editing surface (no source↔cursor mapping). See §3.8.

### 8.2 Why not kibi?

[kibi](https://github.com/ilai-deutel/kibi) is the more polished, better-maintained,
more feature-complete *editor* — but its defining <1024-line constraint makes it a
poor *base*: the code is deliberately golfed, so extending it fights the line limit
and breaks the one thing that makes it special. kiro was built for extension
(modular split, diff-based undo already present, I/O abstracted over traits for
testing), which is why it is the foundation despite being less actively maintained.
Since we fork to own it, kiro's stale-but-well-structured base beats kibi's
active-but-constrained one.

---

## 9. Status / Open Threads (still to design)

- **§ Data flow** — trace a keystroke buffer → `md_parse` → `layout` → `screen`;
  worked example of the column map for a concealed, soft-wrapped line.
- **§ Error handling** — pandoc/filter failures, missing binaries, subprocess
  cancellation, save errors.
- **§ Testing strategy** — `layout` unit tests, golden-render tests, undo/redo
  property tests, filter integration tests.
- **§ Config format** — keybindings, theme, pandoc path, filter presets.
- **§ Keybinding scheme** — default bindings; modal vs modeless.
- **§ md_parse details** — which CommonMark/GFM constructs are concealed/styled
  in v1.
