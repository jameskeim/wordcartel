# Wordcartel — Design Document

**Status:** Design-complete; red-teamed (Codex) — hardening in progress (see §16)
**Last updated:** 2026-06-22

> A terminal-native, markdown-first word processor written in Rust, with live
> (Obsidian-style) preview, Unix-pipe extensibility, and pandoc-powered export.

**The name** (renamed from "groundwords"): *Wordcartel* — chosen because it
doesn't take itself too seriously and because it winks at how much this project
*borrows* from other great editors and word processors. We run a small cartel over
our word "supply chain" (ropey, pulldown-cmark, regex-cursor, arboard, and the
design patterns of Helix, CodeMirror 6, Kakoune, and kiro). See §9.
**Binary/command name:** `wc` is unavailable (it is the Unix word-count tool —
ironic for a word processor), so the CLI command is **`wcartel`** (default
recommendation; `cartel` is a candidate alias). Crate/package name: `wordcartel`.

---

## 1. Overview

**Wordcartel** is a distraction-free terminal word processor for writing prose.
Its native document format is Markdown, it renders that Markdown *live* while you
edit (styled text with concealed markers), and it leans on existing Unix tooling
— pandoc for format conversion, and arbitrary CLI filters — instead of building
those capabilities in-core.

It is inspired by [WordGrinder](https://github.com/davidgiven/wordgrinder) (the
distraction-free terminal-word-processor feel) but takes a fundamentally
different, simpler path: where WordGrinder invents a bespoke document model and a
suite of in-house exporters, Wordcartel makes **plain Markdown text** the source
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
- **Rationale:** Keeps editing, undo, and save trivially simple (it is all text) —
  edits are plain text edits over the `ropey` buffer (§3.10), and undo is a
  reimplemented ChangeSet/history (§9.1), not tied to any AST. Avoids
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
- **Hard-wrap is an I/O concern, not a second editing mode** — see §14.2 (soft-only
  editing + a wrap-guide ruler; reflow/unwrap/ventilate at the edges via repar).

### 3.5 The `filter` primitive: pipe buffer/selection through external tools
- **Decision:** A single mechanism spawns a subprocess, feeds it text on stdin,
  and routes stdout one of three ways. Pandoc export and the repar transforms (§14)
  are presets. **Spellcheck is NOT a filter** — `aspell list` etc. emit a *word
  list*, not corrected text; spellcheck is a **diagnostic command/Effect** (§10.4)
  that produces markers/diagnostics, never a buffer replacement.

  | Disposition | stdin | stdout → | Undoable | Examples |
  |---|---|---|---|---|
  | **Filter** | selection (else whole buffer) | replaces that range | yes | `fmt -w 80`, `sort`, `prettier --parser markdown`, an LLM CLI |
  | **Insert** | (none) | inserted at cursor | yes | `date`, `cat snippet.md` |
  | **Export** | whole buffer | see note | no (read-only) | `pandoc -o out.docx` |

  *Export note:* a preset declares **either** `capture` (we read the child's stdout
  and write it to a chosen path) **or** `writes_output` (the child writes its own
  file, e.g. pandoc `-o`; we pass the path as an arg and capture only stderr/exit).
  The export contract (input flags, output-path selection, front-matter, resource
  paths) is an implementation-spec item.
- **Rationale:** Unix-editor tradition (vim `!`, Acme pipe, Kakoune `|`).
  Pandoc export and "pipe through a tool" are the *same* operation, so they share
  one well-tested module. Turns Wordcartel into a platform: reflow, table
  formatting, grammar/translation/AI — all without in-core features. Architecturally
  cheap because the subprocess plumbing was needed for pandoc regardless.
- **Security/robustness model** (to be fully specified in the hardening pass —
  Codex red-team #8): default to **argv arrays** (no implicit shell); an explicit
  `shell = true` opt-in per preset for pipelines; validate stdout is UTF-8 (reject
  binary); cap output size (stream to temp beyond a threshold); default timeout +
  Esc cancellation (`kill`); deadlock-safe concurrent stdin/stdout (§9.2). The
  buffer is replaced **only** after a successful, fully-collected, valid result.

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

### 3.10 Buffer = `ropey` (LOCKED — affirmed after re-examination)
- **Decision:** Use the **`ropey`** rope crate (MIT/Apache-2.0) as the editable
  buffer, **replacing** kiro's `Vec<Row>` line-vector. Pin `ropey = "1.6.1"`
  (mirroring Helix) until 2.0 stabilizes. Compose with `unicode-segmentation`
  (graphemes) and `unicode-width` (display width). **Decision status: locked.**
- **Rationale:** kiro's line-vector is tuned for *code* (short lines); a prose
  paragraph is one very long logical line, where mid-paragraph edits cost
  O(line-length) — a direct violation of §3.9. A rope is O(log n) for edits
  anywhere in the document regardless of line length. `ropey` specifically wins
  over faster ropes (`crop`, `jumprope`) because it provides the
  `byte↔char↔line` conversion API the cursor/column-map work depends on; the
  others omit it. It's the de-facto standard (used by Helix) and permissively
  licensed.
- **Re-examined against gap buffers and piece tables** (the Apple Writer / Emacs
  lineage) and affirmed. Findings:
  - *Gap buffer:* superb O(1) localized typing and proven for prose, but provides
    no cheap immutable snapshot — handing the buffer to a worker means an O(N)
    copy or locking, which fights our sync-core/async-edges concurrency model
    (§10.3: background spellcheck, full-doc search, non-blocking save). Its
    historical necessity came from 1979 hardware limits (1 MHz / 64 KB) that no
    longer bind.
  - *Piece table/tree:* a conceptual middle ground, but to match ropey's O(1)
    snapshot + O(log n) line indexing it must be built as a persistent augmented
    balanced tree — i.e. it converges to a rope's complexity while losing ropey's
    maturity, `regex-cursor` integration, and memory reclamation (its append-only
    add-buffer retains deleted text). No battle-tested Rust crate exists.
  - *The article's "ropes stutter while typing" claim* describes a naive
    per-character rope, not `ropey` (a B-tree of ~1 KB string chunks: in-chunk
    `memcpy` per keystroke, rare splits, ~10–17% overhead). **Empirically refuted
    by Helix** — the leading ropey-based editor, specifically celebrated for
    responsive typing. Its rare slowdowns trace to tree-sitter highlighting and
    long-line *rendering*, not the rope — confirming §10.5's point that latency
    lives in the parse/render path, not the buffer.
  - *Undo is decoupled* (§9/§10 ChangeSet+history), so the gap-buffer/piece-table
    "undo" tradeoffs the popular comparisons emphasize do not apply to us.
- **Consequence:** This is the one place prior-art research overturned an earlier
  decision. It *strengthens* the "reuse well-written code" goal (a purpose-built,
  widely-used crate instead of a hand-maintained line buffer), but it shrinks
  kiro's role — see §7 and the prior-art section §9.

### 3.11 Render modes: a pure view toggle (live-preview / source-highlighted / source-plain)
- **Decision:** Three *rendering* modes over the single buffer, cycled by a toggle:
  1. **Live preview** (default) — concealed markdown (§3.2).
  2. **Source (highlighted)** — raw markdown, markers visible + light syntax color,
     conceal off (the classic "markdown source" look).
  3. **Source (plain)** — literal raw text, zero styling; the fastest render.
- **A pure rendering toggle — not a change to the text, cursor, or input mode.**
  Buffer, selection, and undo are identical across modes; only `layout` output
  changes. This is the *safe* kind of dual view — contrast the rejected hard/soft
  dual *editing* mode (§14.2), which would mutate text and need two cursor models.
- **Nearly free in our architecture:** the active line already renders raw with an
  identity `col_map` (§10.5); source modes simply render *every* line that way.
  `layout` takes a `render_mode` (held in `View`); in source modes conceal is
  skipped and `col_map` is identity throughout — strictly *cheaper* than live
  preview (the "fast view").
- **Value:** precise markup surgery (links, tables, front matter, escapes);
  transparency for markdown-first users; **a robust fallback that de-risks the
  conceal engine** — source view always shows ground truth if `layout` mis-renders;
  and the fastest possible render on huge documents or weak terminals.

---

## 4. Architecture

Three layers: a pure, terminal-free **core** (unit-testable in isolation); a thin
side-effecting **io/shell** layer (now built on **ratatui**/**crossterm**); and the
**app** wiring. Provenance is marked: ✅ ported from kiro · 🟡 adapted from kiro ·
🟢 provided by a crate dependency (ratatui/crossterm/ropey) · 🔴 net-new (incl.
reimplemented-from-pattern). *(kiro's role is structural reference only — §7/§9.)*

```
wordcartel/
├── core (no terminal/IO deps — pure, unit-testable)
│   ├── rope_buffer   ropey wrapper: edit ops, byte↔char↔line  🟢 ropey + 🔴 wrapper
│   ├── edit_diff     ChangeSet (retain/delete/insert)         🔴 reimpl (Helix/CM)
│   ├── history       undo/redo over ChangeSet                 🔴 reimpl (Helix/CM)
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
cursor position (the active line reveals raw markup) and on the `View`'s
**`render_mode`** (§3.11): in the two source modes, conceal is skipped for *all*
lines and `col_map` is identity (the active-line path generalized to the whole
document). Moving the cursor invalidates **two** logical lines: the one left
(re-conceal) and the one entered (reveal). With ratatui we rebuild the frame and
let its cell-diff emit only the cells that actually changed, so cursor-move redraws
stay cheap without us hand-managing a dirty-row list.

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
- **repar transforms** (in-process): reflow / unwrap / ventilate commands; explicit
  **unwrap-on-import**; **reflow/ventilate export**; **wrap-guide ruler** (§14).
- **Render-mode toggle** (§3.11): live-preview / source-highlighted / source-plain.

### Backlog (post-v1)
- Element-under-cursor reveal (tighter conceal granularity).
- Bundled spellcheck UX beyond the `aspell` filter preset.
- Multiple buffers / windows.
- Richer block styling (tables, footnotes, task lists rendering).
- Configurable themes beyond the default.

---

## 6. Licensing

Wordcartel is **MIT**, compatible with kiro's MIT.

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
| `text_buffer.rs` | 🔴 Replace → `ropey` | Superseded by the `ropey` rope (§3.10); kiro's `Vec<Row>` is structural reference only. |
| `edit_diff.rs` | 🔴 Reimplement | ChangeSet (retain/delete/insert) reimplemented from Helix/CM patterns (§9.1); kiro's enum is a reference, not ported. |
| `history.rs` | 🔴 Reimplement | Branching undo/redo over ChangeSet (§9.1), prose-tuned coalescing; kiro's stack is a reference. |
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
copyleft/other-language design and write our own. Wordcartel is MIT, so copyleft
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
| Reflow / unwrap / ventilate | `repar` (in-process) | MIT (authored by user) | depend (§14) |
| Atomic save | `repar::atomic` pattern | MIT (authored by user) | copy (§14.3) |
| Display width | `repar::width` (`unicode-width`) | MIT (authored by user) | copy/adapt (§14.3) |

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
- **Incremental reparse:** reparse only the dirty block and cache the rest by
  content hash; the active line shows raw markdown (no parse), so per-keystroke
  parse cost is small. **⚠ KNOWN DESIGN RISK (Codex red-team #4) — to be resolved
  in the hardening pass:** a naive blank-line (`\n\n`) scan is *not* safe for full
  CommonMark/GFM — fenced code spanning blanks, HTML blocks, lazy blockquote
  continuation, multi-paragraph list items, and link-reference definitions are
  context-sensitive across blanks. The resolution is either (a) parser-state /
  container-derived invalidation ranges (a real block tree), or (b) narrow v1
  markdown to locally-parseable constructs and render ambiguous cases as raw
  source. The incremental==full-reparse oracle test (§11.2) must cover **every**
  container construct, not just setext/lists. The layout spike will inform the choice.
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
which Wordcartel should do well from v1:
- **Highlight *all* search matches while typing** (viewport-gated). Helix only
  jumps to the first match (long-standing open request).
- **Always-async filters** with instant "running `…` ⏳" feedback. Helix's pipe
  family runs synchronously and blocks its UI.

### 9.5 Clipboard / terminal gotchas to honor (implementation-time)
- Treat the **system** clipboard as optional — never `unwrap`; fall back
  arboard → OSC 52 → (system sync unavailable). Copy/cut/paste **always** work via
  an **internal register**; only *syncing to the OS clipboard* degrades. So an
  unavailable system clipboard is a no-op *for OS sync*, not for editing — this is
  the precise meaning reconciled with §15.6.
- Handle **bracketed paste** as a single `Event::Paste(String)` (crossterm), not
  N keystrokes; spawn large pastes off the loop with a "pasting…" indicator.
- Offer both CLIPBOARD and PRIMARY (X11 middle-click) selections on Linux.

### 9.6 Provenance / attribution to carry
Beyond kiro's MIT notice (§6): preserve attribution for any **copied** code (e.g.
`floem_editor_core`, Apache-2.0) and record that undo/selection/soft-wrap/live-
preview designs are **reimplementations** inspired by Helix (MPL-2.0) and
CodeMirror 6 (MIT) — patterns, not copied source.

---

## 10. Data Flow & Control-Flow Architecture

The load-bearing skeleton, synthesized from a focused prior-art spike (CodeMirror 6,
Helix, the xi-editor retrospective, Zed/GPUI, ratatui). Principle: **functional
core, imperative shell** — a pure, synchronous editing core with one mutation
channel, wrapped by a thin IO/render shell.

### 10.1 The unidirectional cycle

```
crossterm event
   │
   ▼
[input]     KeyEvent → resolve against mode-aware keymap (KeyTrie)
   │
   ▼
[command]   Command fn: (&Context) → Transaction | Effect   (describes, never mutates)
   │
   ▼
[apply]     ONE function: editor.apply(Transaction)
   │          • Transaction = ChangeSet (+ optional new Selection)
   │          • mutate rope · map selection THROUGH the ChangeSet · push undo
   │          • version += 1 · mark derived caches dirty
   ▼
[derive]    recompute O(visible-screen) derived state synchronously
   │         (block reparse, soft-wrap for viewport); kick slow work to a
   │         worker with (snapshot, version)
   ▼
[render]    terminal.draw(): PURE read of state → ratatui frame (cell-diff minimizes writes)
```

This keeps MVU's **discipline** (one state location; all mutations named and
funneled through `apply`; render mutates nothing) without an MVU framework. Because
ratatui diffs cells internally, we rebuild the frame unconditionally each draw — no
dirty-flag render plumbing needed.

### 10.2 State: source of truth vs derived

A flat, owned `Editor` struct (no ECS, no entity/handle framework — confirmed
overkill for single-window). `Document` = logical/persistent state; `View` =
display state. We keep the split **even with one window** because soft-wrap is
view-derived and conflating it with logical lines is what broke xi.

```
Editor { document, view, mode, pending_keys, count, register, status, keymap, jobs, quit }
Document { text: Rope, selection, history, version: u64, path, dirty,
           blocks (cache), syntax/styling (cache) }
View     { scroll, area, wrap (cache) }
```

| State | Role | Lives in | Updates on edit via |
|---|---|---|---|
| Rope text | **truth** (content) | Document | `apply` mutates in place (sole writer) |
| Selection (anchor/head) | **truth** (cursor) | Document | `selection.map(&changeset)` — same atomic step as the text edit |
| Undo history | **truth** (time) | Document | `apply` pushes inverted Transaction (text+selection) |
| `version: u64` | **truth** (revision) | Document | `+= 1` per apply; staleness token for async |
| Block map | derived | Document | dirty → reparse edited block only (pulldown-cmark) |
| Concealed styling | derived | Document | **viewport: synchronous** (block reparse in `derive`, §10.3/§10.5) so the immediate frame is correct; only off-screen / heavy whole-doc passes are async-filled from (snapshot, version), merged if current |
| Soft-wrap map | derived | View | rebuilt for visible range on edit/resize (width-dependent) |
| Scroll / area | display | View | clamped to keep cursor visible; reset on resize |
| mode / pending keys / count / register | ephemeral | Editor | set by input; cleared when a command resolves |
| status / feedback | ephemeral | Editor | set the instant an action starts; cleared on done |

**Load-bearing rule:** derived rows are reconstructible from truth rows and are
never authoritative. The #1 editor bug — "cursor jumps after an edit earlier in the
buffer" — comes from treating a bare `usize` offset as durable; every surviving
position must be mapped through the ChangeSet on the same step as the mutation.

### 10.3 Concurrency: sync core, async edges, no tokio

Core (rope/cursor/edit/undo/render) is synchronous on the foreground thread. Slow
work runs on `std::thread` workers over `mpsc`, on **immutable rope snapshots**
(ropey clone is O(1), `Send + Sync`):

```
apply(): … ; version += 1 ; snapshot = rope.clone() ; job_tx.send(Job{snapshot, version, kind})
          … keep handling input, never block …
before each draw: while let Ok(r) = result_rx.try_recv() {        // non-blocking drain
                      if r.version == document.version { merge r.payload }   // else discard (stale)
                  }
                  terminal.draw(|f| render(f, &editor))
```

- **Reconcile = discard, not rebase.** Drop stale results (version moved). No OT/CRDT
  rebasing — xi's documented over-engineering for a single-user editor.
- **Block reparse stays synchronous** (O(edited block), fits the keystroke budget).
  Reserve workers for spellcheck, full-document search, file load/**save** (set
  `status="Saving…"` immediately), and subprocess **filters**.
- **Debounce typing bursts**: latest-wins single-slot handoff so only the most
  recent snapshot is worked.
- Render loop never blocks: `try_recv()` only; write `status` *before* dispatching a
  job (instant feedback, per §3.9).

### 10.4 Command dispatch

Three decoupled layers: **bindings** (data: key→action, mode→`KeyTrie`, user-config)
→ **commands** (code: `fn(&mut Context) -> CommandResult`, produce a Transaction/
Effect, never mutate directly) → **apply** (sole writer). Keymap lookup returns
`Matched(cmd)` / `Pending` (partial sequence — stash, redraw, show hint) /
`NotFound` (in insert mode, unmapped printable → literal text-insert Transaction).
The boolean/NoOp result allows ordered fallthrough.

### 10.5 Worked keystroke trace (typing a character mid-paragraph)

1. crossterm `KeyEvent('x')` → keymap (insert mode) → `NotFound` → literal insert.
2. Build `Transaction` = insert "x" at cursor; `editor.apply(tr)`:
   `rope.insert` (O(log n)); `selection.map(&changeset)` advances the cursor;
   push undo; `version += 1`; mark the cursor's **block** dirty.
3. Derive (sync, O(visible screen)): reparse only the dirty block with
   pulldown-cmark `into_offset_iter()`; rebuild the affected visual rows in `View.wrap`
   via `layout` (conceal + soft-wrap + `col_map`). The **active line shows raw
   markdown so it isn't concealed**, and inactive lines didn't change — so the work
   is tiny.
4. `terminal.draw()` reads state → ratatui frame; cell-diff emits only changed cells.

Per-keystroke cost = O(visible screen) + O(edited block). No document-sized work on
the hot path — §3.9 satisfied by construction.

### 10.6 Anti-patterns to avoid (from xi & others)

- Async as the core glue (xi's headline mistake) — async only at IO/CPU edges.
- Bare offsets as durable state — always map through the ChangeSet.
- Conflating logical and visual (wrapped) lines — logical in Document, visual in View.
- Eager full re-derivation per keystroke — mark dirty, recompute incrementally.
- Frontend/backend process split, JSON-RPC plugins, CRDT, full ECS — all confirmed
  unjustified for single-user/single-window/offline.
- Scattered mutations / mutating inside `draw()` — one `apply` channel; render is pure.

### 10.7 License note
All adopted patterns are clean: CodeMirror 6 (MIT), ropey/ratatui/crossterm
(MIT/Apache), std threads. Helix (MPL) and Zed/GPUI + xi-rope (GPL) are **pattern
references only**, not copied code.

---

## 11. Testing Strategy

The §10 "functional core, imperative shell" design is what makes this tractable:
the core (buffer ops, `apply`/ChangeSet, `selection.map`, `md_parse`, `layout`/
`col_map`, undo) is **pure and synchronous**, so it is unit- and property-testable
with no terminal, no threads, and no real clock. The thin shell (crossterm input,
ratatui render, clipboard, subprocess) is tested with fakes and snapshots. We drive
core modules **test-first** (the project's TDD discipline) — the purity makes that
natural rather than aspirational.

### 11.1 Test layers

| Layer | Scope | Tooling |
|---|---|---|
| **Unit** (the bulk) | One pure core function: buffer edit, changeset apply/invert, selection map, single-line `layout`, block parse | std `#[test]` |
| **Property** | Algebraic laws & invariants over *generated* inputs (see §11.2) | `proptest` |
| **Golden / snapshot** | Rendered frame for a given (text, cursor, width): the concealed, soft-wrapped cell grid | `insta` + ratatui `TestBackend` |
| **Integration** | Scripted `KeyEvent` sequence → assert resulting document text **and** rendered frame | fake input iterator + `TestBackend` |
| **Subprocess** | The `filter` primitive against real small tools (`tr`, `sort`, `cat`) and a fake that errors / writes stderr / hangs | std `#[test]`, gated on binary presence |
| **Fuzz** | Parser, `layout`, `col_map` fed arbitrary UTF-8 / markdown — assert no panic, invariants hold | `cargo-fuzz` (nightly CI) |
| **Bench / perf guard** | Per-keystroke cost stays ~flat as document grows (operationalizes §3.9) | `criterion`, tracked in CI |

### 11.2 Invariants to encode (each targets a §10.6 bug class)

These properties are the heart of the strategy — they pin the subtle, high-risk
behavior:

- **Undo round-trip:** for any edit `e`, `apply(invert(e)) ∘ apply(e)` restores the
  **exact** original rope *and* selection. `redo` restores the post-edit state.
- **ChangeSet algebra:** `compose` is associative; `invert(invert(e)) == e`;
  `apply(compose(a,b)) == apply(b) ∘ apply(a)`.
- **Position mapping (the "cursor jumped" class):** after any ChangeSet, every
  mapped selection offset is a valid char boundary and matches the hand-computed
  expected position — including edits *before* the cursor. Insertion-bias (`Assoc`)
  is respected at edges.
- **`col_map` bijection (the atomicRanges guarantee):** moving the cursor one
  visual cell never lands inside a concealed marker; `col_map[c]` is always a valid
  source char boundary; visual→byte→visual round-trips are stable. On the **active
  (raw) line**, `col_map` is the identity.
- **Soft-wrap fidelity:** concatenating a logical line's visual rows reconstructs
  the source; wrapping never splits a grapheme; visual widths obey `unicode-width`.
- **Incremental == full reparse (oracle test):** reparsing only the dirty block
  yields the **same** styled spans as parsing the whole document. This is the key
  guard for markdown's context-sensitivity — it must catch the setext-heading
  (`Foo` / `===`) and list-continuation edge cases (§9.2).
- **Undo coalescing:** a burst of typed chars collapses to one undo unit; a paste,
  a programmatic edit, or a deliberate cursor move starts a new unit (§9.2).
- **Selection/clipboard:** copy→paste round-trips text exactly; cut then undo
  restores; bracketed-paste arrives as one atomic edit (one undo unit), not N.

### 11.3 Determinism (no flaky tests)

The shell's non-determinism is injected so tests stay reproducible — adopting
kiro's trait-abstracted I/O:

- **Input** is an `Iterator<Item = KeyEvent>`; tests feed a scripted vector.
- **Output** is ratatui's `TestBackend` (renders to an in-memory cell buffer we can
  assert/snapshot).
- **Clock** is a `TimeSource` trait, not `Instant::now()` — so undo-coalescing
  time thresholds (§9.2) are tested with a fake clock, not wall-clock.
- **Background jobs** run through an injectable runner: tests use a **synchronous**
  runner (work runs inline) or assert on the job/result *messages*, so the
  sync-core/async-edges concurrency (§10.3) is verified without real thread races.
  The version-stamp **stale-discard** path gets an explicit test (submit job →
  advance version → assert result dropped).

### 11.4 Module → test focus

| Module | Primary technique | Must-cover |
|---|---|---|
| `text_buffer` (ropey wrapper) | unit + property | edit ops, byte↔char↔line conversions on multibyte/emoji |
| `edit_diff` / `history` | property | the changeset algebra + undo round-trips above |
| `selection` | property | `.map(changeset)` correctness; clipboard round-trip |
| `md_parse` | unit + fuzz | byte ranges for markers vs content; GFM constructs |
| `layout` / `col_map` | property + golden + fuzz | bijection, soft-wrap fidelity, active-line identity |
| `filter` | subprocess + unit | stdin/stdout/stderr, non-zero exit, cancellation, no deadlock |
| `editor` (loop/dispatch) | integration | keymap resolution, pending-sequence, mode behavior |
| `render` | golden | concealed + wrapped frames; status-line feedback |

### 11.5 Tooling & CI
`proptest`, `insta`, `criterion`, `cargo-fuzz`, ratatui `TestBackend`. CI runs
unit + property + golden + integration on every push; fuzz targets nightly; the
per-keystroke bench is tracked for regressions (a hard threshold would be flaky, so
trend it and alert on step-changes). Mirror kiro/kibi precedent (both ship fuzzing
+ CI), and adopt **repar's discipline** (§14.4): a committed golden corpus, checked-in
`proptest-regressions` seeds, and round-trip-law invariants.

### 11.6 Non-goals
Do **not** test dependency internals (ropey, ratatui, crossterm, pulldown-cmark —
trust them) or pandoc itself (only our *invocation* and error handling, via a
fake/gated real binary). Cross-terminal clipboard/OSC-52 behavior and true
terminal-emulator rendering are **manual smoke checks**, not automated — the
matrix is too large and environment-bound to pin in CI.

---

## 12. Configuration & Keybindings

### 12.1 Interaction model: modeless/CUA in v1, mode-capable layer underneath
- **Decision:** v1 is **modeless / CUA** — you are always typing; commands are
  Ctrl/Alt chords (Ctrl+S save, Ctrl+F find, Ctrl+B bold). This is the WordGrinder
  / nano / Apple Writer lineage and the lowest-friction model for prose, where you
  are writing ~99% of the time.
- **Reconciles §10.4:** v1 has effectively a single mode, so keymap resolution
  simplifies — there is no normal/insert dance. But the dispatch layer stays
  **mode-capable internally** (`mode` is a field that is constant `Insert` in v1),
  so a future **vim/modal mode** can be added post-v1 as a pure config toggle, not
  a rewrite. §10.4's "unmapped printable key → literal text insert" is the v1 default
  path for every printable key.

### 12.2 Command discovery: command palette + hideable menu (cross-linked)
Distraction-free means minimal persistent chrome, but commands must be
discoverable. Two complementary surfaces, each reachable from the other (the
VS Code model):
- **Command palette** (primary power path): invoked by a chord (default
  `Ctrl+P`); fuzzy-searches **every** command + filter presets + export targets,
  including obscure ones. Hidden until invoked; reuses our fuzzy-search stack
  (`nucleo`). This is how power users reach everything without memorizing chords.
- **Menu** (browsable common actions): a hideable bar (File / Edit / Format /
  Insert / View / Export) surfacing *typical* needs. **Toggleable; hidden by
  default** to stay distraction-free. The menu contains an entry that opens the
  command palette (so new users discover the palette through the menu), and any
  command shown in the menu displays its chord (so users learn shortcuts in place).
- Both render **instantly with feedback** (§3.9): the surface opens immediately;
  any slow action it triggers shows a spinner/status rather than freezing — the
  direct fix for the WordGrinder menu-lag frustration.

### 12.3 Default keybindings (CUA)
Representative defaults (fully overridable via config, §12.4). Grouped by area:

| Area | Binding | Command |
|---|---|---|
| File | Ctrl+S / Ctrl+O / Ctrl+N / Ctrl+Q | save / open / new / quit (prompt if dirty) |
| Edit | Ctrl+Z / Ctrl+Y / Ctrl+C / Ctrl+X / Ctrl+V / Ctrl+A | undo / redo / copy / cut / paste / select-all |
| Format | Ctrl+B / Alt+I / Ctrl+K | bold / italic / insert-link |
| Headings/lists | Alt+1..6 / Alt+L / Alt+Q | heading level / toggle bullet list / blockquote |
| Navigate | arrows, Ctrl+←/→, Home/End, Ctrl+Home/End, PgUp/Dn | char/word/line/document movement |
| Search | Ctrl+F / Ctrl+R / F3 | find / replace / find-next |
| View | Ctrl+P / F10 / Ctrl+G / F2 / (toggle) | command palette / menu / word-count / cycle render mode (§3.11) / focus mode |
| Pipe/Export | Ctrl+\| / palette | run filter on selection / pandoc export presets |

### 12.4 Terminal key constraints (important, shapes the defaults)
Legacy terminals collapse some chords onto control codes and **cannot distinguish
them**: `Ctrl+I`≡Tab, `Ctrl+M`≡Enter, `Ctrl+H`≡Backspace, `Ctrl+[`≡Esc; many
`Ctrl+Shift+*` combos are also indistinguishable. Consequences for our defaults:
- **Never bind italic to `Ctrl+I`** (it is Tab) — hence `Alt+I` above. Likewise
  avoid `Ctrl+M/H/[`.
- **Enhance when available:** crossterm can enable the **Kitty keyboard protocol**
  (`PushKeyboardEnhancementFlags`) on supporting terminals (kitty, foot, WezTerm,
  Ghostty…) to disambiguate richer chords; **degrade gracefully** elsewhere.
- **Safety net:** because the **command palette reaches every command**, no command
  is ever locked behind an unavailable chord — discoverability never depends on a
  terminal supporting a particular key combo.

### 12.5 Config file
- **Format:** TOML (Rust-idiomatic; matches Helix). We keep the `toml` crate
  (Wordcartel is not binary-size-constrained the way repar deliberately is).
- **Location & precedence** (adopting repar's chain + project-local discovery):
  built-in defaults < global XDG `~/.config/wordcartel/config.toml` (via
  `etcetera`/`dirs`) < **project-local `.wordcartel.toml`** (found by walking up from
  the file's directory) < env (`$WORDCARTEL_*`) < command-line args. `$WORDCARTEL_CONFIG`
  overrides the global path; `--no-config` disables file lookup. Missing config →
  built-in defaults.
- **Keymap is data; commands are code** (§10.4). The `[keys]` table maps key
  strings → command names, *overriding/extending* defaults. An unknown command
  name is a **surfaced error**, never a silent no-op.
- Sketch:

```toml
[editor]
focus_mode = false         # distraction-free toggle
autosave   = false
wrap_width = 0             # 0 = wrap to viewport; >0 = fixed prose column

[ui]
menu_visible = false       # menu hidden by default (§12.2)
theme        = "default"

[keys]                     # overrides/additions to the CUA defaults
"Ctrl-e" = "export_menu"
"Alt-z"  = "toggle_focus_mode"

[pandoc]
path = "pandoc"            # autodetected; degrade gracefully if absent (§3.1)

# Filters run argv ARRAYS by default (no implicit shell); set shell = true for pipelines.
# (reflow / unwrap / ventilate are IN-PROCESS repar commands, §14 — not subprocess presets.)
[filters.table-align]      # tidy GFM table source so styled-raw reads cleanly (§13.6)
argv        = ["prettier", "--parser", "markdown"]
disposition = "filter"     # filter | insert | export

[filters.normalize-headings]  # setext -> ATX, de-risks the reparse hazard (§13.6)
argv        = ["pandoc", "-f", "gfm", "-t", "gfm"]
disposition = "filter"

[export.docx]              # pandoc writes its own -o file ({out} = chosen path)
argv   = ["pandoc", "-o", "{out}"]
output = "writes_output"   # vs "capture" = read child stdout → path (§3.5 Export note)

# Spellcheck is a DIAGNOSTIC command (§3.5), NOT a filter — it marks misspellings;
# it never replaces buffer text.
[diagnostics.spellcheck]
argv = ["aspell", "--mode=markdown", "list"]
```

- **Theme** keys color the concealed-markdown styling (heading/emphasis/code/link/
  blockquote) and chrome (status line, palette, menu), via `term_color`/ratatui
  with truecolor→256→16 fallback (§4).
- Config is loaded at startup; **live-reload is backlog** (§5).

---

## 13. Markdown Constructs — v1 Conceal/Style Set

Defines which constructs `md_parse` + `layout` recognize and how each renders on an
**inactive** line (the active line always shows raw markdown, §3.2). Base = CommonMark
+ a GFM subset. (The earlier debatable scope calls — tables, setext — were resolved
by reframing them as filter operations; see §13.6.)

### 13.1 Parser configuration
`pulldown-cmark` with GFM extensions enabled: **strikethrough, tables, task lists,
autolinks**. Footnotes off in v1 (see backlog). The parser is fed one block at a
time for incremental reparse (§9.2).

### 13.2 Terminal rendering primitives
Terminals have **no font sizes**, so the heading hierarchy is conveyed by **weight +
color**, not size. Available styling: bold (SGR 1), italic (SGR 3), strikethrough
(SGR 9), underline (SGR 4), dim (SGR 2), plus theme colors (truecolor→256→16, §4).
Each has a graceful fallback (e.g., italic→color if a terminal lacks SGR 3).

### 13.3 Inline constructs (v1)

| Construct | Syntax | Inactive-line rendering | Notes |
|---|---|---|---|
| Emphasis / italic | `*x*` `_x_` | italic; markers concealed | |
| Strong / bold | `**x**` `__x__` | bold | |
| Bold-italic | `***x***` | bold + italic | |
| Inline code | `` `x` `` | code style (distinct color/bg); backticks concealed | |
| Strikethrough (GFM) | `~~x~~` | strikethrough | |
| Link | `[t](url)` | `t` shown underlined/colored; `[](url)` concealed; full URL revealed on active line | handle pulldown-cmark URL-offset quirk (#441) by scanning the link span |
| Autolink / bare URL (GFM) | `<url>`, `http://…` | styled as link | basic in v1 |
| Image | `![alt](url)` | placeholder: styled `alt` + image glyph; syntax concealed | inline image *display* (kitty/iTerm/sixel) = backlog; **not filter-addressable** (§13.6); external "open image" command is the realistic path |
| Escape | `\*` | literal char; backslash concealed | correctness |
| Hard line break | trailing `  ` or `\` | line break in layout | |
| Inline HTML | `<span>` | rendered literally, dimmed | passthrough, no conceal |

### 13.4 Block constructs (v1)

| Construct | Syntax | Inactive-line rendering | Notes |
|---|---|---|---|
| Paragraph | text | base styling | the common case |
| ATX heading | `#`..`######` | hierarchy by weight+color (all 6 levels); `#`s concealed | |
| Setext heading | line over `===`/`---` | styled as heading; underline concealed | parsed correctly; two-line-context hazard (§9.2). De-risked by a **setext→ATX normalize filter** (§13.6) that makes it rare in practice |
| Blockquote | `>` (nestable) | gutter bar + styled text; `>` concealed | |
| Unordered list | `-` `*` `+` | bullet glyph `•`; marker concealed; nested by indent | |
| Ordered list | `1.` | number kept; nested | |
| Task list (GFM) | `- [ ]` / `- [x]` | checkbox glyph `☐` / `☑` | |
| Fenced code block | ```` ``` ```` + info | distinct block style; fences dimmed/concealed | **no in-code syntax highlighting in v1** (backlog) |
| Indented code block | 4-space indent | code style | |
| Thematic break | `---` `***` `___` | horizontal rule line | parser disambiguates from setext |
| Table (GFM) | `\| … \|` | v1: styled raw (pipes dimmed) | a **table-align filter** (§13.6) tidies the source so styled-raw reads cleanly; full live grid render = backlog |
| YAML front matter | `---`…`---` at top | dimmed metadata block; edited raw | relevant to pandoc export (title/author) |
| HTML block | `<div>…` | rendered literally, dimmed | passthrough |

### 13.5 Deferred to backlog (with reason)
- **Footnotes** (def + ref) — extra inline/block complexity; uncommon in drafting.
- **Pretty aligned table rendering** — real layout work (column widths, alignment);
  v1 ships readable raw tables.
- **Inline image display** — terminal image protocols (kitty/iTerm2/sixel) are a
  separate capability-detection feature.
- **In-code-block syntax highlighting** (live, in-editor) — a code-editor feature,
  out of scope for a prose tool. Note: **pandoc already highlights code in exported
  output** (`--highlight-style`), so highlighted code in the produced docx/PDF/HTML
  is free; only the live view lacks it (§13.6).
- **Math** (`$…$`, `$$`), **definition lists** (pandoc extensions) — niche for v1;
  pandoc still handles them on export since the source text is preserved verbatim.
- **Obsidian-isms** (wiki-links `[[…]]`, callouts) — we target CommonMark + GFM +
  pandoc, not a specific app's dialect.

Note: deferring a construct's *rendering* never blocks its *content* — unsupported
syntax stays as plain text in the `.md` and still round-trips through pandoc on
export. "Deferred" means "not specially concealed/styled yet," not "unusable."

### 13.6 What the `filter` layer + pandoc absorb (vs. in-core rendering)
**Principle:** filters transform the markdown *source text* (text→text, into the
buffer or to export); rendering changes the *live view*. A scope item is
filter-addressable only if it can be reframed as a source transformation. Applying
this to the items above keeps in-core rendering thin:

| Item | Filter/pandoc-addressable? | How |
|---|---|---|
| GFM tables | **Mostly** | A **table-align filter** (`prettier --parser markdown` / `pandoc -t gfm`) pads columns so the styled-raw rendering already reads as a clean table; live box-grid render stays backlog but matters less. |
| Setext headings | **Partly (de-risk)** | A **setext→ATX normalize filter** (`pandoc -t gfm`) rewrites `Foo`/`===` to `# Foo`, making the two-line-context reparse case rare. Parsing stays correct when present. |
| Inline image display | **No** | Text→text can't emit pixels (ASCII-art would corrupt source). A separate **"open image externally" command** is the path; inline display stays backlog. |
| In-code highlighting | **No (live) / free (export)** | A filter can't color the live view without corrupting source; but **pandoc highlights code on export**, covering the output-document need. |

These two presets (`table-align`, `normalize-headings`) ship as built-in filter
presets (§3.5/§12.5), discoverable in the palette and menu. This is the §3.1/§3.5
"delegate to the Unix-pipe layer" thesis in action: two of four flagged rendering
gaps become filter presets, a third is covered by pandoc on export.

### 13.7 The two context-sensitive hazards (reminder)
**Setext headings** and **list continuation** are the constructs that violate
block-locality (§9.2): they depend on adjacent lines. The incremental-reparse cache
must use two-line context for setext and re-scan list boundaries on edit — and the
**incremental == full reparse oracle test** (§11.2) exists precisely to catch
regressions here.

---

## 14. repar Integration & Line-Structure Model

[repar](../../par-command/repar) is the author's own MIT prose/markdown reformatter
(I/O-free library core, `#![forbid(unsafe_code)]`, `Options::new()…format(input) ->
Result<String>`). It is both a dependency and a design sibling: **Wordcartel is the
interactive editor to repar's batch reflow engine.** Because it is the author's
code, there is no licensing friction (depend, copy, or relicense freely).

### 14.1 Dependency: in-process transforms
Depend on `repar` as a **library**; Reflow / Unwrap / Ventilate are **in-process**
`repar::Options` calls — no subprocess, markdown-aware, no "is the binary
installed?" concern. Exposed as first-class commands **and** as bundled presets of
the §3.5 `filter` mechanism (which still covers arbitrary external CLIs). One
transform per invocation:
- **Reflow** → hard-wrap to a width (the *publish* format).
- **Unwrap** → one logical line per paragraph (the *soft-wrap-ready* form).
- **Ventilate** → one sentence per line (semantic line breaks; git-diff-friendly).

### 14.2 Line-structure model: soft-wrap editing, hard-wrap as I/O
**Decision (confirms §3.4): one editing model — soft-wrap.** Hard-wrap is never a
second editing mode (a true dual mode was considered and rejected: it would need
two cursor models and undo across a representation switch, straining the
"source = text as-is" invariant). Hard-wrap is handled at the edges:
- **Wrap-guide ruler:** a dim vertical guide at the configured target column
  (default 72/80) gives hard-wrap *awareness* while editing soft. Pure display.
- **Import:** the file's existing on-disk wrapping is **respected by default** — we
  do not silently rewrap. "Unwrap on import" is an **explicit action** (or opt-in
  per file); auto-unwrap+reflow-on-save would rewrap a whole hard-wrapped file and
  bury the user's real change in diff noise.
- **Export / save-as:** optional **reflow** (hard-wrap at width) or **ventilate**
  (semantic breaks) output filters; the default save preserves the buffer text as-is.
- **Ventilate as a storage option:** a config/export option to keep the on-disk
  `.md` ventilated (one sentence per line) for clean VCS diffs; the editor still
  soft-wraps it at view time. repar's round-trip law `reflow(unwrap(p)) ==
  reflow(p)` makes this lossless.

### 14.3 Direct code borrows (repar is MIT, authored by the user)
- **Atomic save (`atomic.rs`)** → Wordcartel's save path (§10.3): same-dir O_EXCL
  temp (owner-only `0o600`) → write → `fsync` → `rename` → dir-`fsync`;
  skip-unchanged; refuse symlink / binary / non-UTF-8; `TempGuard` cleanup on
  unwind; preserves mode. Its documented caveats (symlink refusal, sudo re-own,
  last-writer-wins, mode-only preservation, trailing-newline-once, Unix-only) are
  adopted as Wordcartel's save-error spec (feeds §15). The background-thread save
  (§10.3) wraps this over a rope snapshot.
- **Display width (`width.rs`)** → the `layout`/soft-wrap width math: a tested
  `unicode-width` wrapper with tab-stop handling. We use its **real zero-width**
  behavior (combining marks = 0), not par's `wcwidth<=0 → 1` fidelity punt.

### 14.4 Patterns adopted from repar
- **Testing methodology** (extends §11): golden corpus + property invariants as
  **round-trip laws** + **differential fuzzing against an oracle** + committed
  `proptest-regressions` seeds. repar's `reflow(unwrap(p)) == reflow(p)` is the same
  shape as our incremental==full reparse oracle (§11.2).
- **Markdown structural passthrough:** reflow only prose; pass code/tables/headings
  verbatim; strip-and-reapply list/quote prefixes with a hanging indent. Informs a
  future "rewrap this paragraph/list item" command and our block handling.
- **I/O-free library core + `#![forbid(unsafe_code)]`** independently mirrors our
  functional-core/imperative-shell split (§10) — hold the same line.
- **Not adopted:** repar's `panic="abort"` size profile — Wordcartel is interactive
  and wants unwinding + panic recovery.

---

## 15. Error Handling & Recovery

Consolidates the error behavior referenced throughout the spec into one contract.

### 15.1 Principles
1. **Never lose the user's work.** The on-disk file is never left half-written
   (atomic save, §14.3); an internal panic attempts an emergency buffer dump
   (§15.7) before exit.
2. **Never crash silently.** Every failure is surfaced with immediate feedback
   (§3.9) — the same "no silent waits" rule applied to errors: an error must be
   *visible*, never a frozen or unexplained UI.
3. **Errors are values at the edges; panics are bugs.** All IO/subprocess/parse
   boundaries return `Result` (a `thiserror` enum). The pure core (§10) does not
   fail — its invariants are property-tested (§11). A reached panic means a bug,
   handled by §15.7, not by normal control flow.
4. **Degrade, don't abort.** A missing optional dependency (pandoc, a filter
   binary, a system clipboard) disables *that* feature with a message; core
   editing always continues.

### 15.2 Presentation model
- **Transient status-line message** for the overwhelming majority — info /
  warning / error severity, color-coded, non-blocking, auto-dismissed. This is the
  default; it never interrupts typing.
- **Modal confirmation** only for genuinely destructive decisions: quit with
  unsaved changes, overwrite a file changed on disk (§15.3), overwrite on a failed
  save retry. Reserved and rare — distraction-free means almost nothing is modal.

### 15.3 File I/O
- **Open failures** (not found, permission denied, is-a-directory) → status-line
  error; the editor stays usable (prior/empty buffer). 
- **Binary / non-UTF-8 file** → **refused**, not opened (we are a UTF-8 markdown
  editor; silent lossy replacement would corrupt). Uses repar's `is_binary` test
  (NUL byte or invalid UTF-8). Clear message.
- **Save failures** (dir not writable, disk full, temp create/rename error) → the
  atomic strategy (§14.3) guarantees the **original file is untouched**; the buffer
  **stays dirty**; status-line error invites retry or save-as. Save runs on a
  worker (§10.3): `status="Saving…"` → `"Saved"` or the error.
- **Skip-unchanged:** if formatted bytes equal the on-disk bytes, no write occurs
  (no inode churn) — a no-op, reported quietly.
- **Symlinks:** refused by default (write the realpath) — adopted from `atomic.rs`.
- **External modification:** if the file's mtime/size changed on disk since load,
  a save prompts (reload / overwrite / save-as) rather than silently clobbering
  (mitigates `atomic.rs`'s last-writer-wins). *(Detection v1; richer 3-way merge is
  backlog.)*

### 15.4 Subprocess: filters, pandoc, export
- **Missing binary** (`pandoc`/filter command not found — spawn `ENOENT`) →
  message ("`pandoc` not found — install it to export"); core editing unaffected
  (§3.1). Pandoc presence is probed at startup and the export commands disable
  gracefully when absent.
- **Non-zero exit** → the buffer is **not modified** (the filter aborts, selection
  preserved); the child's **stderr is shown** in the status line with the exit
  code. stderr is never inserted into the buffer.
- **Long-running / hung filter** → runs off the hot path (§10.3); shows
  `running <cmd> ⏳`; **cancellable via Esc**, which `kill`s the child. An optional
  per-filter timeout is configurable.
- **Deadlock-safe I/O:** concurrent stdin-write / stdout-read (two threads or the
  `subprocess` crate), per §9.2/§10.3 — never a single-pipe stall.
- **Export-target write failure** (e.g. read-only output path) → message; the
  buffer and source file are untouched.

### 15.5 Configuration
- **Parse error** (malformed TOML) → fall back to **built-in defaults** and surface
  a message with the file + line; the editor always starts.
- **Unknown command name** in a `[keys]` binding → that binding is ignored and the
  error surfaced (§12.5); startup continues.
- **Unknown/invalid filter preset** (bad disposition, empty command) → that preset
  is skipped with a message; others load.
- `--no-config` bypasses all config-file lookup for a clean-room session.

### 15.6 Clipboard & terminal capabilities
- **Clipboard** is optional (§9.5): the *system* clipboard syncs via
  `arboard → OSC 52`, and if unavailable that *sync* is a no-op (one-time notice) —
  but copy/cut/paste **always** work via the in-process register, so editing is
  never affected. Never `unwrap`.
- **Terminal capabilities** (truecolor, Kitty keyboard protocol, OSC 52) are
  detected and **degrade gracefully** (§4, §12.4) — never an error.
- **Terminal too small** (width below a usable minimum) → render a clamped
  "window too small" notice rather than panicking or mis-wrapping.

### 15.7 Panic safety & crash recovery
- Wordcartel **does not** use `panic="abort"` (§14.4) precisely so a panic can
  **unwind cleanly**: a guard around the main loop restores the terminal (leave raw
  mode, show cursor, disable enhancements), then attempts an **emergency dump** of
  the buffer to a recovery path (e.g. `~/.local/state/wordcartel/recovered-<name>-
  <pid>.md`) before exiting with the panic report for a bug filing. This is the
  last line of the "never lose work" guarantee.
- **Hard kill / power loss** (no unwind possible) is mitigated by (a) atomic save —
  the on-disk file is never half-written — and (b) an **optional autosave / swap
  file** safety net (vim-`.swp`-style), offered on next open. *(Autosave/swap is
  v1-light or backlog; the atomic guarantee + emergency dump cover the common cases.)*

---

## 16. Spec Status — Design-Complete; Hardening In Progress

The **design** (decisions, rationale, architecture) is complete and was
**red-teamed by an independent reviewer (Codex)**. §§1–15 cover: vision & non-goals
(1–2), key decisions & rationale (3), architecture (4), scope (5), licensing (6),
kiro reuse map (7), paths not taken (8), prior art & component stack (9), data-flow
& control-flow (10), testing (11), config & keybindings (12), the v1 markdown
construct set (13), repar integration & line-structure model (14), and error
handling & recovery (15).

**Post-red-team status.** The load-bearing choices were validated; this pass fixed
the contradictions and one bug the review found (buffer-reuse tables, undo wording,
sync/async styling, clipboard semantics, export disposition, the `aspell`-as-filter
bug → spellcheck is now a diagnostic). Two items are not yet fully specified and are
being addressed before/at the start of implementation:

1. **Design risk — incremental markdown reparse** (§9.2 ⚠): blank-line scoping is
   unsafe for full CommonMark; resolution (block-tree invalidation vs narrowed v1
   scope) pending.
2. **Design risk — `layout`/`col_map` × conceal × soft-wrap × cursor** (§3.2/§4/
   §10.5): the formal `ColMap` + navigation contract is being validated by a
   throwaway **layout spike** before the hardening pass.

**Open items deferred to the implementation spec** (tracked, not done): formal
`ColMap` type & navigation-semantics table; conceal/span model; soft-wrap algorithm
details; canonical position type; undo-granularity specifics; IME/paste/grapheme
handling; performance budgets (numeric); large-document limits; full config schema;
filter security model (argv/shell, caps, timeouts); pandoc export contract;
version-control/encoding/newline policy; accessibility baseline; autosave/swap
decision. (See the Codex red-team for the full enumeration.)

**Next steps:** (1) build the **layout spike** to validate the riskiest model
empirically; (2) a **hardening pass** that resolves the two design risks, pulls the
cheap specifics forward, records the product decisions, and turns the open-items
list into concrete contracts; (3) the implementation plan (`writing-plans`). The
natural build order is the **pure core first**: `ropey`-backed buffer →
`edit_diff`/`history` (undo) → `selection` → `md_parse` → `layout`/`col_map`, then
io/shell (crossterm input, ratatui render, `repar` transforms, `filter`, clipboard,
atomic save), then app wiring (editor loop, commands, config, palette/menu).
