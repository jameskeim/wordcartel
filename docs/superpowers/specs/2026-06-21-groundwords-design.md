# groundwords — Design Document

**Status:** Living document — design in progress
**Last updated:** 2026-06-21

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
- **Rationale:** De-risks the build. ~7 of 12 modules port with light edits; the
  genuinely new code concentrates in the 4 areas already identified as novel.

---

## 4. Architecture

Three layers: a pure, terminal-free **core** (unit-testable in isolation); a thin
side-effecting **io/shell** layer; and the **app** wiring. Provenance is marked:
✅ ported from kiro · 🟡 adapted from kiro · 🔴 net-new.

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
├── io / shell (side-effecting, thin)
│   ├── input         raw mode, key/escape parsing             ✅ kiro
│   ├── screen        cursor-aware dirty-row rendering         🟡 kiro
│   ├── clipboard     arboard + OSC 52 fallback                🔴 new
│   └── term_color    truecolor → 256 → 16 detection           ✅ kiro
│
└── app
    ├── editor        lifecycle + input loop, owns state       🟡 kiro
    ├── commands      keybinds → actions (incl. filter presets) 🔴 new
    └── config        keybinds, theme, pandoc path, presets    🔴 new
```

### The hard module: `layout`
`layout` is where conceal, soft-wrap, and the **screen↔source column map**
converge. It is a *pure function* — `(logical line text, is-active-line, viewport
width) → (visual rows, column map)` — with zero terminal involvement, so it is
heavily unit-testable. The column map exists because, when markers are concealed,
*visual column ≠ source byte offset*: pressing → past visible `bold` must move the
cursor over the hidden `**`. This module is expected to host most of the editor's
subtle bugs and therefore gets the most test attention.

### Rendering depends on the cursor
Unlike kiro, a row's rendering is **not** a pure function of the buffer alone — it
depends on cursor position (the active line reveals raw markup). Moving the cursor
marks **two** logical lines dirty: the one left (re-conceal) and the one entered
(reveal). kiro's dirty-row tracking is the foundation; we extend its invalidation
to be cursor-aware.

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
| `input.rs` | ✅ Port, extend | Raw mode + key parsing; add keybinds. |
| `prompt.rs` | ✅ Port, reuse | Bottom-line dialogs: search, save-as, filter prompt. |
| `term_color.rs` | ✅ Port wholesale | Truecolor→256→16 detection. |
| `signal.rs` | ✅ Port wholesale | SIGWINCH resize handling. |
| `editor.rs` | 🟡 Port skeleton | Loop survives; state struct grows. |
| `row.rs` | 🟡 Salvage UTF-8 logic | Keep width/byte-index caching; rendering moves to `layout`. |
| `screen.rs` | 🟡 Keep dirty skeleton | Made cursor-aware + soft-wrap-aware. |
| `highlight.rs` | 🔴 Replace → `md_parse` | Different job; reuse its dirty-invalidation pattern. |
| `language.rs` | ⚪ Drop | Markdown-only; no filetype detection. |

---

## 8. Path Not Taken: why not kibi?

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
