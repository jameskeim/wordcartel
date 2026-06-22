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

### 3.10 Buffer = `ropey` (revises the earlier "port kiro's line buffer" plan)
- **Decision:** Use the **`ropey`** rope crate (MIT/Apache-2.0) as the editable
  buffer, **replacing** kiro's `Vec<Row>` line-vector. Pin `ropey = "1.6.1"`
  (mirroring Helix) until 2.0 stabilizes. Compose with `unicode-segmentation`
  (graphemes) and `unicode-width` (display width).
- **Rationale:** kiro's line-vector is tuned for *code* (short lines); a prose
  paragraph is one very long logical line, where mid-paragraph edits cost
  O(line-length) — a direct violation of §3.9. A rope is O(log n) for edits
  anywhere in the document regardless of line length. `ropey` specifically wins
  over faster ropes (`crop`, `jumprope`) because it provides the
  `byte↔char↔line` conversion API the cursor/column-map work depends on; the
  others omit it. It's the de-facto standard (used by Helix) and permissively
  licensed.
- **Consequence:** This is the one place prior-art research overturned an earlier
  decision. It *strengthens* the "reuse well-written code" goal (a purpose-built,
  widely-used crate instead of a hand-maintained line buffer), but it shrinks
  kiro's role — see §7 and the prior-art section §9.

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

**Net effect of the ratatui + prior-art decisions:** kiro's role shrinks further,
from "port the core" to **structural reference**. The io/shell layer is provided
by ratatui/crossterm; the buffer is now `ropey` (§3.10); undo and selection are
reimplemented from the Helix/CodeMirror patterns (§9). What remains genuinely
kiro-derived is the **editor loop shape** and the *idea* of diff-based undo. This
is not a retreat from "reuse well-written code" — it is the opposite: research
found better-maintained, better-fit components (ropey, regex-cursor, arboard,
pulldown-cmark, floem_editor_core) than a solely-owned port of kiro would be. See
§9 for the full component stack and licensing.

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

## 9. Prior Art & Borrowed Components

Findings from a deep prior-art scan (Rust and non-Rust editors), filtered through
a license lens: **depend** = add as a crate dependency; **copy** = paste
permissively-licensed source (with attribution); **reimplement** = study a
copyleft/other-language design and write our own. groundwords is MIT, so copyleft
sources (GPL/MPL) are *pattern-only*.

### 9.1 Component stack

| Need | Choice | License | How |
|---|---|---|---|
| Text buffer | `ropey` 1.6.1 | MIT/Apache | depend (§3.10) |
| Grapheme / width | `unicode-segmentation`, `unicode-width` | MIT | depend |
| Undo/redo | ChangeSet (Retain/Delete/Insert) + branching history; `smartstring` for inserts | reimpl (from Helix MPL + CodeMirror MIT patterns) | reimplement |
| Selection | single user-facing selection; `SmallVec<[Range;1]>`; anchor+head over byte offsets | Apache (floem) / MPL (helix) | copy `floem_editor_core` **or** reimplement |
| Clipboard | `arboard` (+`wayland-data-control`) | MIT/Apache | depend |
| Clipboard fallback | OSC 52 via crossterm `osc52` feature | MIT | depend (already have crossterm) |
| In-document search | `regex-cursor` (+`regex-automata`) | MIT/Apache | depend |
| Filter subprocess | `subprocess` crate, or `std::thread`+`mpsc` | MIT/Apache | depend / hand |
| Markdown parse | `pulldown-cmark` (`into_offset_iter()`) | MIT | depend |
| Live-preview model | CodeMirror 6 decoration + atomicRanges | MIT (JS) | reimplement |
| Cursor-line reveal | render-markdown.nvim / Vim conceal `concealcursor` | MIT / Apache | reimplement (confirms model) |
| Soft-wrap + column map | Helix `DocumentFormatter` design | MPL | reimplement |

### 9.2 Key technical decisions from the research
- **Undo coalescing is prose-tuned:** group keystrokes into undo units by a time
  threshold (~500 ms burst), and break groups on paste / programmatic edits /
  explicit cursor moves. Do **not** break per word (annoying in prose). Store the
  inverse changeset at commit time (no need to keep old buffer copies).
- **Selection stored in buffer coordinates, not visual:** ranges are byte/char
  offsets over the rope; visual-row spans are computed at render time, so
  selection survives viewport/width changes.
- **Search without whole-doc allocation:** `regex-cursor` runs the regex engine
  directly over rope chunks; `find_next`/`prev` resume from the cursor offset
  rather than rescanning from 0; highlight-all is **viewport-gated**.
- **Markdown parser:** `pulldown-cmark` over `comrak` (which gives line:col, not
  byte offsets for delimiters) and over `tree-sitter-md` (maintainers warn it is
  inaccurate for inline markdown). tree-sitter, if used at all, only for finding
  block boundaries.
- **Incremental reparse:** find the dirty block via blank-line (`\n\n`) scan up/down
  from the edit, reparse only that block, cache inactive blocks by content hash.
  The active line shows raw markdown (no parse), and inactive lines don't change
  while typing the active line — so per-keystroke parse cost is tiny. **Edge case:**
  setext headings (`Foo` over `===`) need two-line context.
- **`col_map` = atomicRanges analog:** during the grapheme render pass, every
  visible grapheme pushes its source byte offset (once per visual column);
  concealed marker bytes push nothing. Cursor at visual column C → source byte
  `col_map[C]`, which makes arrow keys skip hidden markers for free.

### 9.3 Runtime note: no async runtime required
The filter/subprocess work does **not** require tokio. The synchronous
draw-on-event loop (§3.8/§3.9) plus `std::thread` + `mpsc` (or the `subprocess`
crate's deadlock-safe `Communicator`) runs filters off the hot path while keeping
the loop free. This preserves the lean, low-latency design and avoids the async
dependency weight that counted against bubbletea-rs (§8.1).

### 9.4 Differentiators surfaced by the research
Two responsiveness wins where even Helix (the leading Rust editor) falls short, and
which groundwords should do well from v1:
- **Highlight *all* search matches while typing** (viewport-gated). Helix only
  jumps to the first match (long-standing open request).
- **Always-async filters** with instant "running `…` ⏳" feedback. Helix's pipe
  family runs synchronously and blocks its UI.

### 9.5 Clipboard / terminal gotchas to honor (implementation-time)
- Treat the system clipboard as **optional** — never `unwrap`; fall back
  arboard → OSC 52 → silent no-op (covers SSH, headless, GNOME-Wayland).
- Handle **bracketed paste** as a single `Event::Paste(String)` (crossterm), not
  N keystrokes; spawn large pastes off the loop with a "pasting…" indicator.
- Offer both CLIPBOARD and PRIMARY (X11 middle-click) selections on Linux.

### 9.6 Provenance / attribution to carry
Beyond kiro's MIT notice (§6): preserve attribution for any **copied** code (e.g.
`floem_editor_core`, Apache-2.0) and record that undo/selection/soft-wrap/live-
preview designs are **reimplementations** inspired by Helix (MPL-2.0) and
CodeMirror 6 (MIT) — patterns, not copied source.

---

## 10. Status / Open Threads (still to design)

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
