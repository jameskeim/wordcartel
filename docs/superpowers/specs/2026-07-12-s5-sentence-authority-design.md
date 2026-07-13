# S5 — Sentence authority: SPEC

**Status:** SPEC (implementation-grade), authored 2026-07-12 by promoting the approved design in
place. Every locked decision of the approved design (§1–§11) is preserved; each is expanded to
source-grounded, implementer-ready detail. **Entering the Codex spec gate.**

**Item:** backlog **S5** — the first item of the prose-structure arc
(`docs/design/prose-structure-arc.md`: S5 → S6 → S4 → S7 → S8). Ships alone; everything downstream
stands on it.

**Grounding:** the S5 grounding map (anchors verified against real source 2026-07-12), plus live
probes executed while authoring this spec: a `unicode-segmentation` scratch crate (every UAX-29
segment shape quoted in §4 was printed, not reasoned) and the `repar` CLI driven with the shell's
exact fixup stack (§6). Line anchors in this document are the grounded-current ones; where the
approved design's anchors had drifted, this spec supersedes them.

**Grounding corrections folded in** (design claims corrected against probes/source; decisions
unchanged):
- repar's ventilate stop-list has **25** entries, not 24 (`repar/src/transform.rs:72-75`).
- There is **no public `run_transform` in repar**; the differential suite drives the SHELL's
  `wordcartel::transform::run_transform` (`wordcartel/src/transform.rs:326`), which wraps
  `repar::Options::from_par_args(..).format(..)`.
- The CUA bound-Alt-chord list in the design included `k` and `f4`; both are actually free
  (`keymap.rs:649` comment; `alt-f4` appears only in a doc-comment parse example, `keymap.rs:54`).
  `Alt+a`/`Alt+e` remain free, as the design stated.
- The design's claim that repar "preserves [Markdown hard breaks] through all three transforms" is
  **false for ventilate** (probed: `"Roses are red,  \nViolets are blue."` ventilates to ONE line —
  par tokenizes on whitespace, so the two-space marker cannot survive). The R2 hard-break exception
  (§4.5) stands on its own merit — our `select_sentence` must not swallow a verse line — but the
  hard-break fixture moves to the differential suite's divergence LEDGER (§6, entry L5) instead of
  the equality corpus.

---

## 1. Scope — three bugs, not one

The filed item named one bug. Grounding found three. All *(probed)* against the real
`textobj::sentence_bounds` (`wordcartel-core/src/textobj.rs:43`).

| Bug | Behavior today | Frequency |
|---|---|---|
| **Hard wraps** — UAX-29 (SB4) ends a sentence at **every newline** | In any reflowed document, `select_sentence` selects **a line**; Focus=Sentence focuses **a line** | **DOMINANT.** Self-inflicted: wordcartel's own `reflow` (`wordcartel/src/transform.rs`, via `run_transform(TransformKind::Reflow, ..)`) creates the condition |
| **Abbreviations** | `sentence_bounds("Dr. Smith arrived. He was late.", 0)` → `(0,4)` == `"Dr. "` | Common |
| **Lowercase after a closing quote** | `sentence_bounds("“Why?” he asked.", 0)` → `"“Why?” "` | Occasional |

Probe transcript (hard-wrap case — *exactly what our `reflow` emits*):

```
text:  "The committee met on Tuesday and the\nchair insisted on a vote. Then we left."
caret  0 → (0, 37)  == "The committee met on Tuesday and the\n"    ← A LINE
caret 40 → (37, 63) == "chair insisted on a vote. "
```

Plus: the **differential fixture suite** (§6), and **sentence motions** (§7).

An item called "sentence authority" that still selects a line in a reflowed document has not earned
its name. The hard-wrap fix is in scope.

## 2. Blast radius (why this is safe to change now)

Complete caller set of `sentence_bounds` (verified 2026-07-12):

| Site | Path |
|---|---|
| `commands.rs:212-217` — the `Scope::Sentence` arm of `scope_range_at` (fn at `commands.rs:195`) | → `select_sentence` (`registry.rs:330`) |
| `commands.rs:439-456` — the `ExpandSelection` ladder `[Word, Sentence, Paragraph, Document]` | same |
| **`render.rs:488-512` — `gather_row_ctx`, Focus mode, PER FRAME** | → `RowCtx.focus_region` (`render.rs:472`) → `row_is_active` (`render.rs:27-33`), consumed by the per-row dim decision (`render.rs:756-763`) |
| `textobj.rs:90-103` — unit tests (`sentence_bounds_basic`, `empty_window_is_safe`) | — |

**No operator mutates text using a sentence span today** (`textops.rs::scope_or_word`,
`wordcartel/src/commands/textops.rs:22-29`, uses `Scope::Word` only; `mouse.rs:591/595` uses Word
and Paragraph). The blast radius is **selection + one paint decision. There is no data-loss path.**
This is the window to change the contract; after `transpose_sentences` (S4) ships, it becomes a
migration.

## 3. DECISION — the contract: content-only, and total

`sentence_bounds` returns the sentence **without trailing whitespace**.

### 3.1 The signature — unchanged

```rust
pub fn sentence_bounds(text: &str, pos: usize) -> (usize, usize)
```

(`wordcartel-core/src/textobj.rs:43`; module `wordcartel_core::textobj`.) It stays **total**,
**non-`Option`**, byte-offset in/out, `pos` clamped into `0..=text.len()` (the module contract,
`textobj.rs:1-3`, is unchanged: the shell passes the caret's containing leaf-block slice as `text`,
so work stays paragraph-bounded). Both existing call shapes —
`scope_range_at`'s `sentence_bounds(&win, h - ps)` and `gather_row_ctx`'s
`sentence_bounds(&win, head - ps)` — compile and behave unchanged.

### 3.2 The attach rule — exact and doc-commented

For any `pos` (after clamping):

1. `pos` inside a sentence's content span `[from, to)` → that sentence.
2. `pos` in the **gap** between two sentences' content (trailing whitespace, a hard-wrap `\n`, an
   inter-sentence space), or past the last sentence's content → the **PRECEDING** sentence.
3. `pos` **before the first sentence's content** (possible only when the window opens with
   whitespace-only material, e.g. a leading whitespace-only hard-break line) → the **FOLLOWING**
   (first) sentence.
4. A window with **no sentence content at all** (empty, or whitespace-only) → the zero-width point
   `(0, 0)`, preserving `sentence_bounds("", 0) == (0, 0)` (`textobj.rs:102`).

Deliberately **not** `Option`. Making it optional forces a signature change through
`scope_range_at` → `SelectScope` → the expand ladder → **and the renderer**, all structurally total
today, for zero user-visible benefit: the honest answer for a gap caret is "the sentence you just
finished," not "nothing." Introduce `Option` when an object arrives that genuinely has no answer
(a Quotation with no quotes nearby) — with that object, not before.

### 3.3 The deliberate, visible test flip

`textobj.rs:95` pins the old contract:

```rust
assert_eq!(sentence_bounds(t, 2), (0, 9));   // "One two. "     t = "One two. Three four."
```

Under this spec it becomes `(0, 8)` — `"One two."`, trailing space dropped. **This line is
rewritten in the same commit as the implementation, as a documented contract change** (comment on
the assertion naming S5 and "content-only"), NOT quietly "fixed" by an implementer. The sibling
assertion `sentence_bounds(t, 12) == (9, 20)` (`textobj.rs:94`) is unchanged — `"Three four."` has
no trailing whitespace. The nearby `expand_then_shrink_round_trips` (`commands.rs:1134`) survives
untouched — its sentence-step assertion is `starts_with("One two.")`.

### 3.4 Why content-only (rationale — load-bearing, keep)

1. **Nothing depends on the current contract.** The renderer is provably insensitive —
   `row_is_active` (`render.rs:27-33`) is a half-open **row** overlap test, and the trailing `\n`
   is the last byte of the sentence's own line, so the region end lands exactly at the next row's
   start and the next row is never lit. `select_sentence` is the only visible consumer, and it
   currently paints a highlighted blank after the period.
2. **Every future operator requires it.** A gap-preserving `transpose_sentences` must swap content
   and preserve the inter-sentence gap verbatim (exactly as `transpose_words` does:
   `format!("{word2}{gap}{word1}")`, `textops.rs:165-210`). With UAX spans, every operator would
   re-implement the trim.
3. **It repairs the expand ladder.** In a single-sentence paragraph `"One two.\n"`, Sentence ==
   Paragraph == `(0,9)` today, so the strictly-larger containment test
   (`f <= cf && t >= ct && (f < cf || t > ct)`, `commands.rs:447`) makes the Sentence rung
   **silently collapse**. With content-only, `(0,8) ⊂ (0,9)` and the rung survives. Pinned by a
   regression test (§12, T-9).
4. It is what the selection highlight should show.

An "around" span (content + trailing gap), if ever wanted for a delete-sentence, is a **separate
helper** — not a flag on this function.

Notes on span edges: the trim is **trailing only** — leading bytes are never trimmed (UAX-29
attaches whitespace backward, so a sentence's segment never begins with whitespace except in the
window-opening case covered by attach rule 3). A Markdown backslash hard break's `\` is **content**
(the marker is authored text) and stays inside the span; the two-space hard break's spaces are
whitespace and are trimmed. `\r` is ordinary content to this module (no CRLF special-casing);
predicates key on `\n` only.

### 3.5 New public core API (the only additions)

```rust
/// All sentence content spans of `text`, in order — the S5 post-pass over UAX-29.
pub fn sentence_spans(text: &str) -> impl Iterator<Item = (usize, usize)> + '_

/// Start of the sentence STRICTLY BEFORE `pos` in span-start order — i.e. the
/// greatest sentence start < pos. Emacs M-a's kernel. None at/before the first start.
pub fn prev_sentence_start(text: &str, pos: usize) -> Option<usize>

/// End (content end, per the §3 contract) of the first sentence whose end is
/// STRICTLY AFTER `pos`. Emacs M-e's kernel. None past the last content end.
pub fn next_sentence_end(text: &str, pos: usize) -> Option<usize>
```

All three live in `wordcartel-core/src/textobj.rs`, beside `next_word_start`/`prev_word_start`
(`textobj.rs:27/35`), whose `Option` shape the motion pair mirrors. The asymmetric pair
(`prev_…start` / `next_…end`) is deliberate: it encodes the Emacs M-a/M-e asymmetry (§7) so each
nav fn is ONE core call — a symmetric `next_sentence_start` would force nav to re-derive ends.
`sentence_bounds` becomes a thin consumer of `sentence_spans`:

```rust
pub fn sentence_bounds(text: &str, pos: usize) -> (usize, usize) {
    let pos = pos.min(text.len());
    let mut prev: Option<(usize, usize)> = None;
    for (from, to) in sentence_spans(text) {
        if pos < from { return prev.unwrap_or((from, to)); }  // gap → preceding; block start → following
        if pos < to { return (from, to); }
        prev = Some((from, to));
    }
    prev.unwrap_or((0, 0))                                    // past last → last; no content → (0,0)
}
```

`prev_sentence_start`: fold `sentence_spans`, keep the last `from < pos`, `break` once `from >= pos`.
`next_sentence_end`: return the first `to > pos`. Both are `O(spans)` and allocation-free, same as
`sentence_bounds` itself.

## 4. DECISION — the algorithm: a four-rule post-pass over UAX-29

**Not a from-scratch scanner.** UAX-29 already gets all of the following right, for free
*(probed — every line below was executed against `unicode-segmentation` 1.x,
`split_sentence_bound_indices()`, the `_indices` variant the code already uses, `textobj.rs:46`)*:

```
Pi is 3.14 exactly. → 1      The U.S.A. is large. → 1        Well… I suppose so. → 1
We met at 10 a.m. and left. → 1   Use v1.2.3 or newer. → 1   Wait... what happened? → 1
Magnum, P.I. lived in Hawaii. → 1  It cost $4.50 total. → 1  He paused (a long one)… → 1
The firm (Acme Corp.) folded. → 1  He lives on Elm Ave. near… → 1
See p. 5 and fig. 2 for details. → 1
She said “Go home.” Then she left. → 2   (boundary correctly AFTER the ”)
It was (fine.) Then he left. → 2         (Close-class ) also correctly attached)
```

A from-scratch scanner must **re-earn every one of those**. Its failure modes, by contrast, are a
short closed list. So: fold four rules over the UAX segment list.

| Rule | Action | Cures |
|---|---|---|
| **R1** | **merge** when the previous span's last token is a known abbreviation, or a single capital + `.` | `"Dr. "`; `"J. R. R. "`; `"Mr. and Mrs. "` |
| **R2** | **merge** when the previous span does **not** end in a terminator and the break was caused by a line separator | **the hard-wrap bug** |
| **R3** | **merge** when the next segment begins with a **lowercase** letter | `"“Why?” he asked."`; `"He shouted “Stop!” and ran."` |
| **R4** | **shift** the boundary past a run of closing markup after a terminator | `"This is **bold.** And this is next."` |

### 4.1 Data flow

`sentence_spans(text)`:

1. Iterate `text.split_sentence_bound_indices()` — yields `(byte_start, &str)` UAX-29 segments.
2. Fold adjacent segments into **merged spans** by evaluating, at each inter-segment boundary:
   R4 (a boundary *shift*), then merge iff **R1 ∨ R2 ∨ R3**. Whitespace-only segments never start
   or end a span (§4.7).
3. Emit each span **content-only**: end = raw end minus the trailing run of
   `char::is_whitespace` bytes (§3).
4. `sentence_bounds` selects the span for `pos` by the attach rule (§3.2/§3.5);
   `prev_sentence_start`/`next_sentence_end` fold the same iterator.

The fold is **lazy, allocation-free, `O(segments in the block)`** with `O(bytes in the block)`
worst-case char scanning — the same asymptotic as today's loop.

### 4.2 The iterator — shape decision

A shared **private iterator struct** (three public fns consume the same fold — duplicating the fold
three times is the alternative, and it is worse):

```rust
struct SentenceSpans<'a> {
    text: &'a str,
    segs: core::iter::Peekable<unicode_segmentation::USentenceBoundIndices<'a>>,
    // plus a small carry for an R4-consumed segment remainder (start offset), if the
    // implementation chooses to represent it explicitly.
}
```

`unicode_segmentation::USentenceBoundIndices<'a>` is the public concrete type returned by
`split_sentence_bound_indices()` (verified by compiling against it). `sentence_spans` returns
`impl Iterator<Item = (usize, usize)> + '_` so the struct stays private. **No segment-metadata
Vec, no String** — the fold works on `(usize, &str)` pairs and byte offsets only.

`next()` logic (normative pseudocode; helper fns keep every real fn under the 100-line
`clippy::too_many_lines` gate):

```
open A := next content-bearing unit (skip whitespace-only segments; None → iterator done)
loop:
    N := peek next content-bearing unit (whitespace-only segments are gap; see §4.7)
    if N is None: emit (A.start, content_end(A)) and finish this item
    // R4 — boundary shift (§4.3); may consume N entirely, in which case re-peek
    if r4_fires(A, N):
        A.content_end := N.start + closer_run_len(N)
        N := remainder of N after the run, leading whitespace skipped
        if N is empty: consume it; continue
    if r1(A) or r2(A, N) or r3(N):        // §4.4, §4.5, §4.6
        absorb N into A (A.raw_end := N.raw_end; A.content_end := content_end(N)); continue
    else:
        emit (A.start, content_end(A))    // N stays pending — it opens the next span
```

where `content_end(x)` = x's end minus its trailing `char::is_whitespace` run, and
`E(A) = &text[A.start..A.content_end]` is A's **effective content** (used by every predicate; note
it includes any R4 extension already applied).

### 4.3 R4 — shift past closing markup (evaluated first)

UAX-29 already attaches Close-class characters (`)`, `]`, `"`, `”`, `’`, …) to the preceding
sentence *(probed: `"She said “Go home.” "`, `"It was (fine.) "`)*. What it gets wrong is the
**Markdown emphasis closers**, which are SB-class Other *(probed)*:

```
"This is **bold.** And this is next."  → segs "This is **bold."  +  "** And this is next."
"It was _quiet._ Then he left."        → segs "It was _quiet."   +  "_ Then he left."
```

**Alphabet** (the design's set, verbatim — a superset of what UAX needs help with, harmless where
UAX already did the work):

```rust
/// Closing markup/punctuation that may trail a sentence terminator (R4).
const CLOSERS: &[char] = &['*', '_', '`', ')', ']', '"', '\'', '”', '’', '»'];
```

**Predicate.** R4 fires at the boundary between A and next segment N iff:
- N begins with a non-empty maximal run of `CLOSERS` chars, AND
- the char immediately after that run within N is whitespace, or the run is the whole of N, AND
- `ends_terminated(E(A))` — where `ends_terminated(e)` = strip the trailing run of `CLOSERS` from
  `e`, then its last char is in `TERMINATORS` (§4.5's constant).

**Action.** `A.content_end += run.len()` (the closers become sentence content — the span for
`"This is **bold.** And…"` is `"This is **bold.**"`, byte range `(0, 17)`); the remainder of N
after the run, with leading whitespace skipped, becomes the pending next unit (that skipped
whitespace joins the gap). If the remainder is empty (`"**"` at end of text), N is consumed and the
fold re-peeks.

### 4.4 R1 — abbreviation / single-capital merge

Evaluated on `E(A)` (so an R4-extended tail is seen as-is; a tail ending in `**` simply fails the
`.`-suffix test and R1 defers to R2/R3 — correct, since the terminator there is real).

```rust
fn r1_merge(e: &str) -> bool {
    let tok = e.rsplit(char::is_whitespace).next().unwrap_or("");
    let Some(t0) = tok.strip_suffix('.') else { return false };
    let mut cs = t0.chars();
    let single_capital = matches!((cs.next(), cs.next()), (Some(c), None) if c.is_uppercase());
    single_capital || ABBREV_ALWAYS_MERGE.iter().any(|a| t0.eq_ignore_ascii_case(a))
}
```

- **Last token** = the substring after the last `char::is_whitespace` in `E(A)` (the whole content
  if it has no whitespace).
- It must **end with `.`** (an abbreviation break can only follow a full stop); strip exactly that
  one final dot.
- **Single-capital-initial rule**: the stripped token is exactly one char and it
  `char::is_uppercase` → merge (`"J. R. R. Tolkien"`; matches repar's own initials rule,
  `repar/src/transform.rs:67-71`). `"Q.E.D."` strips to `"Q.E.D"` — multi-char, not matched —
  break stands (Q.E.D. is frequently sentence-final; probed UAX shape confirms the break exists to
  adjudicate).
- **List match**: case-insensitive ASCII (`eq_ignore_ascii_case`) against `ABBREV_ALWAYS_MERGE`
  (§5) — the list is all-ASCII by construction, so ASCII folding is exact.

### 4.5 R2 — the hard-wrap merge (THE fix), with the hard-break exception

Under UAX-29, a break happens only after a terminator sequence or at a mandatory line separator
(SB4). So "previous span lacks a terminator" ⟺ "the break was newline-caused" — R2 requires both
anyway (belt and braces, and it makes the predicate self-documenting).

```rust
/// Sentence terminators the post-pass recognizes. The CJK members make R2 honor
/// an ideographic full stop at a hard-wrapped line end (。！？); UAX already
/// breaks at them intra-line.
const TERMINATORS: &[char] = &['.', '?', '!', '…', '。', '！', '？'];
```

**Predicate.** Let `gap = &text[A.content_end..N.start]` (A's trailed whitespace — for adjacent
segments the raw boundary sits inside it). R2 fires iff:
- `gap` contains `'\n'` (the break was at a line separator), AND
- `!ends_terminated(E(A))` (same helper as §4.3: closers-stripped tail's last char not in
  `TERMINATORS`), AND
- **NOT a Markdown semantic hard break** at that newline.

**The hard-break EXCEPTION — the one place this fix could lose authored meaning.** A semantic hard
break must keep its two sides separate sentences, or `select_sentence` would swallow a verse line
or an address-block line. Detection, exact: let `nl` = the absolute byte offset of the FIRST `'\n'`
in `gap`; the break is semantic iff

```rust
text[..nl].ends_with("  ") || text[..nl].ends_with('\\')
```

i.e. two-or-more trailing spaces before the newline (`"  \n"` — `ends_with("  ")` also matches
longer runs, matching Markdown's "two or more"), or a backslash immediately before the newline
(`"\\\n"`). The check runs against the **whole window text** up to `nl`, not against `gap` alone —
probed shapes force this: in `"Roses are red,  \n…"` the two spaces live in the gap (content end is
after the comma), while in `"A line\\\n…"` the backslash is **content** (the last byte of
`E(A)`). One predicate covers both. Fixtures pin both shapes plus a one-trailing-space control
(one space = soft wrap → merge) — §12, T-4.

### 4.6 R3 — lowercase continuation

```rust
fn r3_merge(n: &str) -> bool {           // n = the pending next unit, post-R4
    n.chars().next().is_some_and(char::is_lowercase)
}
```

Strictly the **first char** of the next unit; `char::is_lowercase` is Unicode-aware (so `é` merges,
`中`/digits/punctuation do not). No skipping of leading markup or punctuation — a next unit opening
with `(`/`*`/`>` does not R3-merge (deliberate: skipping would need a markdown-aware tokenizer this
module doesn't have; residue recorded in §10). Note UAX-29's own SB8 already merges
lowercase-after-`.` intra-line *(probed: `"Acme Co. filed suit."` → 1 seg)*; R3's real work is
after `?`/`!` and closing quotes, where UAX has no lowercase rule *(probed: `"“Why?” he asked."` →
2 segs)*.

### 4.7 Whitespace-only segments and degenerate windows

A UAX segment whose content is empty after the trailing-whitespace trim (e.g. a lone `"\n"`, or a
`"  \n"` hard-break line of only spaces) is **gap material**: it never opens a span, never closes
one, and is skipped by the fold (its bytes end up in the inter-span gap, which the attach rule
already handles). Consequences, all pinned by tests (§12, T-2):
- `sentence_spans("")` and `sentence_spans("\n")` yield nothing; `sentence_bounds` returns `(0,0)`
  for both (attach rule 4).
- A window opening with whitespace-only material starts its first span at the first content
  segment's start (> 0), which is the only way attach rule 3 (block-start → following) fires.

### 4.8 The allocation budget — mandatory

R1–R3 are folds over segment indices; R4 adjusts an offset. **Allocation-free, `O(segments in the
block)`.** This budget is a hard constraint, not a preference: `gather_row_ctx`
(`render.rs:488-512`) calls `sentence_bounds` **per frame** when Focus=Sentence is on, and that
path already allocates one `String` per frame via `buf.slice(ps..pe)` (`render.rs:505`). **Do not
add a second allocation** — no `Vec` of segments, no `String` normalization, no regex. The
differential suite and unit tests may allocate freely; the production path may not.

### 4.9 Probe ledger (verified UAX shapes the rules are built against)

All executed 2026-07-12 against `unicode-segmentation` (workspace-pinned major, `"1"` in
`wordcartel-core/Cargo.toml:12`):

| Input | UAX segments | Post-pass result |
|---|---|---|
| `"Dr. Smith arrived. He was late."` | `"Dr. "` · `"Smith arrived. "` · `"He was late."` | R1 merges 1+2 → 2 sentences |
| `"The committee met on Tuesday and the\nchair insisted on a vote. Then we left."` | `"…the\n"` · `"chair …vote. "` · `"Then we left."` | R2 merges 1+2 → 2 sentences |
| `"“Why?” he asked."` | `"“Why?” "` · `"he asked."` | R3 merges → 1 |
| `"He shouted “Stop!” and ran."` | `"He shouted “Stop!” "` · `"and ran."` | R3 merges → 1 |
| `"This is **bold.** And this is next."` | `"This is **bold."` · `"** And this is next."` | R4 shifts → `"This is **bold.**"` + `"And this is next."` |
| `"It was _quiet._ Then he left."` | `"It was _quiet."` · `"_ Then he left."` | R4 shifts → 2 sentences |
| `"Roses are red,  \nViolets are blue."` | `"Roses are red,  \n"` · `"Violets are blue."` | R2 exception (two-space) → 2 sentences |
| `"A line\\\nbroken hard."` | `"A line\\\n"` · `"broken hard."` | R2 exception (backslash) → 2 sentences |
| `"Acme Co. Then he quit."` | `"Acme Co. "` · `"Then he quit."` | class-2 `co` not in const → break stands → 2 |
| `"Acme Co. filed suit."` | 1 segment (UAX SB8) | 1 sentence (no rule needed) |
| `"I saw Mt. Fuji. It was tall."` | `"I saw Mt. "` · `"Fuji. "` · `"It was tall."` | R1 (`mt`) merges 1+2 → 2 |
| `"St. Louis is big. He knows."` | `"St. "` · `"Louis is big. "` · `"He knows."` | R1 (`st`) merges 1+2 → 2 |
| `"See cf. Smith 2001. He agreed."` | `"See cf. "` · `"Smith 2001. "` · `"He agreed."` | R1 (`cf`) → 2 |
| `"Smith et al. Wrote it."` | `"Smith et al. "` · `"Wrote it."` | R1 (`al`) → 1 |
| `"Kramer vs. Wade was long. He read it."` | `"Kramer vs. "` · `"Wade was long. "` · `"He read it."` | R1 (`vs`) → 2 |
| `"Q.E.D. Next problem."` | `"Q.E.D. "` · `"Next problem."` | no rule → 2 (correct) |
| `"One two.\n"` | 1 segment `"One two.\n"` | 1 sentence, span `(0, 8)` |
| `"中文。Then done."` | `"中文。"` · `"Then done."` | no merge → 2 (multibyte-safe offsets) |
| `"Nice 🙂. Then done."` | `"Nice 🙂. "` · `"Then done."` | 2 |
| `"été fini. Then done."` | `"été fini. "` · `"Then done."` | 2 |

## 5. DECISION — the abbreviation list: two classes, one const

**Why not just copy repar's list.** repar's ventilate-only stop-list
(`repar/src/transform.rs:72-75`, **25** entries) is unreachable (`mod sentence` is private —
`repar/src/lib.rs:62`; the list is deliberately quarantined from the frozen par oracle), so we
duplicate *something*. Copying it verbatim would buy coherence-by-construction with the
differential suite — but repar's flat always-stop semantics carry `no`, `co`, `eq`, `pp`, `ch` as
unconditional merges, and at least `no`/`co` **false-merge real boundaries** under those semantics.

**The governing insight (human, and it decides the fork):** repar's `ventilate` is a **one-shot
destructive transform** — a wrong break is visible, undoable, and repairable by a round trip. **The
lens (S6) is read continuously** — a wrong boundary there is a persistent lie about the writer's
own prose, and it corrupts the *diagnosis*, which is the entire value of the arc. **So our detector
is the authority, and it is optimized for correctness, not for agreement.**

**Why capitalization is not a sufficient rule.** Capital-after-period is a *weak* signal: English
capitalizes both sentence starts and proper nouns, and abbreviations are overwhelmingly followed by
proper nouns. `"Dr. Smith"` (no boundary), `"St. Louis"` (no boundary), `"Acme Co. Then"`
(boundary) — all identical in shape. `St.` is genuinely undecidable without grammar: *"I visited
**St. Louis** yesterday"* vs *"I live on Main **St. Louis** is nearby."* **Lowercase**-after-period,
by contrast, is a *strong* signal of no boundary (R3) — and UAX-29's SB8 already encodes it after
`.` intra-line.

### 5.1 Finalized membership

```rust
/// Class-1 abbreviations — ALWAYS merge across the following break (R1).
/// These are prefixes to what follows (a name, a place, a number, a citation
/// target); they are essentially never sentence-final. Matched case-
/// insensitively (ASCII) against the previous span's last token minus its
/// final `.`. Class 2 — suffix-like abbreviations that often ARE sentence-
/// final (`co`, `inc`, `ltd`, `etc`) — is DELIBERATELY ABSENT: for those, a
/// following lowercase word already merges via R3/UAX SB8, and a following
/// capital really is a boundary. `no` is deliberately absent (dropped): "the
/// answer was no." is far more common in prose than "No. 5".
const ABBREV_ALWAYS_MERGE: &[&str] = &[
    // titles
    "mr", "mrs", "ms", "dr", "prof", "rev", "gen", "sr", "jr",
    // name/place prefixes
    "st", "mt", "ft",
    // citation forms
    "fig", "vol", "ch", "pp", "eq", "vs", "cf", "al",
];
```

20 entries (9 titles + 3 name/place prefixes + 8 citation forms), one `const`, living beside the
R-rule helpers in `wordcartel-core/src/textobj.rs`.

| Class | Members | Mechanism |
|---|---|---|
| **Class 1 — always-merge** | the const above | R1 merges regardless of what follows |
| **Class 2 — break-on-capital, merge-on-lowercase** | `co` `inc` `ltd` `etc` | **NO code** — deliberate absence from the const. Lowercase continuations merge via UAX SB8/R3 (`"Acme Co. filed suit"` → already 1 segment, probed); a capital really is a boundary (`"Acme Co. Then he quit"` → break) |
| **Dropped** | `no` | repar's most damaging entry; absent, no special-casing |

Membership notes (curated against the §4.9 probe ledger and the §6 corpus, per the design's
"curated against the fixture corpus in the spec"):
- `ch` / `pp` / `eq` stay class 1 as citation forms: they prefix numbers (`"ch. 4"`, `"pp. 3-9"`,
  `"eq. 5"`), where UAX doesn't break anyway *(probed)*; the const entry only matters before a
  capitalized word (`"see ch. Four"`), where merging is right, and none of the three is an English
  word that ends sentences. Their "damaging" reputation attaches to repar's flat semantics, not to
  this class structure.
- `vs`, `cf`, `al` are spec-curated **additions** beyond the design table (its license: exact
  membership is decided here): each is a citation/comparison prefix that UAX splits before a proper
  noun *(probed: `"Kramer vs. Wade"`, `"cf. Smith"`, `"et al. Wrote"`)*, none can end an English
  sentence, and all three keep the differential suite's equality corpus green (repar's list has
  them too).
- `e.g` / `i.e` are NOT adopted: their last token contains interior dots (`"e.g."` strips to
  `"e.g"`), they are almost always followed by lowercase (R3/SB8 territory), and UAX does not split
  them intra-line *(probed)*. Residue noted in §10.
- **Coherence context** — repar's 25 entries for reference:
  `mr mrs ms dr prof sr jr st vs etc e.g i.e cf al no fig eq inc ltd co vol ch pp rev gen`
  (`repar/src/transform.rs:72-75`) plus its own single-capital-initial rule (`transform.rs:67-71`).
  Ours = repar's, minus `etc inc ltd co` (→ class 2), minus `no` (dropped), minus `e.g i.e`
  (SB8/R3 territory), plus `mt ft` (name prefixes repar lacks — divergence ledger L3, §6).

**NOT user-extensible in S5.** Nobody has hit a missing abbreviation; and the moment the list is
user-extensible it can drift from repar's, reintroducing the very incoherence S5 exists to remove.
When wanted, it is **config-file data, never a command** ("add an abbreviation" is inherently
parameterized; builtins are nullary — command-surface **law 10**). Revisit that together with a
possible additive `repar::Options::abbreviations(&[&str])` upstream, as **one** decision, later.

### 5.2 Prior art — `vim-textobj-sentence`, and why it validates rather than revises this list

`preservim/vim-textobj-sentence` is the widely-used regex sentence text-object for Vim
(`autoload/textobj/sentence.vim`). It is worth recording because, arriving from a completely
different engine — one composite Vim regex, **no** Unicode/UAX awareness — **it independently
converges on our exact four rules**, which is corroboration that R1–R4 is the right closed set, not
an arbitrary one:

| vim regex fragment | effect | our rule |
|---|---|---|
| `([-0-9]+<abbrevs>)@<n><!` negative-lookbehind before `\.` | a `.` after a number or a listed abbreviation is not a terminator | **R1** |
| body `\_.{-}` spans single `\n`; boundary only on `\n\s*\n` | a lone line-break is sentence-internal | **R2** |
| body must start `[[:upper:]]` | a boundary requires a following capital; lowercase continues | **R3** |
| terminator `…)+[<trailing>]*` | consume trailing quotes/brackets after the terminator | **R4** |

**Two decisions it independently confirms.** (1) vim keys the boundary on *a following capital* —
the same weak-signal/strong-signal split this section argues (R3). (2) vim's default abbreviation
set includes `[cn]o` and `[Ee]tc` as **unconditional** merges — i.e. it always-merges `no.`, `co.`,
`etc.` — which is precisely the false-merge §5 avoids by **dropping `no`** and demoting `co`/`etc`
to class 2 (R3 does the work). vim's list is the concrete instance of the over-inclusion we reject.

**Cross-check of the full default list against ours.** vim's default (`plugin/textobj/sentence.vim`)
is ~40 tokens, and once decoded it is dominated by **address/unit SUFFIX forms** — `Rd Ave Av Ln Ct
Pt Apt Str Blvd Assn Dept` and `hr min wt` — plus honorifics, `co/no/etc`, single initials, and a
grab-bag (`div esp est incl alt Op ep Cl pl`). Mapping it onto our two-class structure:

- **Address/unit suffixes** (`Rd Ave Ln Ct Blvd Apt Dept hr min …`) are the *same* ambiguity as the
  accepted `St.` residue (§10): they are suffix-position and genuinely *can* end a sentence ("I live
  on Elm Rd. It's quiet."). Putting them in an **always-merge** list — as vim does — makes that case
  merge wrongly; leaving them out, as we do, lets **R3** merge the common lowercase continuation
  ("Elm Rd. and I…") while a following capital breaks (a plausible over-break, never a nonsense
  fragment). So vim's largest category is exactly what class 1 should **exclude** — the cross-check
  *widens* the case for the small list, it does not shrink it.
- **`Ph.`** (for `Ph.D.`) is already handled by our single-capital-initial rule (the trailing `D.`),
  so it needs no entry.
- **The only genuinely prefix-type tokens vim carries that we lack** are the honorifics
  `Drs Messrs Univ Inst`. All four are rare in continuous prose, the single-capital rule + R3 cover
  their common uses, and adding them trades directly against the small-list thesis for negligible
  benefit — so they are **deliberately not adopted** in S5 (candidates for the later config-data
  revisit, above, not for the builtin const). Conversely we carry `rev gen` and the citation forms
  `fig vol ch pp eq cf al` that vim omits — the two lists have complementary gaps, and neither
  argues for growing ours now.

Net: no change to `ABBREV_ALWAYS_MERGE`. vim is prior-art *confirmation* of both the rule set and the
curation, and its `[cn]o`/`[Ee]tc` defaults are a live demonstration of the false-merge this list is
built to avoid.

## 6. DECISION — the differential suite: a divergence LEDGER, not an equality contract

**Full unification with repar is impossible** — `repar/src/sentence.rs:1-10` states that
`checkcapital`/`checkcurious` are shared by ventilate **and** by the reflow `guess_merge` path, and
the reflow path is **frozen by a byte-exact par-1.53.0 oracle**. So the suite *pins* the
relationship; it cannot merge the implementations.

### 6.1 Architecture — the suite lives in the SHELL

The suite needs both core's detector AND repar's ventilate output; core has **no repar dependency**
(and must not grow one). So it is an **integration test in the shell crate**:
`wordcartel/tests/sentence_differential.rs` (beside `module_budgets.rs` et al.), using:

- ours: `wordcartel_core::textobj::sentence_spans` (§3.5; `wordcartel-core` is a direct dependency
  of `wordcartel`, `wordcartel/Cargo.toml:13`, so integration tests can import it);
- repar's: the shell's own `run_transform` —
  `wordcartel::transform::run_transform(TransformKind::Ventilate, para, 72)`
  (`wordcartel/src/transform.rs:326`, `TransformKind` at `transform.rs:8`) — which drives
  `repar::Options::from_par_args([--width, "--ventilate", FIXUPS_STACK]).format(input)`
  (`FIXUPS_STACK` at `transform.rs:323`; `from_par_args` at `repar/src/options.rs:907`, `format` at
  `options.rs:961`). There is **no public repar `run_transform`** — the shell wrapper IS the
  product's ventilate, which is exactly what coherence should be measured against.
- Width note: ventilate is width-agnostic (`repar/src/driver.rs:209-210`: "Transforms:
  width-agnostic — NO feasibility check, NO reformat"; groups emit at natural length,
  `transform.rs:114-116`), so the `72` is inert; it is passed because `run_transform` requires one.

### 6.2 Extraction — assert word GROUPINGS

Not offsets (ventilate re-emits normalized lines with a re-emitted prefix,
`repar/src/transform.rs:80-93` — its output has no offsets into the source). Not counts (they can
be coincidentally right with the boundaries in the wrong places).

```rust
fn our_groups(para: &str, markers: &[&str]) -> Vec<Vec<String>> {
    wordcartel_core::textobj::sentence_spans(para)
        .map(|(f, t)| para[f..t].split_whitespace()
            .filter(|w| !markers.contains(w))
            .map(str::to_string).collect())
        .collect()
}
fn repar_groups(para: &str, markers: &[&str]) -> Vec<Vec<String>> {
    let out = run_transform(TransformKind::Ventilate, para, 72).expect("ventilate");
    out.lines().filter(|l| !l.split_whitespace().all(|w| markers.contains(&w)))
        .map(|l| l.split_whitespace()
            .filter(|w| !markers.contains(w))
            .map(str::to_string).collect())
        .collect()
}
```

`markers` is a per-fixture token filter (e.g. `&[">"]` for blockquotes, `&["-"]` for list items)
applied to BOTH sides — it subsumes prefix-stripping (ventilate re-emits `"> "` on every output
line; our source spans contain interior `">"` tokens at merged hard-wraps) and keeps the comparison
about prose words only. Agreement fixtures: `assert_eq!(our_groups(..), repar_groups(..))`.

### 6.3 Corpus

One fixture table, each row `(input, markers, expectation)`.

**What the equality corpus asserts.** Each equality fixture asserts that our detector and repar
ventilate produce **identical word groupings** — agreement that can arise EITHER by both **merging**
at the same points OR by both **breaking** at the same points. It is NOT "restricted to stop-list
abbreviations." The agreement comes from repar's non-break mechanisms **taken together**: the
ventilate stop-list (`transform.rs:72-75`:
`mr mrs ms dr prof sr jr st vs etc e.g i.e cf al no fig eq inc ltd co vol ch pp rev gen`), the
single-capital-initial rule (`transform.rs:66-71`), AND repar's shared UAX-level detector
`checkcapital`/`checkcurious` (`sentence.rs`) not seeing a boundary at interior-dot / non-boundary
forms — which is exactly why `U.S.A.` and `10 a.m.` stay one group and `Q.E.D.` breaks in BOTH.
**The entire equality corpus below was verified against live repar ventilate** (with the shell's
fixup stack, `--width=72`, 2026-07-12); each fixture's agreement is empirical, not assumed.

1. **The nasty abbreviation cases** (§4.9), with the agreement direction recorded per fixture —
   `merge` = both keep it one group across the abbreviation dot, `break` = both split at the real
   terminator:
   - `"Dr. Smith arrived. He was late."` → 2 groups (both **merge** at `Dr.` via stop-list, break at `arrived.`)
   - `"See fig. 2 for details. Then leave."` → 2 (both **merge** at `fig.`)
   - `"Kramer vs. Wade was long. He read it."` → 2 (both **merge** at `vs.`)
   - `"See cf. Smith 2001. He agreed."` → 2 (both **merge** at `cf.`)
   - `"Smith et al. Wrote it."` → 1 (both **merge** at `al.`)
   - `"St. Louis is big. He knows."` → 2 (both **merge** at `St.`)
   - `"J. R. R. Tolkien wrote it. He was English."` → 2 (both **merge** the single-capital initials via repar's initial rule / our R1)
   - `"Q.E.D. Next problem."` → 2 (both **break** after `Q.E.D.`: multi-char, not in the stop-list, not a single initial — `checkcapital` sees the capital `Next` as a boundary, our R1 declines to merge)
   - `"We met at 10 a.m. and left."` → 1 (both **non-break** at the interior `a.m.` dots — UAX/`checkcapital` see no boundary before lowercase `and`; there is no other terminator, so one group)
   - `"The U.S.A. is large. It grew."` → 2 (both **non-break** at the interior `U.S.A.` dots, both **break** at `large. It`)
   - **Explicitly NOT in the equality corpus** (ledger divergences instead): `mt`/`ft` name prefixes
     (→ L3, ours merges / repar breaks — repar's list lacks them) and the class-2/dropped-`no`
     cases (→ L4).
2. **The same content hard-wrapped** — the R2 proof, and the case that matters most: the §1 probe
   paragraph both unwrapped and hard-wrapped at the comfortable widths our own reflow emits; both
   forms must produce identical groupings, and identical to ventilate's (which unwraps before
   segmenting — so agreement here proves R2 reconstructs what ventilate sees).
3. **Blockquote (`> `) and list-item (`- `) variants** (prefix re-emission on repar's side,
   interior marker tokens on ours; the `markers` filter normalizes both).
4. **Multibyte**: `é` / `中` / `🙂` fixtures from §4.9 with ASCII terminators (byte-offset safety;
   CJK punctuation is ledger material, below).

### 6.4 The divergence LEDGER

**Known divergences are an explicit ledger**, each entry carrying a **reason** and asserted with
`assert_ne!` — so a divergence that *silently disappears* also fails the build, and the list cannot
rot.

> **L1 — the colon.** repar's default `terminal_chars` is `".?!:"`
> (`repar/src/options.rs:151-152`); UAX-29 does not break at `:`. `"Note: This is fine."` → repar
> ventilates to 2 lines; our detector says 1 sentence. **ACCEPTED as defensible**, not accidental:
> a colon genuinely *is* a structural break, and `ventilate` exists to expose structure. (It
> *could* be removed via `--terminal-chars=.?!` through the frozen `from_par_args` surface — no
> upstream change needed — but that would change a **shipped transform's output** for every
> existing user, to serve a detector they are not looking at when they run it.)
>
> **L2 — the ideographic full stop.** `"The word 中文。Then done."` — ours breaks at `。`
> (UAX + `TERMINATORS`); repar's terminal set is ASCII and par segments on whitespace-delimited
> words, so ventilate keeps one line. Ours is correct for CJK prose.
>
> **L3 — name prefixes `mt` / `ft`.** `"I saw Mt. Fuji. It was tall."` — ours R1-merges
> (`mt` in class 1); repar's frozen list lacks `mt`/`ft` and breaks before `"Fuji"`. The §5
> correctness-first stance, exercised.
>
> **L4 — class 2 and the dropped `no`.** `"Acme Co. Then he quit."` (also `inc`/`ltd`/`etc`
> fixtures, and `"The answer was no. Then we left."`) — ours breaks (capital after a class-2
> suffix / after `no.`); repar's flat stop-list always merges. The design's own indictment of
> those entries, pinned.
>
> **L5 — Markdown hard breaks.** `"Roses are red,  \nViolets are blue."` — ours yields 2 sentences
> (R2 exception, §4.5); ventilate emits ONE line *(probed with the shell's exact fixup stack: par
> tokenizes on whitespace, so the two-space marker cannot survive, and `"red,"` has no terminal
> char)*. Ours preserves authored structure; the transform cannot. (This corrects the design's
> "repar preserves hard breaks through transforms" claim — see the header corrections.)

Not a fuzz/property test in S5. A fixture table.

## 7. DECISION — sentence motions: `Dir` variants, Emacs semantics

### 7.1 Where — the `Dir` seam

Two new variants on the existing enum (`commands.rs:34-52`; derive at 34, `pub enum Dir` at 35),
adjacent to the word pair:

```rust
    WordLeft,
    WordRight,
    SentenceLeft,
    SentenceRight,
```

Two new arms in the `Move` inner `match dir` (`commands.rs:246-296`; the match is exhaustive, no
`_` — the compiler forces the arms, which is the registration seam; the surrounding `run` already
carries `#[allow(clippy::too_many_lines)]`, `commands.rs:237`):

```rust
        Dir::SentenceLeft  => nav::move_sentence_left(editor),
        Dir::SentenceRight => nav::move_sentence_right(editor),
```

Consequences, all free: **extend-selection composes** (the `Move` arm already builds
`Selection::range(anchor, new_head)` when `extend`), as do the ladder reset
(`sel_history.clear()`), fold normalization (`fold::normalize_caret`), `derive::rebuild`, and
`ensure_visible`. **Do NOT** add these as bespoke leaf commands outside `Dir` — that would
duplicate the extend/fold/jump-ring plumbing.

### 7.2 Semantics — Emacs `M-a`/`M-e` (start/end, NOT next/prev)

- `Alt+a` → the **start of the current sentence**; if already there, the start of the previous.
- `Alt+e` → the **end of the current sentence** (the *content* end, per §3); if already there, the
  end of the next.

The asymmetry is the point: `Alt+e` lands where you'd *continue writing*, `Alt+a` where you'd
*re-read*. It is idempotent-safe, and `Alt+a` then `Shift+Alt+e` is a natural sentence selection.
(Considered and rejected: symmetric next/prev-start, which would match the app's own word motions —
but it throws away the useful end-of-sentence landing, and A14 already established an Emacs-parity
thread in this product.)

The §3.5 helpers make each semantics ONE core call: `prev_sentence_start(win, rel)` (greatest start
strictly before `rel`) IS M-a within a window — mid-sentence it returns the current start; at the
start it returns the previous one. `next_sentence_end(win, rel)` (first content end strictly after
`rel`) IS M-e — mid-sentence it returns the current end; at the end (or in the gap) it returns the
next one.

### 7.3 The nav fns — mirror the word templates

`move_word_right`/`move_word_left` (`nav.rs:846-903`, doc-commented "crossing block boundaries
(skipping gaps)") are the templates; the sentence pair reuses their exact structure and helpers
(`head` `nav.rs:40`, `paragraph_range_at` `nav.rs:655`, `next_paragraph_start` `nav.rs:704`,
`prev_paragraph_start` `nav.rs:709`). New fns, in `nav.rs` beside the word pair:

```rust
/// Move to the start of the current sentence, or of the previous one when
/// already there (Emacs M-a), crossing block boundaries (skipping gaps).
pub fn move_sentence_left(editor: &mut Editor) -> usize {
    let h = head(editor);
    let new = {
        let buf = &editor.active().document.buffer;
        let blocks = editor.active().document.blocks();
        let (wstart, wend) = paragraph_range_at(blocks, buf, h);
        let window = buf.slice(wstart..wend);
        let rel = h.saturating_sub(wstart);
        match wordcartel_core::textobj::prev_sentence_start(&window, rel) {
            Some(r) => wstart + r,
            None if wstart > 0 => {
                let pps = prev_paragraph_start(blocks, buf, wstart);
                let prev_end = paragraph_range_at(blocks, buf, pps).1;
                let ptext = buf.slice(pps..prev_end);
                wordcartel_core::textobj::prev_sentence_start(&ptext, ptext.len())
                    .map(|r| pps + r)
                    .unwrap_or(pps)
            }
            None => 0,
        }
    };
    editor.active_mut().desired_col = None;
    new
}

/// Move to the end of the current sentence's content, or of the next one when
/// already there (Emacs M-e), crossing block boundaries (skipping gaps).
pub fn move_sentence_right(editor: &mut Editor) -> usize {
    let h = head(editor);
    let new = {
        let buf = &editor.active().document.buffer;
        let blocks = editor.active().document.blocks();
        let (wstart, wend) = paragraph_range_at(blocks, buf, h);
        let window = buf.slice(wstart..wend);
        let rel = h.saturating_sub(wstart);
        match wordcartel_core::textobj::next_sentence_end(&window, rel) {
            Some(r) => wstart + r,
            None => {
                // End of the next block's first sentence (skips gaps), else doc end.
                let nps = next_paragraph_start(blocks, buf, wend);
                if nps >= buf.len() {
                    buf.len()
                } else {
                    let next_end = paragraph_range_at(blocks, buf, nps).1;
                    let ntext = buf.slice(nps..next_end);
                    wordcartel_core::textobj::next_sentence_end(&ntext, 0)
                        .map(|r| nps + r)
                        .unwrap_or(nps)
                }
            }
        }
    };
    editor.active_mut().desired_col = None;
    new
}
```

(Exact structure of the word templates — same slice/rel arithmetic, same cross-block fallthrough,
same `desired_col = None`; the `left` fn body wording above is normative for behavior, and the
plan supplies the final line-for-line code. Block-crossing semantics: M-a from a block's first
sentence start lands on the previous block's LAST sentence start; M-e past a block's last content
end lands on the next block's FIRST sentence end. No third behavior is invented.)

### 7.4 Registry rows — four, mirroring the word rows verbatim

Word precedent at `registry.rs:185-191`. New rows, same shape (`register` at `registry.rs:111`),
ids locked by the design:

```rust
        // Sentence motions (S5, Emacs M-a/M-e) — palette-only (menu: None).
        r.register("sentence_left",  "Move Sentence Left",  None, |c| run(c, Command::Move { dir: Dir::SentenceLeft,  extend: false }));
        r.register("sentence_right", "Move Sentence Right", None, |c| run(c, Command::Move { dir: Dir::SentenceRight, extend: false }));

        // Sentence selecting motions (extend) — palette-only (menu: None).
        r.register("select_sentence_left",  "Select Sentence Left",  None, |c| run(c, Command::Move { dir: Dir::SentenceLeft,  extend: true }));
        r.register("select_sentence_right", "Select Sentence Right", None, |c| run(c, Command::Move { dir: Dir::SentenceRight, extend: true }));
```

`menu: None` → palette-only, matching the word motions' classification. (Titles mirror the word
rows' directional style; the Emacs start/end nuance lives in the doc comments and this spec, not
the label.) The existing `select_sentence` row (`registry.rs:330`) is untouched — it gains the
content-only span through `scope_range_at` automatically.

### 7.5 Keybindings

**CUA** (`static CUA: &[(&str, &str)]`, `keymap.rs:257`): `alt-a` and `alt-e` are **free**
(verified against the full bound-Alt set: `r left right o up down shift-up z shift-z shift-x b
shift-c shift-v , . u l c t shift-t shift-l j space shift-j \ s` — and `alt-shift-a` /
`alt-shift-e` are free too). `alt-left`/`alt-right` are spoken for by the jump ring
(`keymap.rs:326-327`). New rows:

```rust
    // Sentence motions (S5) — Emacs M-a / M-e semantics.
    ("alt-a",       "sentence_left"),
    ("alt-e",       "sentence_right"),
    ("alt-shift-a", "select_sentence_left"),
    ("alt-shift-e", "select_sentence_right"),
```

The shifted pair is required by the design's own composition claim ("`Alt+a` then `Shift+Alt+e` is
a natural sentence selection") and mirrors the word motions' shifted-extend pattern
(`ctrl-shift-left/right`, `keymap.rs:314-315`).

**WordStar** (`static WORDSTAR`, `keymap.rs:375`): **UNBOUND — no rows added.** WordStar has no
sentence idiom; law 7 means the four commands still appear in the palette without a hint. That is
contract-compliant, not a gap.

## 8. Command-surface contract conformance

Per-law, against `docs/design/command-surface-contract.md` and the live gates:

- **Law 3 (palette exhaustive):** the 4 new registry rows appear in the palette automatically;
  the invariant tests `palette_is_exhaustive_over_the_registry` (`palette.rs:255`) and
  `palette_is_exhaustive_over_a_plugin_loaded_registry` (`palette.rs:271`) gate it.
- **Law 7 (hints track the active keymap):** `Alt+a`/`Alt+e` (and the shifted pair) hint in CUA;
  no hint in WordStar; re-resolution on preset switch is gated by
  `hints_reresolve_on_preset_switch` (`keymap.rs:1087`) and
  `custom_bind_surfaces_in_menu_and_palette` (`menu.rs:435`). A new resolution test pins the four
  CUA chords and WordStar's deliberate unboundness (§12, T-12).
- **Law 4 (menu ⊆ palette):** N/A — all four rows are `menu: None` (palette-only), the word-motion
  precedent.
- **Law 2 (every option is a command):** N/A — motions add **no persisted option**;
  `every_persisted_setting_has_a_command` (`settings.rs:1003`) is unaffected. Note on
  `focus_granularity`: it is a **config-file-load-time option in `ViewConfig`** (field
  `config.rs:158`, enum `config.rs:94`, parse `config.rs:483-487`), **NOT a runtime
  `SettingsSnapshot` field** (`SettingsSnapshot`, `settings.rs:37-60`, has no such field), so it is
  outside the scope the Law-2 guard test covers and **Law 2 is N/A to it** — the same status as
  other config-load-only settings. The registry has `toggle_focus` (the focus on/off toggle,
  `registry.rs:544`) but **no set-focus-granularity command**; that is a pre-existing gap.
  **S5 adds no new persisted option and adds no focus-granularity command** — closing that gap is
  out of scope (S5 neither creates it nor is obligated to close it). S5 changes only what the
  EXISTING "sentence" granularity RESOLVES to (§9), not the option or its command surface.
- **Law 10 (builtins nullary):** all four commands are nullary ✓. (The abbreviation list is
  deliberately NOT a command — §5.)
- **No amendment to the contract is required.**

## 9. Behavior change to SURFACE, not hide

Focus mode with `focus_granularity = "sentence"` (field `config.rs:158`, enum
`FocusGranularity { Paragraph, Sentence }` at `config.rs:94`, parse at `config.rs:483-487`) today
focuses **a line** in reflowed text — the path already ships
(`gather_row_ctx`, `render.rs:488-512`, computes `focus_region` from `sentence_bounds`). After S5
it focuses **a sentence**. Desirable — but it is a **visible change to a shipped view**, not an
invisible bugfix. It must be called out in the effort notes and the release notes. Also visible:
`select_sentence` stops highlighting the trailing blank after the period (§3), and the expand
ladder gains a working Sentence rung in single-sentence paragraphs (§3.4.3).

## 10. Known residue — accepted

- `"I live on Main St. It's quiet."` merges into one sentence (`St.` as *Street*, ending a
  sentence, followed by a capital). **Benign by design:** the failure mode is a *plausible
  over-long sentence* the writer can see and dismiss — never a nonsense one-word fragment, and
  never a mutation (§2: no operator consumes a sentence span). This is exactly the ambiguity
  **S7's POS tagger dissolves**: `Louis` tags `PROPN` (continuing a noun phrase); `Then` tags `ADV`
  (starting a clause). The heuristic is not a permanent compromise — it is the placeholder for a
  principled rule that arrives two items later.
- **R3 does not see through leading markup** (§4.6): `"…!” *and so on*"`-shaped continuations
  whose next segment opens with markup/punctuation instead of a lowercase letter keep their UAX
  break. Markdown-aware continuation detection belongs to the lens work, not this module.
- **`e.g.` / `i.e.` are not in the const** (§5.1): interior-dot tokens, near-always followed by
  lowercase (SB8/R3 already merge). A capitalized continuation (`"e.g. Smith"`) keeps its break.
- Blockquote windows where a real terminator is followed by a `> `-prefixed lowercase continuation
  line break at the prefix (R3 sees `>`), diverging from ventilate's prefix-stripped analysis. The
  §6 corpus deliberately exercises blockquotes on the R2 path (hard-wrap mid-sentence), which
  agrees; the lowercase-after-terminator-behind-prefix combination is out-of-corpus residue of the
  same markup-blindness noted above.

## 11. Explicitly OUT of scope for S5

- User-extensible abbreviations (§5) — config data, later, together with the repar upstream
  question.
- `Option`-returning ("I don't know") objects (§3) — with the object that needs it.
- Any operator that *mutates* using a sentence span — that is **S4**.
- The lens (**S6**), the objects (**S4**), the POS substrate (**S7**), the lenses (**S8**).
- Fuzz/property testing of the detector — a fixture table suffices here.
- A sentence click-tier in `mouse.rs` (double/triple = word/paragraph today, `mouse.rs:591/595`) —
  not part of this item.

## 12. Test plan — every test, by crate and file

Core detector tests live in `wordcartel-core/src/textobj.rs` `#[cfg(test)]` (extending the
existing module at `textobj.rs:56-104`); shell tests live where their subjects live. Fixture
strings assert **slice text**, not hand-computed offsets, wherever a span is checked
(`&t[f..t] == "…"`) — robust and reviewable.

| # | Test | Crate / file | Pins |
|---|---|---|---|
| T-1 | `sentence_bounds_basic` — **the deliberate flip**: `(t, 2)` → `(0, 8)` (comment names S5 content-only); `(t, 12)` → `(9, 20)` unchanged | core / `textobj.rs:90-96` (rewritten in place) | §3.3 |
| T-2 | `empty_window_is_safe` extended: `sentence_bounds("", 0) == (0,0)` (unchanged, `textobj.rs:102`); plus `("\n", any) == (0,0)`; `prev_sentence_start("", 0) == None`; `next_sentence_end("", 0) == None` | core / `textobj.rs` | §3.2.4, §4.7 |
| T-3 | R1 unit tests: `Dr.`/single-capital `J. R. R.`/`Mt. Fuji`/`St. Louis`/`vs.`/`cf.`/`et al.` merges; `Q.E.D.` and class-2 `Acme Co. Then` and dropped-`no` breaks; case-insensitive `DR. SMITH` | core / `textobj.rs` | §4.4, §5.1 |
| T-4 | R2 + hard-break exception: the §1 hard-wrap paragraph merges to 2 sentences; `"  \n"` two-space and `"\\\n"` backslash fixtures stay 2 sentences; one-trailing-space control merges | core / `textobj.rs` | §4.5 |
| T-5 | R3 unit tests: `"“Why?” he asked."` → 1; `"He shouted “Stop!” and ran."` → 1; capital control breaks | core / `textobj.rs` | §4.6 |
| T-6 | R4 unit tests: `**bold.**` and `_quiet._` shift fixtures (span text includes the closers); end-of-text `"This is **bold.**"` → one span `(0, 17)` | core / `textobj.rs` | §4.3 |
| T-7 | UAX-preservation regression: the §4 "free wins" table (`10 a.m.`, `U.S.A.`, `fig. 2`, `“Go home.”`-boundary-after-quote) still segment correctly through the post-pass | core / `textobj.rs` | §4 |
| T-8 | Attach rule: gap caret → preceding (`(t, 8)` → `(0,8)`); `pos == len` → last; block-start (window opening with a whitespace-only hard-break line) → following; multibyte carets (`é`/`中`/`🙂` fixtures) | core / `textobj.rs` | §3.2 |
| T-9 | Expand-ladder regression: `"One two.\n"`, caret in "One": Word → Sentence slice `"One two."` `(0,8)` → next expand strictly larger — the rung no longer collapses | shell / `commands.rs` `#[cfg(test)]` beside `expand_then_shrink_round_trips` (`commands.rs:1134`) | §3.4.3 |
| T-10 | Motion semantics: M-a start-of-current / repeat-to-previous; M-e end-of-current-content / repeat-to-next; cross-block both directions (prev block's LAST start; next block's FIRST end); `extend: true` composes a selection (`select_sentence_right` grows from anchor) | shell / `commands.rs` `#[cfg(test)]` beside the motion tests (`doc_start_and_end`, `commands.rs:1101`) | §7.2-7.3 |
| T-11 | `sentence_left`/`sentence_right`/`select_sentence_left`/`select_sentence_right` dispatch through the registry (rows exist, handlers run) — plus the palette-completeness gates (`palette.rs:255/271`) pass with the new rows | shell / `registry.rs`+`palette.rs` existing gates + one dispatch assert | §7.4, §8 |
| T-12 | Chord resolution: CUA resolves `alt-a`/`alt-e`/`alt-shift-a`/`alt-shift-e` to the four ids; WordStar resolves none of them to a command (deliberately unbound, mirroring `close_buffer_is_unbound_in_both_presets_by_design`, `keymap.rs:1056`); hint re-resolution gates (`keymap.rs:1087`, `menu.rs:435`) stay green | shell / `keymap.rs` `#[cfg(test)]` | §7.5, §8 |
| T-13 | Differential suite: equality corpus (§6.3) + ledger L1–L5 (`assert_ne!` each, with its reason in the assert message) | shell / `wordcartel/tests/sentence_differential.rs` (new) | §6 |
| T-14 | Focus-mode behavior change: e2e journey — hard-wrapped two-sentence paragraph, `view_opts.focus = true`, `focus_granularity = Sentence`; BOTH visual rows of the wrapped first sentence render undimmed and the second sentence's row dims (asserted via a `Modifier::DIM` row probe on the `TestBackend` buffer, the `underlined_cols` pattern at `e2e.rs:258-262`) | shell / `wordcartel/src/e2e.rs` | §9 |

Gates unchanged and in force: `cargo test` all suites, warning-free build, workspace clippy clean
(`too_many_lines` at 100 — §4.2's helper-fn structure exists for this), module budgets, PTY smoke
suite mandatory-run/advisory-pass with its one-line summary quoted in the pre-merge report.

---

## Pipeline status

**Brainstorm: COMPLETE and approved (2026-07-12).**
**Spec: AUTHORED (this document, 2026-07-12) — entering the Codex spec gate (loop to clean).**

**Not yet done:** Codex spec gate → plan → Codex plan gate → branch → subagent-driven TDD
execution → the two final gates (Fable whole-branch + Codex pre-merge) → `--no-ff` merge.
