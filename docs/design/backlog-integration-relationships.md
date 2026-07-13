# Backlog integration relationships — spines, cross-pollination, and sequencing

**Status:** ANALYSIS (2026-07-13). Whole-backlog relationship map + triage proposal. Nothing here
changes item state; every proposed edge/fold/re-scope is a PROPOSAL for the human. Grounded against
`scripts/backlog open|shipped`, the arc/design-space docs, and the real code at HEAD (symbols
verified: `ProviderSet`/`DiagSource`/`active_lens_diags`, `View { mode, ventilate }`,
`view_opts: ViewConfig`, `ventilate::fill_visible`, `Diagnostic { source, code, href }`,
`wc.timer`/parameterized plugin commands).

**Corrections to the record found while grounding (read these first — they change the map):**

1. **The linter-arc spine (effort "a") is SHIPPED but UNFILED.** The multi-provider diagnostics
   SPINE merged (`a2f9062`): `Diagnostic` carries `source`/`code`/`href`, `ProviderSet` replaces the
   single `Box<dyn DiagnosticsProvider>`, the store is source-partitioned (`slot(source)`), and the
   switchable analysis lens (`active_analysis_source` + `analysis_next` + per-engine enables) is
   live. `DiagSource::LTeX`/`Vale` are **reserved vocabulary only** — no provider exists; the
   overlay ignores `href`/`code` (grep: no "learn more"/detail consumer). **None of the remaining
   linter-arc efforts (b) ltex/vale providers, (c) viewing/action delta, (d) `wc.async`,
   (e) plugin-declared servers is a backlog item.** They are invisible to `scripts/backlog open`.
2. **S6 did NOT ship as "a fifth RenderMode."** The archive prose (and the arc doc) say "RenderMode
   already cycles four states — this is a fifth." The code shipped `View.ventilate: bool` —
   **per-buffer, orthogonal to `RenderMode`, composing with it** (`fill_visible` renders prose rows
   per the active `mode`). So the shipped answer to E8's open question "is a layout lens a fifth
   RenderMode?" is **no — a per-buffer boolean that stacks with the mode cycle**. E8's design must
   start from that fact, not the archive's stale sentence.
3. **S6's rhythm gutter shipped word-count-only.** `GUTTER_COLS = 6` (`NNN │ `); the opening-word
   column and repeated-opener highlighting from the pitch ("four of these six open with 'The'")
   did **not** ship. That remainder is itself a *style lens* under E8's own taxonomy (it paints
   spans, it stacks) — see §4.3.
4. **PA/PB/PC `depends_on ["P"]` is satisfied** — Effort P shipped 2026-07-12. The graph no longer
   blocks them; product judgment does (and should — see §4.1).

---

## 1. The SPINES — cross-cutting mechanisms multiple features share

### SP-1. Sentence authority (S5 — SHIPPED)
**What:** wordcartel's own sentence detector (`textobj::sentence_spans`, UAX-29 + the 4-rule
abbreviation post-pass + hard-break veto), pinned to repar's ventilate by the differential corpus.
The D2 law: our detector owns everything the user **sees and selects**; repar stays the destructive
transform; the two are pinned by tests, never merged.
**Stands on it:** S6 (shipped — the lens segments with it, SEE==SELECT by construction), **S4**
(`select_sentence`, sentence motions, `transpose_sentences`), **S7** (harper-brill's
`tag_sentence` takes *a sentence* — OUR spans decide what a sentence is, so S5's correctness
propagates directly into POS quality; a mis-segmented span is mistagged wholesale), **S8** (lenses
attribute findings per sentence), the S6 gutter, any future readability metric (PA).
**Health:** solid; the one open wart it surfaced is **B10** (EOF caret clamp in shared
`nav::caret_line`).

### SP-2. The lens surface (E8 — OPEN, needs-design; the axis model)
**What:** the unifying "how do I see my prose" surface. The two-axis model — **style lenses paint
and STACK; layout lenses re-draw and are EXCLUSIVE** — is validated by shipped code
(`toggle_focus` stacks with diagnostics; ventilate re-breaks rows). The code now holds **five**
toggle shapes across **three different state homes**:

| Surface | Shape | Scope (state home) |
|---|---|---|
| `RenderMode` (also the diagnostics gate) | exclusive 4-cycle | **per-buffer** (`View.mode`) |
| `View.ventilate` (S6 layout lens) | boolean | **per-buffer** (deliberate — "a lens is into THIS writing," S6 §F5) |
| `active_analysis_source` | exclusive selector | **global** (`Editor`) |
| `toggle_focus` + granularity | boolean (stacks) | **global** (`view_opts`) |
| `toggle_typewriter` / `toggle_measure` | booleans | **global** (`view_opts`) |

**The grounding adds a THIRD axis the E8 prose doesn't yet name: SCOPE (per-buffer vs global).**
S6 chose per-buffer; the analysis lens chose global; focus chose global. That inconsistency is
already shipped product feel, and **E9 ("diagnostics lens: per-buffer vs global") is exactly this
axis asked about one lens** — E9 is a sub-question of E8, not a standalone item (§4.2, §5).
**Stands on it:** S8 (style-lens kind), E9, plugin-registerable lenses (Effort-P), the unshipped
repeated-opener highlight, PA's "readability lens", arguably `toggle_focus`'s future.

### SP-3. The diagnostics/provider seam (linter arc — spine SHIPPED)
**What:** `DiagnosticsProvider` (async, whole-document, process-lifecycled) + `ProviderSet` +
source-partitioned `DiagStore` + per-source staleness + the engine-agnostic LSP transport
(`lsp_rpc.rs`). Open–Closed for provider #2: adding ltex/vale is "clone the `harper_ls.rs`
lifecycle template."
**Stands on it:** the unfiled (b) ltex/vale providers and (c) viewing/action delta
(`href`/"learn more"/detail region, per-engine dictionary writers, `executeCommand` relay — the
`Diagnostic` fields already exist and are unconsumed); plugin-declared engines
(`DiagSource::Plugin` reserved); E9's scope question. **Explicit non-consumer: S8** — caret-local,
synchronous, processless; it must NOT implement this trait (verified: `notify_change` takes the
whole document as `String`).

### SP-4. The POS substrate (S7 — OPEN; the expensive bet's foundation)
**What:** harper-brill in-process (measured: +119 activated crates, +0.95 MB, zero FFI/GPU) —
`tag_sentence` + `chunk_sentence` over the caret's block window, cold-path, cached by
`(block_span, document.version)`.
**Stands on it:** S8 entirely (X-ray lenses, Phrase/Clause select-only objects), D5's principled
clause splitting, and — per the arc doc — "eventually a native stylistic-diagnostic provider
alongside harper-ls." **⚠ That last clause is in latent tension with E8's finding** (§3, R5).
**Gates:** the S6 two-week kill-gate (premise), and `cargo deny`/`cargo audit` over the 119 crates
(supply chain — matters more post-Effort-P).

### SP-5. The command-surface contract (governing law — SHIPPED, amended as needed)
**What:** registry = truth; every option a command; palette exhaustive; set-per-state + a stateful
menu representative; dynamic menu sections (data rows over shared setters).
**Every lens toggle, engine enable, and metrics view lands here.** E8's hardest contract question
is already visible: N stackable style lenses = N toggles by rule 8 — what does the View menu carry?
(A lens submenu? a dynamic section like Documents? presets?) The **dynamic-section seam** is the
plausible answer and is *also* what the linter arc's per-engine menu section wants — one design
serves both (§3, R8-adjacent).

### SP-6. The plugin/async substrate (P1–P3 — SHIPPED; `wc.async` + plugin menu rows DEFERRED)
**What:** in-process Lua, main-thread pump, `!Send` confinement; commands (parameterized, with
minibuffer arg-collection), events, config/reload, guarded timers. Deferred with named drivers:
**`wc.async`** (closed Rust op-menu; canonical driver = vale-CLI or a formatter) and
**plugin-contributed dynamic menu sections**.
**Stands on it:** PA/PB/PC, S2 (the binder is the anointed first real plugin), linter (d)/(e),
A17's plugin emit API, E8's plugin-registerable lenses. **Constraint E8 inherits:** a plugin lens
callback is `!Send` Lua and must never run per-frame — plugin lenses must be *precomputed span
sets*, edge-triggered off content changes, cached like everything else (§3, R7).

### SP-7. Quantitative prose metrics (latent — no single owner yet)
**What:** per-sentence/region word counts and style statistics. Today: `count.rs` (core),
the S6 gutter (per-sentence counts, live), S4's proposed `count_region`, PA's proposed
readability/goals plugins, the unshipped repeated-opener stat. Five callers converging on "compute
stats over sentence spans in a window" with no shared helper. Cheap to converge now, expensive to
unify later (§4.3).

---

## 2. The RELATIONSHIP MAP (open-item pairs/clusters that matter)

**S8 ↔ E8 (the tightest coupling in the backlog — bidirectional).**
S8's POS X-rays are style-lens *instances*; E8 is the surface they toggle through. The manifest
says `E8 depends_on [S6, S8]` (generalize after instances — the rule-of-three stance, and S6 shipped
without E8, consistent with it). But the influence runs backwards too: if S8 ships its lenses as
yet another bespoke shape (a 6th toggle surface, a 4th state home), E8 inherits a migration instead
of a generalization. **Resolution (proposal): split E8 into model and mechanism.** Decide E8's
*axis model* (style×layout×scope, attribution, naming) as a short design exercise BEFORE S8's
spec; implement E8's *surface* (registry of lenses, menu/palette shape, plugin registration) after
S8 exists as its second in-tree instance. The depends_on edge stays honest for the mechanism; the
model informs S8's spec. (§6 Phase 1.)

**E9 ⊂ E8 (fold).** "Per-buffer vs global" is not a diagnostics question — it is the scope axis of
every lens, and shipped code already answers it two different ways (ventilate per-buffer, analysis
source + focus global). Deciding E9 standalone bakes a one-lens answer E8 must then either
generalize or contradict. Propose: E9 becomes a named section of E8's brainstorm (or gains
`depends_on [E8]` if kept as a tracking item).

**S7 ↔ S5 (shipped, but a live design input).** harper-brill tags *sentences*; our detector
supplies them. S7's spec should state this explicitly: the tagger's input spans are
`textobj::sentence_spans` output (content-trimmed — remember the trailing-whitespace trap), so the
differential corpus indirectly protects POS quality too. Also gives S7 a free test lever: the S5
fixture corpus doubles as the tagger-input corpus.

**S7 ↔ the linter arc (independent engines, one honest caveat, one latent conflict).**
Independence is real and verified (no LSP method returns a parse; different process models/latency
budgets). Two edges remain: (i) once S7 lands, `burn` is in the binary and H2's dep-weight defense
of the subprocess split is partially spent — re-measure before citing it again (already recorded);
(ii) the arc doc's "eventually a native stylistic-diagnostic provider alongside harper-ls" would
put S8-class findings INTO the diagnostics seam — see R5.

**S4 ↔ S6 (shipped) — surgery for the diagnosis.** S4 is now *earned*: the lens shows the 41-word
monster; S4's `select_sentence`/`transpose_sentences` act on it. Two hard constraints S4 inherits
from S6's implementation: (a) **SEE==SELECT** — S4's sentence object must use the identical
`paragraph_range_at` window + detector call the lens renders with (the lens code was deliberately
built to mirror `Scope::Sentence`; S4 must not drift); (b) the S6 pitch's second half ("move them
around like cards") is unshipped until S4 lands — which bears on the kill-gate question (Q2, §7).

**S4 ↔ S1.** `S1 depends_on [S4]` is correct and specific: S1 inherits `select_section` and OWNS
section transpose (because `outline::sections` ranges nest — swap-with-next corrupts). The
cross-pollination: S4's `select_section` spec should define section identity with S1's *sibling*
problem in view (same-or-higher-level boundary, separator normalization), so S1 extends rather than
re-derives it.

**S4 ↔ B10 / hazard-4.** S4 works in exactly the code B10 lives in (`nav::caret_line` clamp) and
its expand-ladder workflow is broken by the shipped hazard-4 bug (`sel_history` cleared on every
edit/undo — the structural workflow is expand→operate→undo→re-expand). Propose: fold both fixes
into S4's scope rather than leaving them as ambient debt S4 trips over.

**S3 ↔ S1/S4 (the trust multiplier).** Snapshots are "fearless editing" insurance, and the arc's
own mutation law (D6) exists because structural mutation is scary. A writer will try
`transpose_sentences` and S1's subtree moves far more readily with one-keystroke restore behind
them. S3 is independent (pure durability-spine surfacing), needs no design input from the arc, and
its per-heading-subtree granularity fork composes with S1 later. High product leverage per unit
risk; sequence it before or alongside the mutation-heavy items.

**S2 ↔ S1 ↔ PB/PA.** Unchanged from triage: S2 after S1, as the anointed first real plugin
(binder = manifest of files, reusing S1's rearrange UI). PA's backlinks/wiki-links composes with
S2's directory model; PB's wiki-link rendering is the visual half of the same plugin. These stay
watch-tier until the writing-unit question (Q6) is answered.

**A17 ↔ linter (b) ↔ the plugin era.** A17 (routed messaging) is the seam that ltex's
`Starting`/"warming ltex… 30s–2min" UX, provider `Degraded` events, plugin `wc.status` spam
containment, and lens attribution notices all want to emit into. Every effort that ships before
A17 adds more ad-hoc status-line pokes that A17 must later route. A17 is explicitly flagged
"decide with P or just ahead of it" — P has now shipped, so the window is *now-ish*. Full A17 is a
real effort; the cheap hedge is to design the **`Message` vocabulary** (kind/severity/lifetime/
source enum + the emit signature) before linter (b), so (b)'s warming UX lands on the right type
even if routing/history come later.

**`wc.async` ↔ linter (d) ↔ PA/PB.** Fully independent of everything (P3 deferred it with its own
driver). Its natural trigger is the first real one-shot-tool want: vale-CLI-as-plugin, a formatter,
or PA's CMS publish. Don't schedule it on its own; schedule it when its driver arrives.

**H10/H13 (watch) ↔ E8/A17/P.** H10's trigger ("collapse the intercept chain when plugin/overlay
stages grow it") and H13's ("peel Editor field clusters when a refactor wants a cleaner
plugin-facing state surface") are both plausibly pulled by E8 (lens state may want a home — a
`LensState` cluster) and A17 (a message queue on Editor). Not blockers; note them in those specs so
the triggers fire deliberately.

**Independent singletons (no meaningful edges, schedule freely):** B6 (heading-glyph style — pure
command-surface template work), B7, B8, B9, A15, A16, H19, H20 (do early — a flaky test undermines
the merge GATE every effort relies on), M9, H3 (watch, correctly).

---

## 3. "Designed-in-isolation" RISKS (the findings the human asked for)

- **R1 — S8 specced without E8's axis model.** S8 would default to copying the nearest precedent —
  either the analysis-lens selector (wrong: that's exclusive; X-rays should stack) or bespoke
  booleans in a 4th state home. Result: a 6th toggle shape and E8 becomes a migration. Mitigation:
  the model/mechanism split above; S8's spec states its lenses' axis (style, stacking), scope
  (per-buffer vs global — deliberately), and attribution story in E8's vocabulary.
- **R2 — E9 answered for diagnostics alone.** Whatever scope the diagnostics lens picks becomes the
  de-facto precedent the other lenses didn't vote on — and shipped code is already split two-vs-two
  across state homes. Fold E9 into E8.
- **R3 — linter (b) shipping ltex's warming/degraded UX as status-line pokes.** Works, but adds the
  exact ad-hoc messaging A17 exists to end, on the surface (long-running lifecycle events) that
  most needs routing/history. Mitigation: A17 `Message` vocabulary first (cheap), or an explicit
  decision to accept the debt.
- **R4 — PA's "readability lens" built as a diagnostics-catalog plugin while S8 builds the same
  thing natively.** Long sentences = the S6 gutter; adverbs/passives = S8's marquee lenses. Two
  Hemingway surfaces with different toggles, different attribution, different scope. Mitigation:
  re-scope PA-readability to "S8 lens presets + (later) a plugin-registered lens proof-case"
  (§4.1).
- **R5 — S7's API baked for one consumer.** The arc wants S7 caret-local/block-windowed (for S4/S8);
  the arc ALSO floats a future "native stylistic-diagnostic provider" — which would be whole-document
  and land in the diagnostics seam. If S7's cache/API assumes "block window only," the provider idea
  forces a rework; if it generalizes prematurely, it violates YAGNI. Mitigation: S7's spec takes
  sentence-spans-in, tags-out as the pure core (window-agnostic), keeps the *cache* block-windowed,
  and explicitly DEFERS the provider idea — noting it would enter via the shipped `ProviderSet` as
  an in-process provider impl, not by bending S8's lens.
- **R6 — S4's `select_section` without S1's sibling model.** Cheap to prevent (one spec paragraph);
  expensive to re-derive (S1 would fork section identity).
- **R7 — E8's "plugins register a lens" designed without the P pump discipline.** A naive design
  invokes Lua at paint time — per-frame, hot-path, `!Send`. The P3 grounding's lesson (menu rows =
  data computed at build time; callbacks fire through the capped pump) transfers: a plugin lens must
  submit *span data* (edge-triggered, version-stamped, capped), never a paint callback. E8's spec
  must state this; it also reuses the deferred boxed-closure work the plugin-dynamic-menu effort
  needs — co-schedule them.
- **R8 — the per-engine Analysis menu section and E8's lens menu designed separately.** Both want a
  dynamic `DYNAMIC_SECTIONS` category showing live state (Ready/warming/off; active/stacked). One
  design, two consumers; whichever effort goes first should build the section shape the other can
  reuse.

## 4. REDUNDANCIES / overlaps to collapse or reconcile

1. **PA-readability vs S6+S8 (collapse).** Hemingway's feature set = sentence length (S6 gutter,
   shipped) + adverbs/passives (S8, native). Keep PA's *goals/streaks/CMS/backlinks*; strike or
   re-scope "readability lens" to a pointer at S8 — or keep exactly one slice ("a plugin-registered
   lens") as the E8 plugin-API proof-case.
2. **One metrics helper (SP-7).** S6's gutter counts, S4's `count_region`, PA goals' word counts,
   and the unshipped opener stat should share one core "sentence/region stats over a window"
   helper (extending `count.rs` over `sentence_spans`). Propose S4's spec name this explicitly.
3. **E9 into E8** (as above).
4. **`toggle_focus` (and arguably typewriter/measure) under the lens model.** Focus is the shipped
   proof that style lenses stack; leaving it a bespoke boolean while building the lens surface is
   incoherent. E8 should at minimum adopt it nominally (it IS a style lens) even if the code moves
   later. Typewriter/measure are viewport behaviors, not lenses — E8 should say so and leave them.
5. **The unshipped repeated-opener highlight.** Don't file it as an S6 follow-up in isolation — it
   is the cheapest possible *style lens* (zero NLP, data already computed by the gutter pass) and
   therefore the ideal E8 first instance / test article before S8's POS lenses exist.
6. **Ventilate-command vs ventilate-lens naming** (arc open Q2): both live in the product now —
   `ventilate`/`ventilate_buffer` (Format, destructive) and `toggle_ventilate` (View, lens). The
   contract is satisfied, but the naming/menu story ("Ventilate" appears in two menus with opposite
   safety profiles) is an E8-brainstorm item, not two separate polish items.
7. **Unfiled linter-arc work.** File (b), (c), (d), (e) as items (theme E or a new letter) so the
   dashboard reflects reality; today the arc's remaining ~3 efforts are invisible.

## 5. Dependency-graph REFINEMENTS (proposed, not applied)

Current open-item edges: `S1←S4`, `S2←[P,S1]`, `S4←S5✓`, `S7←S6✓`, `S8←S7`, `E8←[S6✓,S8]`,
`PA/PB/PC←P✓`.

- **Keep `E8 depends_on [S6, S8]`** (mechanism-after-instances) but record the model/mechanism
  split in E8's prose — the *model* is an input to S8's spec, which no edge can express.
- **E9: add `depends_on [E8]`** (or fold outright). Today it has no edges and looks independently
  schedulable — misleading.
- **`S7 depends_on [S6]` under-states the gate.** The real dependency is the S6 *two-week real-use
  verdict*, not the merge (which landed today). Propose annotating the hook: "gated on the S6
  kill-gate verdict (~2026-07-27), not the merge."
- **PA: add a relationship note (or edge) to S8/E8** for the readability slice (R4). PB: its
  markup-extension API need is a P-follow-on effort that doesn't exist as an item — same
  filing gap as the linter arc.
- **File linter (b) and (c) with an edge to the shipped spine** (satisfied), **(d) `wc.async`
  edge-free** (independent), **(e) ← (d) + plugin-dynamic-menu**.
- **A17: no hard edges, but two soft "informs" notes** — linter (b) (warming UX) and any further
  plugin-ecosystem growth. Consider a `blocks`-style note rather than a false hard edge.
- **B10: note S4 adjacency** (candidate fold-in).
- No backwards edges found: `S1←S4` (not the reverse) is right given select_section's home;
  `E8←S6` was already corrected and matches how S6 actually shipped.

## 6. TRIAGE RECOMMENDATION (sequenced, grouped, reasoned)

**Phase 0 — now, during the S6 kill-gate window (cheap, de-risked, high leverage):**
1. **Run the trial.** Daily real-prose use of the ventilate lens is the highest-information
   activity available and costs zero engineering. Everything expensive hangs on it.
2. **Bookkeeping (hours):** file linter (b)/(c)/(d)/(e); fold E9 into E8; annotate S7's gate;
   re-scope PA-readability (pending Q4).
3. **H20** (flaky test — it erodes the one merge GATE every future effort uses), plus any of the
   independent singletons (B7, A15, A16, B9, B6) as filler between design sessions.
4. **S3 snapshots** — independent of every fork above, surfaces the existing durability spine,
   and multiplies trust in the arc's upcoming mutation features. Good Phase-0 flagship.
5. **S4** — IF Q2 resolves to "the trial should test the full diagnose+operate thesis" (my lean:
   yes — see Q2). Zero NLP, fully earned, blocked on nothing (S5 shipped). Fold B10 + the
   hazard-4 `sel_history` fix; spec `select_section` with S1's sibling model; name the shared
   metrics helper (SP-7).

**Phase 1 — at the S6 verdict (~2 weeks):**
- **Verdict positive:** run the **E8 model design** (axis model: style×layout×scope; attribution;
  focus adoption; menu shape incl. the shared dynamic-section design of R8; plugin-lens data-not-
  callback rule) as a short brainstorm/spec — then **S7** (with the `cargo deny` gate and the R5
  API posture), then **S8 specced in E8's vocabulary**, then **E8 mechanism** (with S8 + the
  repeated-opener lens as its two style instances, plus the shipped selector as the diagnostics
  family). This order honors both `E8←S8` (mechanism after instances) and R1 (model before S8).
- **Verdict negative: STOP the arc** (S7, S8 die; the +119-crate bet is never placed). E8 shrinks
  to "reconcile the existing five toggles + the scope axis (E9)" — still worth doing, much smaller.
  Capacity shifts to the linter arc and Theme-S structure items (S1/S3 path), which are
  arc-independent.

**Phase 2 — demand-driven, either branch:**
- **Linter (b) ltex/vale + (c) action delta** — independent of the arc; schedule on real user want
  (a second engine, "learn more" links). Precede with the **A17 `Message` vocabulary** decision
  (R3), even if full A17 routing/history comes later as its own effort.
- **A17 full design** — before the plugin ecosystem grows past a couple of plugins; it defines the
  plugin-era interaction texture and the user flagged it as wanting deep thought.
- **S1 → S2** — after S4; S2 stays the first-real-plugin driver, pending Q6.
- **`wc.async`** — when its driver materializes (vale-CLI plugin, formatter, or PA CMS publish).
- **PB CriticMarkup** — the anchor for the markup-extension API design; its own effort, post-E8
  (the paint layer will be better understood once lenses exist).

**Cheap + de-risked + high-leverage:** S3, S4, H20, the bookkeeping, the E8 *model* exercise, the
A17 vocabulary. **Expensive bets, gated:** S7+S8 (S6 verdict + cargo-deny), full A17, linter (b)
(JVM lifecycle), S1/S2, PB markup API, E8 *mechanism* (plugin registration).

## 7. OPEN QUESTIONS for the human

- **Q1 — E8 sequencing paradox.** Accept the model/mechanism split (design E8's axis model now, on
  paper, to govern S8's spec; build E8's surface after S8 as `E8←S8` says)? The alternative —
  letting S8 pick its own shape and generalizing later — is the recorded edge's literal reading,
  and it re-runs the exact failure the E8 reground documents (the diagnostics effort merging before
  E8's constraint reached it).
- **Q2 — does S4 run during the S6 trial, or after the verdict?** For a *clean* falsification the
  trial should test the lens alone (if the pure lens doesn't earn its keep, the premise dies
  cheaply). But the S6 pitch explicitly includes "then move them around like cards" — without S4
  the trial tests half the thesis, and a lukewarm verdict would be ambiguous. My lean: build S4
  during the window (it's cheap and NLP-free either way) but note which capabilities arrived when,
  so the verdict can distinguish "lens alone" from "lens + surgery."
- **Q3 — the lens SCOPE axis.** Is a lens per-buffer (S6's deliberate choice — "a lens is into THIS
  writing") or global (the analysis selector and focus today)? This is product feel, it is already
  inconsistent in shipped code, and it decides E9. One coherent answer: layout + style lenses
  per-buffer; viewport behaviors (typewriter/measure) global; then the analysis lens's global-ness
  is a (fixable or acceptable) anomaly.
- **Q4 — PA-readability's fate.** (a) strike it (S6+S8 subsume); (b) keep one slice as the E8
  plugin-lens proof-case; (c) keep as-is (two Hemingway surfaces — not recommended).
- **Q5 — A17 timing.** Vocabulary-only now + full effort later, vs full design before linter (b),
  vs accept status-line pokes for (b) and design A17 whole afterwards?
- **Q6 — writing unit (carried, still undecided).** Single long document (S1 is the whole answer)
  vs book-as-directory (S2 on top of S1). Gates S1-vs-S2 priority and PB/PA's backlinks slice.
- **Q7 — does `toggle_focus` migrate into the lens model** (nominally now, code later), or stay a
  bespoke boolean outside E8? (Recommend: adopt nominally — it is the shipped proof that style
  lenses stack.)
