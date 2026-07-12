# Prose text objects — structural selection + operator layer: design space (pre-spec exploration)

**Status:** DESIGN-SPACE / pre-spec (2026-07-12). NOT law, NOT an approved spec, NOT a plan. This
grounds an eventual brainstorm for backlog item **S4** (Theme S — manuscript structure); every design
choice below is an open decision for that brainstorm, not a commitment. Despite the original title
("Implementation Specification"), in this project's taxonomy an approved spec is post-brainstorm,
code-grounded, and Codex-gated under `docs/superpowers/specs/` — this is the pre-spec layer that
precedes it, the sibling of `docs/design/effort-p-plugin-system-design-space.md`.

**Provenance:** drafted collaboratively with an external LLM that did **not** have access to this
codebase. It was told about `wordcartel` — `repar`, the Markdown parse tree, the non-modal
mark/selection model, the command registry/palette — and reasons from that description, but it could
not read the real source. So its architecture is *plausible and well-aligned but unverified*, and
§8 ("Integrating `repar`") is explicitly a list of **open questions for the implementer** precisely
because it could not inspect `repar`'s internal factoring. Treat every concrete type, signature, and
seam here as a proposal to be checked against — and largely re-derived from — the real code during a
grounding pass, exactly as we did for Effort P. It is idea material and a strong starting map, not
ground truth.

**Magnitude / theme note:** this is an **XL** effort — an editing-model layer spanning a text-object
trait, a heuristic sentence-detection module, a clause object with no code analogue, a parameterized
paired-delimiter primitive, Markdown-tree-backed structural objects, and the `repar` seam. Filed
under Theme S for now (it sits beside S1's heading-subtree structure work and reuses the shipped
`repar` transforms of C2/C2b and the transpose/case operators of A14), but it may be **promoted to
its own theme** if it grows its own cluster of items — its scope is broader than any single existing
S item and closer to Effort-P scale. Relationships to record for the eventual brainstorm: S1
(rearrangeable outline — the `Section` object here is the primitive that move needs), C2/C2b
(`repar` reflow/unwrap/ventilate — already shipped; §8 is the seam), A14 (Emacs-parity
transpose/word-case — already shipped; overlaps the operator layer).

---

## 1. Purpose and Rationale

Code editors have spent forty years teaching people to think in **structured selections**: not "select from column 4 to column 18," but "select this argument," "delete this block," "change what's inside these quotes." The Vim ecosystem crystallized this into *text objects* — named regions the editor understands semantically, which compose with *operators* (delete, change, yank, select, move). You never think about coordinates; you think about *things*.

Prose has structure at least as rich as code, but almost no editor exposes it. A writer's document is a nested hierarchy — document, section, paragraph, sentence, clause, phrase, word — layered with typographic and inline structure — quotations, emphasis, links, parentheticals, inline code. Yet the tools we hand writers bottom out at "word, line, paragraph," and even those are usually naive (a "sentence" that breaks on every period, including "Dr." and "3.14").

This spec describes a text-object and operator layer purpose-built for prose. The thesis in one line:

> **A prose editor that genuinely understands sentences, clauses, quotations, and document structure — and lets the writer select, move, and transform them as first-class objects — can offer editing operations no code editor ever would.**

Concretely, that unlocks operations like:

- *Transpose the two clauses of this sentence* (move a subordinate clause to the front).
- *Delete this parenthetical* — the aside and its surrounding em dashes or parens, cleanly.
- *Select this quotation* including its curly quotes, to restyle or re-attribute dialogue.
- *Move this sentence to the previous paragraph.*
- *Reflow this list item and all its children.*
- *Select the current section* (this heading and everything under it) to promote, demote, or relocate.
- *Count words in the current sentence / paragraph / section* on demand.

None of this is exotic once the object model exists. The hard part is defining the objects well for prose, where boundaries are heuristic rather than syntactic. That is the substance of this document.

### 1.1 Lineage and what we are borrowing

This design is informed by a family of Vim plugins built on **`kana/vim-textobj-user`**, a framework that reduces a text object to a single idea: *a function from a cursor position to a span, plus a flag for whether to include surrounding whitespace.* Everything else — the inside/around distinction, integration with operators, motions — is shared machinery.

The specific plugins studied:

| Plugin | Contribution we take |
| --- | --- |
| `vim-textobj-user` | The core abstraction: object = (cursor → span) + inside/around modifier. |
| `vim-textobj-sentence` | Sentence detection as *heuristic period-disambiguation* (abbreviations, decimals, quotes, markup). |
| `vim-textobj-indented-paragraph` | Structure inferred from leading indentation; blank-line inclusion toggle. |
| `vim-textobj-markdown` | Document structure (headings at levels, code fences, prose blocks) as navigable objects; "current vs. search forward/back." |
| `vim-textobj-quote` / `-curly` | Typographic (curly) quotes as objects — prose-native, not ASCII. |
| `vim-textobj-between` | A region between two arbitrary characters — the general primitive many others specialize. |
| `vim-textobj-uri` / `-url` | Link-as-object. |
| `vim-textobj-entire` | Whole-buffer as a composable target. |
| `vim-textobj-lastpat` | The last search match as an object (basis for operate-on-match / multi-cursor). |

### 1.2 Where we deliberately diverge from the Vim lineage

These are code tools wearing prose clothes. Four assumptions do not serve a prose editor and we drop them:

1. **Line-orientation.** Vim objects are heavily line-based. Prose objects are sub-line *and* cross-line: a sentence starts mid-line and runs across a hard wrap. We work in **character/byte offsets over a rope**, not line ranges.
2. **Regex ceiling.** The sentence plugin is regex all the way down, which is why it can never do clauses or grammatical roles. We keep regex/heuristics for the fast path but design the trait so a smarter backend (POS tagging, dependency parse) can be swapped in behind the same interface.
3. **Cramped keyspace.** We are not bound to Vim's `as`/`is`/`g)`/`][` mnemonic crunch. Objects and motions get human-readable names and live in a command palette; keybindings are a presentation choice layered on top.
4. **Regex-scraped Markdown.** `vim-textobj-markdown` recognizes headings and fences with regex. We already parse Markdown for rendering, so our structural objects are driven by a **real parse tree**, not re-scraped from text. (See §7.)

---

## 2. Architectural Overview

The system has four layers, bottom to top:

```
┌─────────────────────────────────────────────────────────┐
│  Command / keybinding / palette layer                    │  (names, mnemonics)
├─────────────────────────────────────────────────────────┤
│  Operator layer   (Select, Delete, Change, Yank,         │  operators apply
│                    Transpose, Move, Reflow, Count …)      │  to spans
├─────────────────────────────────────────────────────────┤
│  TextObject layer (Word, Sentence, Clause, Paragraph,    │  cursor → Span
│                    Quotation, Emphasis, Link, Section …)  │
├─────────────────────────────────────────────────────────┤
│  Buffer          (Rope + Markdown parse tree + cache)     │  the text
└─────────────────────────────────────────────────────────┘
```

The invariant that makes this composable: **every operator works on every object**, because operators consume `Span`s and objects produce them. Adding an object gives it all operators for free; adding an operator gives it every object for free. This is the single most important property of the design and the reason the Vim ecosystem is so productive.

---

## 3. The `TextObject` Trait and the Operator Layer

### 3.1 Core types

```rust
/// A half-open byte range into the buffer: [start, end).
/// Byte offsets index a Rope; callers must keep them on char boundaries.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        debug_assert!(start <= end);
        Span { start, end }
    }
    pub fn is_empty(&self) -> bool { self.start == self.end }
    pub fn len(&self) -> usize { self.end - self.start }
    pub fn contains(&self, offset: usize) -> bool {
        offset >= self.start && offset < self.end
    }
}

/// Inside vs. around — the `i`/`a` distinction, generalized.
///
/// `Inside`  = the content proper.
/// `Around`  = content plus its natural surroundings: trailing whitespace
///             for a sentence, the delimiters for a quotation, the blank
///             line for a paragraph, etc. Each object decides what "around"
///             means for it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Affinity {
    Inside,
    Around,
}

/// Direction for the "search if not currently inside one" behavior and for motions.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Direction { Forward, Backward }
```

### 3.2 The buffer view a text object is allowed to see

Objects should not touch the concrete editor. They receive a read-only view exposing exactly what they need: the rope, and (for structural objects) the Markdown tree. This keeps objects testable in isolation — you can hand them a `&str` in a unit test.

```rust
pub trait BufferView {
    /// The full text as a rope slice abstraction.
    fn rope(&self) -> &Rope;

    /// Convenience: char at a byte offset, if any.
    fn char_at(&self, offset: usize) -> Option<char>;

    /// The Markdown parse tree, if this buffer is parsed as Markdown.
    /// Plain-text buffers return None and structural objects degrade gracefully.
    fn markdown(&self) -> Option<&MdTree>;

    /// Byte length.
    fn len(&self) -> usize;
}
```

### 3.3 The trait itself

```rust
/// A text object: given a cursor, produce the relevant span.
///
/// Design notes:
/// - `find` returns None when there is no such object at/around the cursor
///   (e.g. `Quotation` when the cursor is not near any quotes). Operators
///   treat None as a no-op, matching Vim's "ding and do nothing".
/// - Objects are stateless and cheap to construct; configuration lives in
///   the struct (e.g. which delimiters a PairedDelimiter matches).
pub trait TextObject {
    /// Stable identifier for command/palette wiring and telemetry.
    fn id(&self) -> &'static str;

    /// Human-readable name for the command palette ("Sentence", "Quotation").
    fn label(&self) -> &'static str;

    /// The core operation: cursor position + affinity → span.
    fn find(&self, buf: &dyn BufferView, cursor: usize, aff: Affinity) -> Option<Span>;

    /// Optional: the object *after* the given one, for `count` and for
    /// operators like "move to next sentence". Default: not iterable.
    fn next(&self, _buf: &dyn BufferView, _from: usize, _aff: Affinity) -> Option<Span> {
        None
    }

    /// Optional: the object before. Default: not iterable.
    fn prev(&self, _buf: &dyn BufferView, _from: usize, _aff: Affinity) -> Option<Span> {
        None
    }
}
```

Two optional methods (`next`/`prev`) rather than one, because backward search is often not the mirror of forward search (sentence detection especially — see §5). Objects that are naturally iterable (sentence, paragraph, heading, link) implement them; objects that are not (entire-buffer) do not.

### 3.4 Motions come free from the object

A *motion* is just "jump the cursor to the start (or end) of the next/previous object." Because the object already knows how to enumerate itself, motions need no separate implementation:

```rust
pub enum MotionTarget { Start, End }

pub fn motion(
    obj: &dyn TextObject,
    buf: &dyn BufferView,
    cursor: usize,
    dir: Direction,
    target: MotionTarget,
) -> Option<usize> {
    let span = match dir {
        Direction::Forward  => obj.next(buf, cursor, Affinity::Inside)?,
        Direction::Backward => obj.prev(buf, cursor, Affinity::Inside)?,
    };
    Some(match target {
        MotionTarget::Start => span.start,
        MotionTarget::End   => span.end,
    })
}
```

This directly reproduces the Vim `(` / `)` / `g)` / `g(` sentence motions and the markdown plugin's `]]` / `[[` heading motions, but generically, for any object.

### 3.5 The operator layer

Operators are the verbs. Each takes an object, resolves it to a span at the current cursor, and acts. The critical design point: **operators depend only on `Span` and the buffer, never on which object produced the span.** This is what makes the matrix (objects × operators) fill itself in.

```rust
pub struct Editor {
    buf: Buffer,          // concrete: owns Rope + MdTree + undo history
    cursor: usize,
    registers: Registers, // yank/paste storage
    // …
}

pub enum Operator {
    Select,     // set the visual selection to the span
    Delete,     // remove the span
    Change,     // remove the span and enter insert at start
    Yank,       // copy span text to a register
    Move(Direction, MotionTarget), // reposition cursor via motion()
    Transpose,  // swap this object with the adjacent one of the same kind
    Reflow,     // hard-wrap the body to width (delegates to repar; see §8)
    Unwrap,     // soft-wrap: join the body to one logical line (repar; see §8)
    Ventilate,  // one sentence/clause per line (repar + §5 detector; see §8)
    Count,      // report word/char/sentence stats for the span (prose-specific)
    Uppercase, Lowercase, TitleCase, // case transforms over the span
}

impl Editor {
    pub fn apply(&mut self, op: Operator, obj: &dyn TextObject, aff: Affinity) -> ActionResult {
        let view = self.buf.view();
        match op {
            Operator::Move(dir, target) => {
                if let Some(pos) = motion(obj, &view, self.cursor, dir, target) {
                    self.cursor = pos;
                    ActionResult::Moved
                } else { ActionResult::NoOp }
            }
            Operator::Transpose => self.transpose(obj, aff),
            _ => {
                let Some(span) = obj.find(&view, self.cursor, aff) else {
                    return ActionResult::NoOp; // the "ding"
                };
                match op {
                    Operator::Select    => { self.set_selection(span); ActionResult::Selected }
                    Operator::Delete    => { self.delete(span); ActionResult::Edited }
                    Operator::Change    => { self.delete(span); self.enter_insert(); ActionResult::Edited }
                    Operator::Yank      => { self.yank(span); ActionResult::Yanked }
                    Operator::Reflow    => { self.repar_reflow(span); ActionResult::Edited }
                    Operator::Unwrap    => { self.repar_unwrap(span); ActionResult::Edited }
                    Operator::Ventilate => { self.repar_ventilate(span); ActionResult::Edited }
                    Operator::Count     => { self.report_stats(span); ActionResult::Reported }
                    Operator::Uppercase => { self.map_case(span, Case::Upper); ActionResult::Edited }
                    Operator::Lowercase => { self.map_case(span, Case::Lower); ActionResult::Edited }
                    Operator::TitleCase => { self.map_case(span, Case::Title); ActionResult::Edited }
                    Operator::Move(..) | Operator::Transpose => unreachable!(),
                }
            }
        }
    }

    /// Swap the object under the cursor with the next one of the same kind.
    /// This is the "move the subordinate clause to the front" primitive.
    fn transpose(&mut self, obj: &dyn TextObject, aff: Affinity) -> ActionResult {
        let view = self.buf.view();
        let Some(a) = obj.find(&view, self.cursor, aff) else { return ActionResult::NoOp };
        let Some(b) = obj.next(&view, a.end, aff) else { return ActionResult::NoOp };
        // Splice b before a (order matters to keep offsets valid).
        let a_text = self.buf.slice(a).to_string();
        let b_text = self.buf.slice(b).to_string();
        self.buf.replace(b, &a_text); // replace the later span first
        self.buf.replace(a, &b_text); // then the earlier one
        ActionResult::Edited
    }
}
```

Two operators here have no analogue in code editors and are worth calling out as the payoff of the whole design:

- **`Transpose`** over a *clause* or *sentence* object gives "reorder the parts of this sentence" / "swap these two sentences" as a single keystroke.
- **`Reflow`** and **`Count`** are prose-native operators that make sense on *any* prose object — reflow a paragraph, count words in a section.

### 3.6 The object × operator matrix (illustrative)

Every cell is valid by construction. A few are especially useful for prose (the `Reflow` column stands in for its siblings `Unwrap` and `Ventilate`, omitted here for width; see §8.5):

| | Select | Delete | Change | Yank | Transpose | Reflow | Count |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Word | ✓ | ✓ | ✓ | ✓ | ✓ (swap words) | – | ✓ |
| Sentence | ✓ | ✓ | ✓ | ✓ | ✓ (swap sentences) | ✓ | ✓ |
| Clause | ✓ | ✓ | ✓ | ✓ | ✓ (reorder clauses) | – | ✓ |
| Paragraph | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| Quotation | ✓ | ✓ | ✓ | ✓ | – | – | ✓ |
| Emphasis | ✓ | ✓ | ✓ | ✓ | – | – | ✓ |
| Link | ✓ | ✓ | ✓ | ✓ | – | – | – |
| Section | ✓ | ✓ | ✓ | ✓ | ✓ (reorder sections) | ✓ | ✓ |
| Entire | ✓ | ✓ | ✓ | ✓ | – | ✓ | ✓ |

---

## 4. The Prose Hierarchy

Code nests syntactically: expression in statement in block in function. Prose nests too, but *heuristically and semantically*, and no mainstream editor models the full ladder. Making the whole hierarchy first-class is the conceptual core of this project.

```
Document
  └─ Section          (a heading and everything beneath it, recursively nested by level)
       └─ Block        (paragraph, list, list-item, blockquote, code fence, table)
            └─ Sentence
                 └─ Clause        (independent / subordinate, split on ; : — and conjunctions)
                      └─ Phrase   (noun/verb/prepositional group — optional, needs POS)
                           └─ Word
                                └─ Syllable / morpheme   (optional, rarely needed)
```

### 4.1 Each level, defined

**Document.** The entire buffer. Trivial object (`Entire`); valuable as a composable target for count, reflow, export.

**Section.** *A heading plus all content until the next heading of equal or higher level.* This is inherently recursive: an `##` section contains its `###` subsections. Sections are the unit of large-scale editing — promote/demote a heading, move a whole section, fold one, count its words. Driven entirely by the Markdown tree (§7). This is the level `vim-textobj-markdown` gestured at with per-level heading motions but never modeled as a containment hierarchy.

**Block.** The paragraph and its siblings: list, list-item, blockquote, fenced code, table. These are parse-tree nodes in Markdown. In plain text, a "block" degrades to a blank-line-delimited paragraph. The blank-line-inclusion toggle from `vim-textobj-indented-paragraph` becomes the `Inside`/`Around` affinity here.

**Sentence.** The heuristically-bounded prose sentence. The hardest object; its own module (§5). Crucially *sub-block and cross-line*: one sentence may span several hard-wrapped lines, and one line may hold several sentences.

**Clause.** The distinctive prose object with no code analogue. A sentence splits into clauses at `, ; : —` and at coordinating (`and, but, or, nor, for, so, yet`) and subordinating (`because, although, while, since, if, when, …`) conjunctions. Clause-level `Transpose` — moving a subordinate clause to the front of its sentence, or swapping two coordinated clauses — is a genuinely novel editing operation. Starts rule-based (§4.3); gains precision with a POS/dependency backend later.

**Phrase (optional).** Noun phrase, verb phrase, prepositional phrase. Requires real grammatical parsing, so it is explicitly *phase 2+*. The trait is designed so this slots in without disturbing anything: it is just another `TextObject` whose `find` consults a parser.

**Word.** Unicode-segmentation word (`unicode-segmentation` crate, UAX #29), not "run of non-space," so that contractions, hyphenates, and punctuation behave for prose.

**Syllable / morpheme (optional).** Rarely needed; listed for completeness. Would support poetic operations (meter, hyphenation). Not planned.

### 4.2 Why the hierarchy matters more than any single object

Three payoffs:

1. **Containment-aware selection.** Repeated "expand selection" walks *up* the ladder — word → sentence → clause → … no, wait, word → clause → sentence → block → section → document. Repeated "shrink" walks down. This is the single most-loved feature of structural editors (Vim's `v` growth, VS Code's expand-selection, tree-sitter's incremental selection) and prose has never had a properly prose-shaped version.

2. **Consistent operators at every scale.** The same `Reflow`, `Count`, `Transpose`, `Move` verbs apply whether you are working on a clause or a section. The writer learns one vocabulary of verbs and one of nouns; the cross-product is the whole editing surface.

3. **A natural home for a smarter backend.** The lower three levels (phrase, and better clause/sentence) improve as you add grammatical analysis, *without any change to the operator layer or keybindings*. The hierarchy is the seam along which intelligence gets added.

### 4.3 Expansion / contraction algorithm

```rust
/// Ordered, coarse→fine, for expand/shrink selection.
pub const HIERARCHY: &[&str] = &[
    "document", "section", "block", "sentence", "clause", "word",
];

/// Given the current selection, return the smallest object in the hierarchy
/// that strictly contains it — i.e. "expand selection".
pub fn expand(
    reg: &ObjectRegistry,
    buf: &dyn BufferView,
    cursor: usize,
    current: Option<Span>,
) -> Option<Span> {
    // Walk fine→coarse; return the first object whose span strictly contains
    // the current selection (or contains the cursor, if nothing selected yet).
    let start_idx = match &current {
        None => HIERARCHY.len() - 1, // nothing selected: start at finest (word)
        Some(_) => 0,
    };
    for level in HIERARCHY.iter() {
        let obj = reg.get(level)?;
        if let Some(span) = obj.find(buf, cursor, Affinity::Inside) {
            match &current {
                None => return Some(span),
                Some(cur) if span.start <= cur.start
                          && span.end >= cur.end
                          && span.len() > cur.len() => return Some(span),
                _ => continue,
            }
        }
    }
    let _ = start_idx;
    None
}
```

`shrink` is the mirror: remember the expansion stack and pop it.

---

## 5. The Sentence-Disambiguation Module

Sentence detection is where prose object quality lives or dies. The naive rule — "split on `. ! ?`" — fails constantly: abbreviations (`Dr.`, `Ave.`, `etc.`), initials (`P. I. Magnum`), decimals and versions (`3.14`, `v1.2`), ellipses (`…` and `...`), terminal punctuation *inside* quotes and parens (`She said "Go home." and left.`), and hard-wrapped lines mid-sentence. `vim-textobj-sentence` treats this as heuristic period-disambiguation driven by a configurable abbreviation list; we port that strategy to Rust, make it debuggable, and ship it with a test fixture suite of nasty cases.

We start heuristic (not ML) deliberately: it is fast, transparent, and user-extensible — a writer can add their own abbreviations, exactly as the Vim plugin allows per filetype. A statistical/model backend can replace `SentenceDetector::boundary_after` later without touching callers.

### 5.1 Module design

```rust
/// Configuration for sentence detection. User-extensible; sensible defaults.
#[derive(Clone)]
pub struct SentenceConfig {
    /// Abbreviations that do NOT end a sentence (compared case-insensitively,
    /// without the trailing period). Ships with a starter set; users append.
    pub abbreviations: Vec<String>,
    /// Treat a run of ≥ this many capitalized single letters + periods
    /// (e.g. "P.I.", "U.S.A.") as an initialism, not a boundary.
    pub initialism_max_gap: usize,
    /// If true, a hard line break inside a paragraph does NOT end a sentence
    /// (prose-wrapped source). If false, one-sentence-per-line mode.
    pub join_hard_wraps: bool,
    /// Terminal punctuation glyphs.
    pub terminators: Vec<char>,      // '.', '!', '?', '…'
    /// Closing glyphs allowed to trail a terminator before the boundary:
    /// quotes, brackets, parens, and markup like * _ ` .
    pub trailing_closers: Vec<char>,
}

impl Default for SentenceConfig {
    fn default() -> Self {
        SentenceConfig {
            abbreviations: STARTER_ABBREVIATIONS.iter().map(|s| s.to_string()).collect(),
            initialism_max_gap: 1,
            join_hard_wraps: true,
            terminators: vec!['.', '!', '?', '…'],
            trailing_closers: vec!['"', '\'', '”', '’', ')', ']', '»', '*', '_', '`'],
        }
    }
}

pub struct SentenceDetector {
    cfg: SentenceConfig,
    abbrev_set: HashSet<String>, // lowercased, for O(1) lookup
}

impl SentenceDetector {
    pub fn new(cfg: SentenceConfig) -> Self {
        let abbrev_set = cfg.abbreviations.iter().map(|a| a.to_lowercase()).collect();
        SentenceDetector { cfg, abbrev_set }
    }

    /// Is there a sentence boundary immediately AFTER byte offset `i`
    /// (which must point at a terminator glyph)? This is the heart of it.
    pub fn boundary_after(&self, text: &str, i: usize) -> bool {
        let bytes = text.as_bytes();
        let ch = text[i..].chars().next().unwrap();
        if !self.cfg.terminators.contains(&ch) {
            return false;
        }

        // 1. Decimal / version: digit . digit  → not a boundary.
        if ch == '.' {
            let prev = text[..i].chars().next_back();
            let next = text[i + ch.len_utf8()..].chars().next();
            if matches!(prev, Some(p) if p.is_ascii_digit())
                && matches!(next, Some(n) if n.is_ascii_digit()) {
                return false;
            }
        }

        // 2. Abbreviation: the word ending at this period is in the set.
        if ch == '.' {
            let word = trailing_word(text, i); // "Dr", "Ave", "etc", …
            if self.abbrev_set.contains(&word.to_lowercase()) {
                return false;
            }
            // 3. Single-letter initialism: "P." in "P.I." — capital letter
            //    directly before the period, capital letter shortly after.
            if word.chars().count() == 1
                && word.chars().next().map_or(false, |c| c.is_uppercase()) {
                if self.followed_by_initial(text, i) {
                    return false;
                }
            }
        }

        // 4. Consume trailing closers (quotes/brackets/markup) after the
        //    terminator, then require whitespace/EOF and a capital/opening
        //    start for the next token.
        let mut j = i + ch.len_utf8();
        while let Some(c) = text[j..].chars().next() {
            if self.cfg.trailing_closers.contains(&c) { j += c.len_utf8(); } else { break; }
        }
        match text[j..].chars().next() {
            None => true,                              // end of text: boundary
            Some(c) if c.is_whitespace() => {
                if !self.cfg.join_hard_wraps { return true; }
                // Peek past whitespace: a boundary needs the next real char
                // to look like a sentence start (capital, quote, digit, dash).
                self.next_token_starts_sentence(text, j)
            }
            Some(_) => false, // terminator glued to more text → not a boundary
        }
        // (bytes unused placeholder to show we operate on &str safely)
        ; let _ = bytes;
    }

    fn followed_by_initial(&self, text: &str, i: usize) -> bool {
        // After the period, optional space, then an uppercase letter + period.
        let rest = &text[i + 1..];
        let mut chars = rest.chars().peekable();
        let mut seen_space = 0;
        while let Some(&c) = chars.peek() {
            if c.is_whitespace() && seen_space <= self.cfg.initialism_max_gap {
                seen_space += 1; chars.next();
            } else { break; }
        }
        matches!(chars.next(), Some(c) if c.is_uppercase())
            && matches!(chars.next(), Some('.'))
    }

    fn next_token_starts_sentence(&self, text: &str, ws_start: usize) -> bool {
        let next = text[ws_start..].chars().find(|c| !c.is_whitespace());
        matches!(next,
            Some(c) if c.is_uppercase() || c.is_ascii_digit()
                    || matches!(c, '"' | '\'' | '“' | '‘' | '(' | '[' | '—' | '*' | '_' | '`'))
            | None
        )
    }
}

/// The word (letters/’) immediately preceding a period at offset `i`.
fn trailing_word(text: &str, i: usize) -> String {
    text[..i]
        .chars()
        .rev()
        .take_while(|c| c.is_alphabetic() || *c == '\'' || *c == '’')
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}
```

### 5.2 Wrapping it as a `TextObject`

```rust
pub struct Sentence {
    detector: SentenceDetector,
}

impl TextObject for Sentence {
    fn id(&self) -> &'static str { "sentence" }
    fn label(&self) -> &'static str { "Sentence" }

    fn find(&self, buf: &dyn BufferView, cursor: usize, aff: Affinity) -> Option<Span> {
        let text = buf.rope().to_string(); // real impl: operate on rope chunks
        let start = self.scan_back_to_start(&text, cursor);
        let end_inside = self.scan_fwd_to_end(&text, cursor);
        let end = match aff {
            Affinity::Inside => end_inside,
            Affinity::Around => eat_trailing_ws(&text, end_inside),
        };
        Some(Span::new(start, end))
    }

    fn next(&self, buf: &dyn BufferView, from: usize, aff: Affinity) -> Option<Span> {
        let text = buf.rope().to_string();
        let start = self.scan_fwd_to_next_start(&text, from)?;
        self.find(buf, start, aff)
    }

    fn prev(&self, buf: &dyn BufferView, from: usize, aff: Affinity) -> Option<Span> {
        let text = buf.rope().to_string();
        let start = self.scan_back_to_prev_start(&text, from)?;
        self.find(buf, start, aff)
    }
}
```

*(Scan helpers walk the text calling `detector.boundary_after`; on a real rope, iterate chunks rather than materializing a `String` — shown here for clarity.)*

### 5.3 Starter abbreviation set

Adapted and expanded from `vim-textobj-sentence`'s defaults. Ships as `STARTER_ABBREVIATIONS`; users append via config.

```rust
pub const STARTER_ABBREVIATIONS: &[&str] = &[
    // Titles / honorifics
    "Mr", "Mrs", "Ms", "Dr", "Prof", "Rev", "Fr", "Sr", "Jr", "St",
    "Hon", "Gov", "Sen", "Rep", "Gen", "Col", "Lt", "Sgt", "Capt", "Cmdr",
    // Academic / professional
    "PhD", "MD", "BA", "MA", "BSc", "MSc", "LLB", "Esq", "RN", "DDS",
    // Latin / editorial
    "etc", "vs", "viz", "cf", "al", "ibid", "op", "seq", "inc", "incl",
    "eg", "ie",            // e.g. / i.e. (period-internal handled separately)
    // Address / geography
    "Ave", "Blvd", "St", "Rd", "Ln", "Ct", "Dr", "Apt", "Ste", "Fl",
    "No", "Mt", "Ft",
    // Time / calendar
    "Jan", "Feb", "Mar", "Apr", "Jun", "Jul", "Aug", "Sep", "Sept",
    "Oct", "Nov", "Dec", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun",
    // Business / org
    "Co", "Corp", "Ltd", "Dept", "Univ", "Assn", "Bros", "Mfg",
    // Measurement
    "approx", "min", "max", "avg", "vol", "pp", "ch", "sec", "fig",
];
```

### 5.4 Test fixtures

The suite is the contract. Each case is `(input, expected_number_of_sentences)` plus, where relevant, the expected boundary offsets. These are the "nasty cases" the module must survive.

```rust
#[cfg(test)]
mod fixtures {
    use super::*;

    struct Case { text: &'static str, sentences: usize }

    const CASES: &[Case] = &[
        // --- Baseline ---
        Case { text: "This is one. This is two.", sentences: 2 },
        Case { text: "One! Two? Three.", sentences: 3 },

        // --- Abbreviations must NOT split ---
        Case { text: "Dr. Smith arrived. He was late.", sentences: 2 },
        Case { text: "We met at 10 a.m. and left.", sentences: 1 },
        Case { text: "See p. 5 and fig. 2 for details.", sentences: 1 },
        Case { text: "The firm (Acme Corp.) folded.", sentences: 1 },
        Case { text: "He lives on Elm Ave. near the park.", sentences: 1 },

        // --- Initials / initialisms ---
        Case { text: "Magnum, P.I. lived in Hawaii.", sentences: 1 },
        Case { text: "J. R. R. Tolkien wrote it.", sentences: 1 },
        Case { text: "The U.S.A. is large.", sentences: 1 },

        // --- Decimals / versions ---
        Case { text: "Pi is 3.14 exactly.", sentences: 1 },
        Case { text: "Use v1.2.3 or newer.", sentences: 1 },
        Case { text: "It cost $4.50 total.", sentences: 1 },

        // --- Ellipsis ---
        Case { text: "Well… I suppose so.", sentences: 2 },
        Case { text: "Wait... what happened?", sentences: 2 },

        // --- Terminal punctuation inside quotes ---
        Case { text: "She said \u{201C}Go home.\u{201D} Then she left.", sentences: 2 },
        Case { text: "\u{201C}Why?\u{201D} he asked.", sentences: 1 },
        Case { text: "He shouted \u{201C}Stop!\u{201D} and ran.", sentences: 1 },

        // --- Markup-wrapped (lightweight markdown) ---
        Case { text: "This is **bold.** And this is next.", sentences: 2 },
        Case { text: "Read `code.rs` first. Then run it.", sentences: 2 },

        // --- Hard-wrapped source, join_hard_wraps = true ---
        Case { text: "This sentence is\nwrapped across lines. Next one.", sentences: 2 },

        // --- Parentheticals ---
        Case { text: "He paused (a long one) before speaking.", sentences: 1 },
    ];

    #[test]
    fn counts_match() {
        let det = SentenceDetector::new(SentenceConfig::default());
        for c in CASES {
            let n = count_sentences(&det, c.text);
            assert_eq!(n, c.sentences, "wrong count for: {:?}", c.text);
        }
    }

    // A second suite pins exact boundary offsets for a handful of cases,
    // guarding against off-by-one errors in Around/Inside affinity.
    #[test]
    fn boundary_offsets() {
        let det = SentenceDetector::new(SentenceConfig::default());
        let text = "Dr. Smith arrived. He was late.";
        //                            ^ boundary after the first '.' at the
        //                              period following "arrived"
        assert!(det.boundary_after(text, byte_of(text, '.', 2)));   // "arrived."
        assert!(!det.boundary_after(text, byte_of(text, '.', 1)));  // "Dr."
    }
}
```

The fixture list *is* the spec for the detector; every reported false-split or missed-split from real use becomes a new case here before the fix lands.

---

## 6. The Parameterized Paired-Delimiter Object

A third of the useful Vim objects — backticks, curly quotes, between-char, braces, underscores — are the *same object* with different delimiters. Rather than implement each, we build one **`PairedDelimiter`** and register instances. This covers straight and curly quotes, Markdown emphasis (`*…*`, `_…_`), strong (`**…**`, `__…__`), inline code (`` `…` ``), and links (`[…](…)`) as configured data, not code.

### 6.1 Design

```rust
/// How the opening and closing markers relate.
#[derive(Clone)]
pub enum DelimiterKind {
    /// Same string on both ends: `*`, `_`, `` ` ``, `"`, `**`.
    Symmetric { marker: &'static str },
    /// Distinct open/close, possibly directional: “ … ”, ( … ), [ … ].
    Asymmetric { open: &'static str, close: &'static str },
    /// A two-part compound: link text in [ ] immediately followed by ( ).
    /// `inside` selects one part; `around` selects the whole construct.
    Compound {
        first:  (&'static str, &'static str),  // ("[", "]")
        second: (&'static str, &'static str),  // ("(", ")")
    },
}

#[derive(Clone)]
pub struct PairedDelimiter {
    id: &'static str,
    label: &'static str,
    kind: DelimiterKind,
    /// If true, nesting is tracked (parens inside parens); if false, the
    /// nearest enclosing pair wins (typical for quotes and emphasis).
    nesting_aware: bool,
    /// For Compound: which part `Inside` targets by default.
    compound_inside_part: CompoundPart,
}

#[derive(Copy, Clone)]
pub enum CompoundPart { First, Second }

impl TextObject for PairedDelimiter {
    fn id(&self) -> &'static str { self.id }
    fn label(&self) -> &'static str { self.label }

    fn find(&self, buf: &dyn BufferView, cursor: usize, aff: Affinity) -> Option<Span> {
        match &self.kind {
            DelimiterKind::Symmetric { marker } =>
                self.find_symmetric(buf, cursor, marker, aff),
            DelimiterKind::Asymmetric { open, close } =>
                self.find_asymmetric(buf, cursor, open, close, aff),
            DelimiterKind::Compound { first, second } =>
                self.find_compound(buf, cursor, first, second, aff),
        }
    }
    // next()/prev() enumerate same-kind pairs forward/backward.
}
```

- **`Inside`** = content between the markers.
- **`Around`** = content *plus* the markers (for `Compound`, the whole `[text](url)`).
- For **quotes and emphasis**, `nesting_aware = false`: pick the nearest enclosing pair, scanning outward from the cursor.
- For **parens/brackets**, `nesting_aware = true`: balance-count so nested pairs resolve correctly.
- **Directional curly quotes** are `Asymmetric { open: "“", close: "”" }`, so the object never confuses an opening `“` with a closing `”` — the concrete failure of ASCII-quote objects that `vim-textobj-quote` was built to fix.

### 6.2 The registration table

New inline objects are added here as data — no new code:

```rust
pub fn default_inline_objects() -> Vec<PairedDelimiter> {
    use DelimiterKind::*;
    vec![
        // --- Quotations ---
        PairedDelimiter {
            id: "quote-straight-double", label: "Quotation (\")",
            kind: Symmetric { marker: "\"" },
            nesting_aware: false, compound_inside_part: CompoundPart::First,
        },
        PairedDelimiter {
            id: "quote-curly-double", label: "Quotation (\u{201C}\u{201D})",
            kind: Asymmetric { open: "\u{201C}", close: "\u{201D}" },
            nesting_aware: false, compound_inside_part: CompoundPart::First,
        },
        PairedDelimiter {
            id: "quote-curly-single", label: "Quotation (\u{2018}\u{2019})",
            kind: Asymmetric { open: "\u{2018}", close: "\u{2019}" },
            nesting_aware: false, compound_inside_part: CompoundPart::First,
        },
        // --- Emphasis / strong ---
        PairedDelimiter {
            id: "emphasis-star", label: "Emphasis (*)",
            kind: Symmetric { marker: "*" },
            nesting_aware: false, compound_inside_part: CompoundPart::First,
        },
        PairedDelimiter {
            id: "emphasis-underscore", label: "Emphasis (_)",
            kind: Symmetric { marker: "_" },
            nesting_aware: false, compound_inside_part: CompoundPart::First,
        },
        PairedDelimiter {
            id: "strong-star", label: "Strong (**)",
            kind: Symmetric { marker: "**" },
            nesting_aware: false, compound_inside_part: CompoundPart::First,
        },
        // --- Inline code ---
        PairedDelimiter {
            id: "code-inline", label: "Inline code (`)",
            kind: Symmetric { marker: "`" },
            nesting_aware: false, compound_inside_part: CompoundPart::First,
        },
        // --- Links ---  Inside → the visible text; Around → whole [text](url)
        PairedDelimiter {
            id: "link", label: "Link",
            kind: Compound { first: ("[", "]"), second: ("(", ")") },
            nesting_aware: true, compound_inside_part: CompoundPart::First,
        },
        // --- Parentheticals / brackets (nesting-aware) ---
        PairedDelimiter {
            id: "paren", label: "Parenthetical ( )",
            kind: Asymmetric { open: "(", close: ")" },
            nesting_aware: true, compound_inside_part: CompoundPart::First,
        },
    ]
}
```

### 6.3 An important caveat on emphasis and code

There is tension between the *textual* paired-delimiter approach and the *parsed* Markdown approach (§7). For emphasis, strong, and inline code, the delimiter characters are also Markdown syntax. A `PairedDelimiter` scanning raw text for `*` will mis-fire on a literal asterisk, an unmatched one, or one inside a code span. Two strategies, and we use both:

- **Fast path (typing):** the textual `PairedDelimiter` gives instant, cursor-local results while the user edits, before a reparse settles.
- **Authoritative path (settled):** once the Markdown tree is current, emphasis/strong/code/link objects prefer *parse-tree spans* (§7.2), which are correct by construction. The `PairedDelimiter` for these acts as a fallback when no tree is available (plain-text buffers) or between reparses.

Quotations and bare parentheticals, by contrast, are *not* Markdown syntax, so their `PairedDelimiter` is always authoritative. This split is the practical reason the object trait exposes both a textual and a tree-backed implementation for the same conceptual object.

---

## 7. Markdown: How Our Implementation Changes the Design

`vim-textobj-markdown` recognizes structure by scanning text with regular expressions — it re-derives "this line is an `##` heading" every time. We do not, because **we already parse Markdown to render it.** That single fact reshapes several objects and is the most important integration point in this spec.

### 7.1 Parser choice and the tree we expose

Assume a CommonMark-compliant parser (`pulldown-cmark` for an event stream, or `comrak`/`markdown-rs` for an actual AST). Whichever we use, we materialize a lightweight tree with byte spans:

```rust
pub struct MdTree {
    pub root: MdNode,
}

pub struct MdNode {
    pub kind: MdKind,
    pub span: Span,          // byte range in the source
    pub children: Vec<MdNode>,
}

pub enum MdKind {
    Document,
    Heading { level: u8 },
    Paragraph,
    BlockQuote,
    List { ordered: bool },
    ListItem,
    CodeBlock { fenced: bool, lang: Option<String> },
    Table, TableRow, TableCell,
    // inline
    Emphasis, Strong, Strikethrough,
    CodeSpan,
    Link { dest: Span, title: Option<Span> },
    Image { dest: Span },
    Text,
}
```

The key difference from the Vim approach: **structural objects become tree queries, not text scans.** They are correct by construction, handle nesting natively, and never mis-fire on syntax that merely *looks* like a heading (e.g. a `#` inside a code fence — the parse tree knows it is code; a regex does not).

### 7.2 Which objects change, and how

**Heading / Section — substantially better.** `vim-textobj-markdown` offered per-level heading *motions* (`]]`, `][`, `]}`) but no notion of a section as a *containing* region. With a tree, `Section` is trivially "the heading node plus its following siblings until a heading of equal-or-higher level," and it *nests* correctly. This directly enables promote/demote, move-section, fold-section, and count-section-words. Both ATX (`##`) and Setext (`===`/`---`) heading styles are unified by the parser, so we handle both for free — something the Vim plugin had to special-case.

```rust
pub struct Section;
impl TextObject for Section {
    fn id(&self) -> &'static str { "section" }
    fn label(&self) -> &'static str { "Section" }
    fn find(&self, buf: &dyn BufferView, cursor: usize, aff: Affinity) -> Option<Span> {
        let tree = buf.markdown()?;                 // degrade to None on plain text
        let heading = enclosing_heading(tree, cursor)?;
        let level = match heading.kind { MdKind::Heading { level } => level, _ => return None };
        let end = end_of_section(tree, heading, level); // next heading ≤ level, or EOF
        let start = match aff {
            Affinity::Inside => body_start_after_heading(heading), // exclude the heading line
            Affinity::Around => heading.span.start,                // include the heading
        };
        Some(Span::new(start, end))
    }
    // next()/prev() walk to sibling/adjacent headings.
}
```

**Code fence — becomes exact.** The Vim plugin matched ```` ``` ```` lines textually and could be fooled by fences inside blockquotes or indented contexts. The parser hands us `CodeBlock` nodes with precise spans and the language tag, so `Inside`/`Around` (exclude/include the fence lines) and "next/prev fence" are exact. Language-aware operations (e.g. "reflow prose but never a code block") fall out naturally.

**Prose block between fences — reframed.** `vim-textobj-markdown` had a special "text block" object meaning "text between code fences." In a tree model this is not special at all: it is simply a `Paragraph`/`List`/`BlockQuote` node. We drop the bespoke object in favor of generic `Block`, which is more general (it also handles the first/last block, blocks between headings, etc.).

**Emphasis / Strong / Code span / Link — dual implementation (see §6.3).** Authoritative spans come from `Emphasis`/`Strong`/`CodeSpan`/`Link` nodes; the textual `PairedDelimiter` is the fast/fallback path. For `Link`, the tree additionally gives us the `dest` span directly, so "select the URL" vs. "select the link text" vs. "select the whole link" are three exact sub-selections of one node — cleaner than the `vim-textobj-uri` regex approach.

**List item + children — indentation meets tree.** `vim-textobj-indented-paragraph` inferred nested blocks from leading whitespace. The Markdown tree already models `List` → `ListItem` → (nested `List`) containment, so "this item and its children" is a subtree, not an indentation scan. We keep the indentation heuristic only as the *plain-text* fallback when there is no tree.

### 7.3 The plain-text degradation path

Not every buffer is Markdown (a `.txt` file, a commit message, a raw note). Every structural object therefore checks `buf.markdown()` and, on `None`, falls back to a text heuristic:

| Object | Markdown (tree) | Plain text (fallback) |
| --- | --- | --- |
| Section | heading subtree | run between blank-line-separated ALL-CAPS or underlined lines, else whole doc |
| Block | Paragraph/List/Quote node | blank-line-delimited paragraph |
| List item | ListItem subtree | indentation block (à la indented-paragraph) |
| Emphasis/Strong/Code | inline node | `PairedDelimiter` text scan |
| Link | Link node | `vim-textobj-url`-style URL scan |
| Quotation | *(always text)* `PairedDelimiter` | same |
| Sentence / Clause / Word | *(always text)* | same |

The lower half of the prose hierarchy (sentence, clause, word, quotation) is *always* text-driven and identical in both modes — Markdown structure lives at the block level and above. This clean split means the sentence and clause modules never need to know whether they are in a Markdown buffer.

### 7.4 Incremental reparse and object freshness

Because objects read the tree, the tree must be reasonably current. Practical policy:

- Reparse incrementally on edit (debounced a few tens of ms), keeping byte spans valid via the rope's edit tracking.
- Structural objects tolerate a slightly stale tree: spans shift with edits between reparses using the rope's transform, and a full correctness pass lands on the next reparse.
- Inline objects on the fast path (`PairedDelimiter`) never depend on the tree at all, so typing inside emphasis or a quote is always responsive.

---

## 8. Integrating `repar`: Open Questions for the Implementer

Part of this system is already built. `repar` — a from-scratch, Markdown-aware, UTF-8-aware Rust reimplementation of `par`, embedded in the `wordcartel` prose processor — already provides `reflow`, `unwrap`, and `ventilate` as content-preserving newline transforms (§8.5). That changes this spec from "design a reflow engine" to "decide how the text-object layer and `repar` share responsibility." The sections above were written as if reflow were a hand-wavy operator; in reality it is an existing, capable subsystem, and the interesting work is at the seam between it and the object model.

This section does not prescribe answers. It frames the decisions the implementer has to make, because several of them depend on how `repar` is factored internally — knowledge the implementer has and this spec cannot assume. Each subsection poses a question, explains why it matters, and sketches the tradeoffs, so the choices are made deliberately rather than by accident of whichever code path was easiest to wire first.

### 8.1 Background: what `par`'s model contributes

Worth stating plainly, because it shapes every question below. `par`'s real contribution was never line-wrapping (`fmt` did that). It was the **prefix / suffix / body decomposition**: for any run of lines, compute the longest common prefix and suffix, treat those affixes as *structure* to be preserved, and rewrap only the *body* to width — then reattach the affixes to every output line. That is what lets `par` reflow an email quote (`> > text`) or a comment block without destroying its structure, recursively, several levels deep.

The `Reflow`, `Unwrap`, and `Ventilate` operators in §3.5 are, in effect, the object layer's names for `repar`'s newline-normalization transforms (§8.5). The question is not whether to use `repar` — it is where the boundary sits between "the editor knows the structure" and "`repar` knows the structure," now that *both* understand Markdown.

### 8.2 Where is the seam? (The decomposition-ownership question)

**Consider:** the original `par` had to *infer* structure because it had no parser — `comprelen`/`comsuflen` were its way of guessing that `> > ` is two quote levels. This editor already maintains a Markdown parse tree (§7) that *knows* a node is a nested blockquote, a depth-2 list item, or a code block that must not be touched. With a Markdown-aware `repar`, there are now **two** components that understand Markdown structure. The central integration question is how to keep them from duplicating or, worse, disagreeing.

The answer depends on how `repar` is factored, and the implementer should locate `repar` among these three shapes before writing any glue:

- **`repar` self-parses (black-box structure).** It does its own scan for blockquote prefixes, list markers, and fences. *Consideration:* two parsers can drift. A blockquote-inside-a-list that the editor's tree resolves one way and `repar` resolves another produces reflow that doesn't match the region the user sees highlighted. If this is the shape, the integration work is *reconciliation* — either feed `repar` pre-computed structure so it bypasses its own detection, or treat `repar` as an independent oracle and never let the editor's tree assert affixes that `repar` will silently override. Decide which is authoritative and enforce it.

- **`repar` separates decomposition from rewrap.** It can answer "decompose this span into (prefix, body, suffix, nesting)" independently of "rewrap this body to width." *Consideration:* this is the most flexible shape and the one to prefer if `repar` can be nudged toward it. The editor's tree and `repar`'s decomposition do the same job, so pick one as authoritative — likely the tree, since it already drives rendering and selection — and have the other consume its output. The rewrap engine stays shared and single-sourced.

- **`repar` is a single Markdown-aware entry point.** Span in, reflowed span out, structure handled invisibly. *Consideration:* least work now, most limiting later. The visible-affix UX in §8.6 needs the decomposition surfaced; a black box won't yield it. If `repar` is this shape, weigh whether the affix-transparency feature is worth refactoring `repar` to expose its internal decomposition.

**A design option that sidesteps the question:** define the decomposition contract as an explicit interface — the `(prefix, body, suffix, nesting)` shape the object layer expects — and let `repar` satisfy it however it is built. This documents the boundary regardless of what is behind it, and lets the internal factoring change later without touching callers.

### 8.3 Does the plain-text affix-inference path still earn its keep?

**Consider:** §7.3 proposed that `par`'s `comprelen`/`comsuflen` inference serve as the plain-text fallback for structural decomposition. A Markdown-aware `repar` weakens that claim, and the implementer should decide how much to invest in it. In Markdown mode, nobody needs to *guess* structure — not the tree, not `repar`. Affix inference matters only for genuinely structure-less buffers: a raw `.txt`, a commit message, pasted email. The question is whether that path is worth maintaining as a distinct, tested code route, or whether it is a rare enough case to handle with a much simpler heuristic (or to leave to `repar`'s own behavior if it already degrades sensibly on plain text). Do not carry `comprelen` as load-bearing infrastructure if the Markdown path covers the overwhelming majority of real use; demote it to a clearly-marked last resort.

### 8.4 Is there a single source of truth for sentence boundaries?

**Consider:** this is the question most likely to produce subtle incoherence if left unexamined. `ventilate` — one sentence (or clause) per line — must decide where sentences end. §5 defines a `SentenceDetector` that does exactly this, with tested handling of abbreviations, decimals, initialisms, and terminal punctuation inside quotes. If `repar`'s ventilate ships its *own* sentence-splitting logic, there are now two authorities on "where does a sentence end," and they can disagree.

The failure mode is concrete and user-visible: `repar` breaks a line after "Dr." while the sentence *object* correctly treats "Dr. Smith" as one sentence. Now "ventilate this paragraph" and "select this sentence" contradict each other, and the editor feels incoherent even though each piece is individually defensible. **The consideration:** enforce a single source of truth. Prefer having `repar`'s ventilate call into the §5 `SentenceDetector` rather than maintain a parallel splitter, even if that means restructuring `repar` to accept an injected boundary function. If that coupling is undesirable for `repar`'s independence as a tool, the alternative is to extract the detector into a shared crate both depend on. Either way, the two must not decide sentence boundaries by different rules.

A secondary consideration falls out of this: once ventilate can call the detector, **clause-granularity ventilation becomes possible** — but only because the editor supplies the clause object (§4.3), which `repar` alone has no grammar to derive. This is a capability the integration *creates* that neither system has alone: "break this dense sentence into one clause per line to see its structure, then re-collapse it." Whether to expose it is a product decision, but the architecture should leave room for it.

### 8.5 The three newline transforms preserve content, including semantic breaks

**Resolved (as built in `repar` / `wordcartel`).** The earlier framing of this section asked whether reflow and ventilate should be *inverses* and how to verify it. That framing was wrong, and the shipped design is cleaner. There are **three** newline transforms, not two, and they are not an inverse pair — they are three destinations in one *newline-normalization space* over a body that `repar` has already decomposed:

- **Reflow** — *hard* wraps: redistributes the body across physical lines with real newlines at the width boundary.
- **Unwrap** — *soft* wraps: joins the body into one logical line per structural unit, with no hard breaks, leaving wrapping to the display.
- **Ventilate** — one sentence (or clause) per physical line, via the §5 detector.

All three are total, idempotent, and content-preserving: they change only where *layout* newlines fall. Critically, **`repar` does not reflow or unwrap Markdown's semantic line breaks** — a trailing-double-space break, a backslash-newline break, and the other authored hard breaks are treated as content, not as wrap artifacts, and every transform leaves them intact. So the content-vs-layout classification that a naive reflow engine would have to get right is already correct in `repar`: layout breaks are freely redistributed, semantic breaks are boundaries the transforms respect, exactly as they respect a blockquote prefix.

This closes what would otherwise have been the one real correctness hazard in the reflow story. A reflow engine that could not tell an authored break from a mechanical one would let Unwrap silently eat a stanza line or an address-block break, and that feels *broken* to a writer in a way no amount of correct width-wrapping compensates for. `repar` does not have that failure mode, so there is nothing here to build — only to preserve. The relevant implementer discipline is a regression one: keep a fixture suite asserting that semantic breaks survive all three transforms (across nested quotes, loose vs. tight lists, verse, and address blocks), so the property is not lost in a future change to `repar`'s decomposition.

**A clarifying consequence for the seam:** of the three transforms, only **Ventilate** consults the sentence detector. Reflow and Unwrap are content-agnostic — they need the decomposed body and (for Reflow) a width, nothing more. This sharpens §8.4: the shared substrate is `repar`'s decompose-and-redistribute engine, and the sentence detector is a dependency of exactly one operator. Single-sourcing sentence boundaries (§8.4) therefore affects only Ventilate, not the reflow subsystem as a whole.

**A clarifying consequence for the seam:** of the three transforms, only **Ventilate** consults the sentence detector. Reflow and Unwrap are content-agnostic — they need the decomposed body and (for Reflow) a width, nothing more. This sharpens §8.4: the shared substrate is `repar`'s decompose-and-redistribute engine, and the sentence detector is a dependency of exactly one operator. That is a clean seam, and it means single-sourcing sentence boundaries (§8.4) affects only Ventilate, not the whole reflow subsystem.

### 8.6 How much of the decomposition should be made *visible*?

**Consider:** a non-modal editor with explicit marks (§8.8) can do something `par` never could — show the writer what it considers structure versus prose *before* committing a reflow. When a blockquote is marked, the computed `>` prefix can be highlighted distinctly from the body, so the user sees exactly what will be preserved and what will be rewrapped. `par` computes this invisibly; this editor can make "here is what I treat as decoration versus content" an inspectable, correctable thing.

This is a genuine UX advantage, but it is only available if the decomposition is surfaced (§8.2) rather than buried inside a single reflow call. The consideration for the implementer: decide early whether affix-transparency is a feature you want, because it constrains the seam. If yes, `repar` must expose its decomposition, and the mark-rendering layer needs a way to paint prefix/body/suffix as distinct spans. If it is deferred, at least do not foreclose it by baking the decomposition irretrievably into `repar`'s internals.

### 8.7 Does the whole pipeline agree on what a "character" is?

**Consider:** `repar` being UTF-8-aware from scratch is a real asset, and the implementer should make sure the rest of the system meets it at the same standard. Reflow width must be computed in **display columns**, not bytes or codepoints: CJK ideographs are double-width, combining marks are zero-width, and emoji/ZWJ sequences defy naive counting. A reflow that wraps by byte length breaks non-Latin prose at the wrong place. If `repar` already handles display width correctly (via `unicode-width` or equivalent), then `Reflow` is correct for international prose — which, for this editor's intended use, is not a hypothetical requirement.

The deeper consideration is *offset agreement*. The sentence and clause modules (§5) iterate `char`s over `&str`; the paired-delimiter objects (§6) scan for multi-byte glyphs like curly quotes and em dashes; `repar` computes affixes and wrap points. All of these produce and consume byte offsets into the same rope. They can only share spans if they agree that offsets land on `char` boundaries and mean the same positions. UTF-8-native `repar` makes this agreement natural rather than fragile — but the implementer should treat "every component indexes the buffer identically" as an invariant to test, not to assume, especially at the boundaries where a span produced by an object is handed to `repar` or vice versa.

### 8.8 How do marks reshape the operator layer around `repar`?

**Consider:** this editor is not modal. It has explicit marks — blocks and selections the user establishes as a separate step from acting on them. §3.5's operator layer was written with a modal, cursor-resolves-the-object assumption; the non-modal reality *simplifies* the design and the implementer should lean into that rather than porting the modal shape.

The resolution order inverts helpfully. In a modal editor the object *is* the selection mechanism (one motion computes a sentence span and deletes it). Here, selection and action are already distinct in the user's mind, so text objects play two separate roles:

- **Object as selection-maker.** "Mark the current sentence," "mark this section," "extend the mark to the enclosing clause." The object's `find` produces a span that *becomes* the mark. This is expand/shrink selection (§4.3), and it is arguably more natural non-modally, because the user sees the mark and can adjust it before acting.
- **Object as operator-scope — often unused.** When a mark already exists, `Reflow`, `Unwrap`, `Ventilate`, `Count`, and case transforms act on the marked span directly. The object registry is not consulted at all.

So the operator entry point should distinguish the two, and the type should reflect that these are genuinely different situations:

```rust
enum Scope {
    Marked(Span),                 // user already marked a region: act on it
    Object(ObjectId, Affinity),   // no mark: resolve object at cursor, then act
}

fn apply(&mut self, op: Operator, scope: Scope) -> ActionResult;
```

`repar`-backed operators (`Reflow`, `Unwrap`, `Ventilate`) are the common case for `Scope::Marked`: mark a paragraph, ventilate it; mark a section, reflow all its blocks; mark a ventilated block, unwrap it. The object layer feeds these operators only when nothing is marked.

**A specific consequence for `Transpose`:** §3.5 defined transpose modally as "swap the object at the cursor with the next one of the same kind." Non-modally, the natural gesture is different and better: mark two regions, then "swap marks." Marking makes multi-region operations legible in a way modal transpose never is — the user *sees* both clauses highlighted before swapping them. The implementer should redesign `Transpose` around two marks rather than cursor-plus-next, and consider whether the second region can be inferred (the adjacent same-kind object) when only one is marked, as a convenience rather than the primary path.

### 8.9 Summary of what to resolve before building the reflow integration

In rough priority order, the implementer should settle:

1. **The seam (§8.2)** — which of the three factorings `repar` has, and therefore who owns decomposition. Everything else depends on this.
2. **Sentence-boundary single-sourcing (§8.4)** — unify `repar`'s ventilate splitting with the §5 detector, or extract a shared crate. Incoherence here is user-visible.
3. **Offset agreement (§8.7)** — confirm every component indexes the rope identically; make it a tested invariant.
4. **The `Scope::Marked | Scope::Object` split (§8.8)** — rework the operator layer for the non-modal reality; redesign `Transpose` around marks.
5. **Semantic-break preservation (§8.5)** — already correct in `repar` (it does not reflow or unwrap Markdown's semantic line breaks); keep a regression fixture so the property is not lost. **Affix transparency (§8.6)** — decide whether to surface `repar`'s decomposition for the visible-affix UX.
6. **Plain-text fallback (§8.3)** — decide how much to invest; likely demote from load-bearing.

---

## 9. Object Registry and Command Wiring

Objects register by id; commands and keybindings resolve objects by id at call time. This is what lets keybindings stay a thin, reconfigurable layer (§1.2, point 3).

```rust
pub struct ObjectRegistry {
    objects: HashMap<&'static str, Box<dyn TextObject>>,
}

impl ObjectRegistry {
    pub fn with_defaults(cfg: &Config) -> Self {
        let mut objects: HashMap<&'static str, Box<dyn TextObject>> = HashMap::new();
        objects.insert("word",      Box::new(Word));
        objects.insert("sentence",  Box::new(Sentence::new(cfg.sentence.clone())));
        objects.insert("clause",    Box::new(Clause::new(cfg.clause.clone())));
        objects.insert("block",     Box::new(Block));
        objects.insert("section",   Box::new(Section));
        objects.insert("entire",    Box::new(Entire));
        for pd in default_inline_objects() {
            objects.insert(pd.id(), Box::new(pd));
        }
        ObjectRegistry { objects }
    }
    pub fn get(&self, id: &str) -> Option<&dyn TextObject> {
        self.objects.get(id).map(|b| b.as_ref())
    }
}
```

A command is then `(Operator, object_id, Affinity)`:

```rust
pub struct Command { pub op: Operator, pub object_id: &'static str, pub aff: Affinity }

// Example bindings — names, not cryptic mnemonics:
//   "select inside sentence"   → Command { Select,   "sentence", Inside }
//   "delete around quotation"  → Command { Delete,    "quote-curly-double", Around }
//   "transpose clause"         → Command { Transpose, "clause",   Inside }
//   "move to next section"     → Command { Move(Forward, Start), "section", Inside }
//   "reflow paragraph"         → Command { Reflow,    "block",    Around }
//   "count words in section"   → Command { Count,     "section",  Inside }
```

Every command is discoverable in the palette by `label()`; keybindings are optional accelerators.

---

## 10. Build Order

A staged plan that validates the architecture early and defers the hard/optional parts:

**Phase 1 — Skeleton.** `Span`, `Affinity`, `BufferView`, `TextObject`, the operator layer, and the registry. Implement `Word` and `Block` (blank-line paragraph) only. Wire `Select`/`Delete`/`Change`/`Yank`. Prove the object × operator matrix fills in. *Exit criteria: all four operators work on both objects with no object-specific operator code.*

**Phase 2 — Sentences.** The sentence-disambiguation module (§5) with the starter abbreviations and the full fixture suite. Add `Move`, expand/shrink selection across word→sentence→block. *Exit criteria: every fixture in §5.4 passes; expand-selection walks the hierarchy.*

**Phase 3 — Inline objects.** `PairedDelimiter` (§6) with the registration table: quotes (straight + curly), emphasis, strong, inline code, links, parens. Fast-path only (textual). *Exit criteria: select/delete/change inside and around each delimiter type; curly-quote directionality correct.*

**Phase 4 — Markdown structure.** Wire the parse tree into `BufferView`; implement `Section`, tree-backed `Block`, `CodeBlock`, list-item subtree, and the authoritative inline spans (§7.2). Add the plain-text degradation table (§7.3). *Exit criteria: section move/promote/demote; fences exact; inline objects prefer tree spans when fresh.*

**Phase 5 — The distinctive layer.** `Clause` (rule-based split on punctuation + conjunctions) and `Transpose` over clauses and sentences. `Reflow` and `Count` operators across all prose objects. *Exit criteria: "move the subordinate clause to the front" works as one command.*

**Phase 6+ — Intelligence (optional).** Swap the clause/sentence heuristics for a POS/dependency backend behind the same trait; add `Phrase`. No changes to operators, registry, or keybindings — the seam holds.

---

## 11. Summary of the Argument

The Vim text-object ecosystem proved that *named, composable selections* are how expert users edit. That ecosystem is overwhelmingly code-shaped: functions, blocks, arguments, camelCase. Prose has an equally rich structure — a real hierarchy from document down to clause, plus a typographic layer of quotes, emphasis, and links — that no mainstream editor exposes as objects.

This spec adapts the proven abstraction (object = cursor→span + inside/around, operators consume spans) and rebuilds its contents for prose:

- a **sentence module** that treats boundary detection as the heuristic, test-driven, user-extensible problem it actually is;
- a **parameterized paired-delimiter object** that collapses a third of the Vim catalog into one configurable primitive, with proper curly-quote support;
- a **prose hierarchy** — including the code-less **clause** object — that makes expand/shrink selection and cross-scale operators (reflow, count, transpose) natural;
- a **Markdown integration that leans on the parse tree we already build**, making structural objects correct by construction rather than regex-scraped, while degrading cleanly to text heuristics for non-Markdown buffers;
- and an **integration with the existing `repar` engine** (in `wordcartel`) whose seams (§8) are posed as explicit research questions rather than settled architecture, because the right answers depend on how `repar` is internally factored — the load-bearing decisions being who owns structural decomposition, single-sourcing sentence boundaries between Ventilate and the §5 detector, and reshaping the operator layer around this editor's non-modal mark model. (`repar` already preserves Markdown's semantic line breaks through all three newline transforms, so that correctness question is closed rather than open.)

The payoff is a set of editing operations — transpose these clauses, delete this parenthetical, move this section, reflow this list item, unwrap that block to soft-wrapped prose, ventilate a dense paragraph clause-by-clause, count this sentence — that follow inevitably from the object model and that no code editor would ever offer. Much of the reflow machinery already exists in `repar`, with Reflow, Unwrap, and Ventilate built as content-preserving transforms over a decomposed body; the work ahead is less about building it than about deciding, deliberately, where the object layer ends and `repar` begins. That is the case for building it.
