<!-- GENERATED from backlog.toml — do not edit by hand. -->
<!-- Regenerate: BLESS_BACKLOG=1 cargo test -p wordcartel --test backlog -->

# Backlog

**40 open · 72 shipped · 2 dropped**

Blocking Effort P: **0**

## Open

| id | title | status | kind | size | P? | hook |
|---|---|---|---|---|---|---|
| B12 | Lone block-begin marker renders nothing (^KB before ^KK is invisible) | needs-design | bug | S |  | Found in the cursor-system review. Effort-9A shipped the marked-block system WITHOUT rendering a LONE begin marker: after ^KB (block-begin) with no ^KK (block-end) yet, there is NO visual indication a start is set — only a complete begin+end region paints (SE::MarkedBlock). The writer can't see they set a mark. Fix: render the pending_block_begin position. Superseded by B13 (styled boundaries) which does this properly for all markers; filed separately as the acute defect. ~S. |
| B13 | Block markers — styled boundary cells (modern B-lite; no injected bracket glyphs) | needs-design | feature | S |  | C2 of the cursor-system review (docs/design/cursor-system-concept-review.md). Render marked-block BEGIN/END (and the lone-begin, closing B12) as STYLED BOUNDARY CELLS — a reverse/underline/theme treatment on the boundary column — NOT injected bracket glyphs. Decision 2026-07-13: the WordStar [ ]-bracket model was predicated on hardware-terminal capabilities; styled boundary cells are the modern, cheaper choice (~15% of the B-full injected-marker cost) with the IDENTICAL data model. Explicitly NOT B-full (Fork B: injected display-only marker columns via layout()/ColMap — reuses the ventilate/ColMap regime but Medium + highest correctness stakes; deferred unless a real need appears). No byte injection, no ColMap change -> no data-safety surface. Must honor the terminal-plain/no-color fallback (reverse/underline, not color alone). ~S. |
| H23 | palette_overlay_rect u16 overflow at extreme terminal width (H7-class geom) | triage | debt | S |  | Surfaced by the H21 whole-branch Fable probe (2026-07-16), PRE-EXISTING (byte-identical on main). chrome_geom::palette_overlay_rect computes ov_w = w*3/5 in u16; w*3 overflows u16 at terminal width >= 21846 (debug panic / release wrap-then-clamp). Reachable only via an absurd/hostile terminal Resize — NOT data-loss, NOT a merge blocker; same arithmetic-soundness class as H7. Fix: do the *3/5 in u32 (or saturating_mul) + clamp back; sweep sibling *_overlay_rect helpers. Batch into a future H7-style geom/panic sweep. |
| H25 | compose::face_to_ratatui is add-only — can't express modifier subtraction | triage | debt | S |  | compose only ADDS modifiers (no sub_modifier path), so a face's dim can't clear a DIM an underlay wrote; B7 worked around it at the ChromeStyles cache seam |
| M9 | Optional: upgrade/patch pulldown-cmark | watch | chore | S |  | M4-rest only ISOLATES its parse panic; a real upgrade is optional, low priority. |
| A18 | Snippet / abbreviation expansion (trigger to text; dynamic placeholders; cursor stop) | needs-design | feature | SM |  | Type an abbreviation -> canned text (sign-offs, boilerplate, dynamic date/time/clipboard). Our OWN proto-demo already exists: insert_date.lua (P1 command demo — inserts dynamic text), which is why a plugin is the natural home. External prior art: helowrite/src/snippets.py — flat trigger=replacement config, non-word-char boundary guard, longest-first, %PLACEHOLDER. AVOID their mechanism: find_trigger scans the WHOLE prefix per trigger = O(document) (violates theme-R / O(edited)) and can match a non-caret occurrence; silent except:pass; no cursor stop. OUR design: bounded-suffix detection ending AT the caret (O(edited)); word boundary via our UAX-29 machinery; expansion via submit_transaction (undo-atomic, M2); dynamic placeholders (%DATE/%TIME/%CLIPBOARD, reuse C3); ADD a final-cursor stop ($0) they skip; typed errors -> status line. HOME: an Effort-P plugin (P2 on-edit hook + command; insert_date.lua shows the shape) or a small core command. ~SM. |
| A19 | Writing sprint — word-count goal + session progress (edge-triggered; NOT an elapsed timer) | needs-design | feature | SM |  | Word-count GOAL for a document/session ('write 500 words') + a progress indicator in the chrome + words-added-this-session. EDGE-TRIGGERED off edits; builds on the shipped live word-count segment (render_status::word_count_segment) and the wordcount.lua P2 demo (a proto-goal). CORRECTION 2026-07-13: a session TIMER is NOT philosophically awkward — the plugin timer API is SHIPPED and idle-SAFE: wc.timer / wc.timer_cancel (P3 §3b, observer-tier), and the event loop blocks until the EARLIEST timer/swap deadline (timers.rs plugin_timer_deadline / next_due_ms), never polls. pomodoro.lua (tests/fixtures/plugins) is the P3 success-criterion demo of exactly this. So a pomodoro/session timer is available TODAY as a plugin (promote the fixture to a bundled plugin); A19's genuinely-new part is the goal/PROGRESS chrome, which a timer can drive. Related: PA (goals/streaks as post-P plugin). ~SM. |
| A20 | Forward-only drafting mode — toggle that disables deletion (Write-or-Die style) | needs-design | feature | SM |  | A toggle that DISABLES deletion (backspace / delete / cut) to force forward momentum through a first draft (Write-or-Die style). Pure input-filter toggle: no background work, no timer — on-brand for the 'drafting is hours, revision is weeks' thesis (a DRAFTING discipline; revision is where the lens/structure power lives). A registry command (toggle_forward_only, MenuMark::OnOff) that gates the delete/cut command paths while active. SAFETY: must NOT disable the toggle itself, navigation, save, or the recovery/undo safety net — there is always a way OUT; it blocks destructive EDITS, never the escape hatch. ~SM. Filed 2026-07-13 (helowrite comparison). |
| B6 | Heading-glyph STYLE toggle | needs-design | feature | SM |  | Cycle shades / Nerd numerals / inverted numerals; default stays universal Shades. |
| PD | wc.async — one-shot subprocess primitive (formatters / vale-CLI); closed op-menu + AsyncDone pump-drain | needs-design | feature | SM |  | wc.async — the deferred one-shot subprocess primitive (linter-arc effort d, but INDEPENDENT of the linter spine): a CLOSED Rust op-menu (wc.async{op,args,on_done}) + Msg::AsyncDone pump-drain + resource/security caps, with a formatter (prettier/fmt) or vale-CLI driver. The !Send constraint forces the closed-primitive shape (P3 F1-option-A). Depends ONLY on the shipped plugin system. Design: docs/design/prose-linters-design-space.md §2/§6 + effort-p3-grounding.md. |
| S3 | Snapshots — durable revision checkpoints | needs-design | feature | SM |  | Capture/list/diff/restore; reuses rope snapshot + ChangeSet; one net-new display diff. |
| E10 | Multi-engine linting (b) — ltex-ls-plus / LanguageTool provider + JVM lifecycle | needs-design | feature | M |  | Multi-engine linting effort (b): the ltex-ls-plus / LanguageTool provider + its JVM lifecycle — lazy-spawn on Review, Starting/'warming ltex…' status (no-silent-UI), idle-shutdown, never block the hot path. Reuses lsp_rpc.rs + the harper_ls.rs template; the ~300MB JVM 30s–2min warm-up is the only genuinely new risk. Builds on the SHIPPED diagnostics spine (harper-ls + selector); may need to finish the Vec/registry provider-seam generalization. Design: docs/design/prose-linters-design-space.md §6. |
| E11 | Multi-engine linting (c) — diagnostics viewing/action delta (href, detail region, dict/rule writers, executeCommand) | needs-design | feature | M |  | Multi-engine linting effort (c): the diagnostics VIEWING/ACTION delta — per-diagnostic 'learn more'/href + a detail region on DiagOverlay; per-engine (non-harper) dictionary/rule writers; the executeCommand relay; more-suggestions population. Consumes the SHIPPED-BUT-UNUSED Diagnostic.code/href fields. Parallelizable with E10. Design: docs/design/prose-linters-design-space.md §1/§6. |
| E12 | Multi-engine linting (e) — plugin-declared LSP servers + plugin-contributed engine-menu rows | needs-design | feature | M |  | Multi-engine linting effort (e), LAST/optional: plugins declare an LSP server + contribute dynamic engine-menu rows (MenuRowAction::Plugin widening). Only if plugin-authored engines materialize. Needs wc.async (PD) + the shipped spine + the deferred plugin-dynamic-menu-section effort. Design: docs/design/prose-linters-design-space.md §5/§6. |
| E8 | Lens — the unifying view surface (layout vs style axes; plugin-registerable) | needs-design | feature | M |  | PRODUCT CONCEPT (user, 2026-07-12): "a lens for your writing" — one first-class, summoned, non-destructive way of SEEING prose. REGROUNDED against SHIPPED code (a2f9062, multi-provider diagnostics SPINE + switchable lens — its spec/plan PREDATE its merge of this item, so E8 did not reach its design). THE REAL PROBLEM: FOUR toggle surfaces already answer "how do I see my prose" with four different shapes (RenderMode exclusive cycle — which ALSO gates diagnostics; active_analysis_source exclusive engine selector; toggle_focus boolean; typewriter/measure booleans) and nothing unifies them. S6 (layout) and S8 (style) add two more. THE FORK, now VALIDATED BY THE CODE not merely proposed: STYLE lenses PAINT and STACK (toggle_focus already stacks with the diagnostics underline today); LAYOUT lenses re-DRAW and are EXCLUSIVE. So: one active layout lens x N active style lenses. FINDING: DiagnosticsProvider is whole-document + async + process-lifecycled (ensure_running/shutdown/availability/notify_change(text: String)); S8's POS lens is caret-local, synchronous, processless — it MUST NOT implement that trait (a whole-doc String per check breaks the O(visible) rule). S8 is a different lens KIND. E8 is therefore a GENERALIZATION ABOVE the shipped engine-selector, NOT a widening of it; never-merge is RIGHT for diagnostics — do not re-litigate. Effort-P plugins should be able to REGISTER a lens. AUDIT 2026-07-14 (ad-hoc-surface sweep): the four view-toggle SETTERS are already seamed by command-surface-contract law-6 (one shared set_* per option) — so E8's remaining value is the PRODUCT lens MODEL (one layout lens × N style lenses), NOT an ad-hoc-surface cleanup. DISTINCT from the input-overlay dispatch table (H21): view lenses ≠ the search/menu/palette input modals. Stays gated on S6/S8. |
| PE | Bundled example plugins — full-featured writer plugins + authoring tutorials (each with a README) | needs-design | feature | M |  | Promote the P-demo fixtures (insert_date.lua P1, wordcount.lua P2, pomodoro.lua P3, currently under tests/fixtures/plugins) into a curated set of BUNDLED, user-facing EXAMPLE plugins that do DOUBLE DUTY: (a) create genuinely good writer experiences out of the box — real drafting-comfort features, not minimal demos; and (b) TEACH plugin authoring — exemplary, heavily-commented Lua that walks the command / on-edit hook / timer / config surfaces and the observer-tier vs edit-tier (submit_transaction) rules. EACH plugin ships its OWN README (what it does for the writer + a build-it-yourself walkthrough), so the set is the plugin-authoring tutorial corpus AND the shipped drafting-comfort layer. Likely needs a BUNDLED-PLUGINS mechanism (where they live, how they load/enable/update) if none exists — surface that first. Expand beyond the three; candidates map to the feature items: snippet expansion (A18 <- insert_date), word-count goal + progress (A19 <- wordcount), writing-session timer (<- pomodoro), forward-only mode (A20), insert-template. Later: an async example once wc.async (PD) lands. Relates: P (shipped system), A18/A19/A20 (the features). ~M. |
| S1 | Rearrangeable outline / heading-subtree corkboard | needs-design | feature | M |  | Structure mode: atomic heading-subtree move via submit_transaction; drag-reorder. Inherits select_section from S4, and OWNS section transpose (deliberately cut from S4: outline::sections yields NESTED/overlapping ranges — the 'next section' after an H2 is usually its own H3 child, so naive swap-with-next corrupts the document; S1 must solve sibling identification + separator normalization anyway). |
| S2 | Directory-as-binder | needs-design | feature | L |  | Directory of .md as a manuscript: ordered manifest + compile step (post-Effort-P plugin). |
| A15 | About command/menu item that shows the splash | triage | feature | TBD |  | About command/menu item that shows the splash |
| A22 | Write-Block Redirect exports the whole document, not the marked block | triage | feature | TBD |  | Write-Block Redirect exports the whole document, not the marked block |
| B14 | Ventilate lens treats tables as prose (no Table BlockRole → prose_block_at never declines) | triage | feature | TBD |  | Ventilate lens treats tables as prose (no Table BlockRole → prose_block_at never declines) |
| B15 | Shrink into a folded region leaves the caret on a hidden line (no SnapOut) | triage | feature | TBD |  | Shrink into a folded region leaves the caret on a hidden line (no SnapOut) |
| B16 | Scope::Sentence highlight window drifts from content-anchored select on indented prose | triage | feature | TBD |  | S4-INTRODUCED REGRESSION (re-recorded 2026-07-20; the filing said pre-existing): at merge base ef03888 select (commands.rs Scope::Sentence) and paint (render.rs, now gather_row_ctx) both used raw nav::paragraph_range_at — identical, no divergence; S4 (600cb92 + 2cece7e) content-anchored SELECT only, leaving the FocusGranularity::Sentence paint arm raw. Caret in a <=3-space indent -> painted active-sentence region diverges from select (SEE==SELECT violation on the PAINT side). Fix: route the paint arm through commands::prose_sentence_at (content-anchored, saturating), raw fallback on decline. |
| B9 | Menu bar horizontal overflow — clip/windowing for narrow terminals (<62 cols) | triage | feature | TBD |  | Menu bar horizontal overflow — clip/windowing for narrow terminals (<62 cols) |
| H13 | Editor is a 75-field data god-object | watch | debt | TBD |  | Field-clustering, not dispatch; NOT a defect. AUDIT 2026-07-14 reframe (field count 58→75): of 75 fields only ~12 are real ad-hoc debt — the `status` field (→ A17) and the 11 overlay Options whose DISPATCH, not data, is hand-parallel (→ H21). The overlays stay a flat XOR set (do NOT wrap in a sub-struct); it is their routing that wants a seam. Sole DRY nit among the pending_* is collapsing the 4 prompt-payload fields into Option<PromptPayload> (the other pending_* are unrelated axes — a naming rhyme, not a shared abstraction). The remaining ~46 fields are legitimately distinct state — healthy, not debt. Peel PendingActions/ClipboardState only if a refactor wants it. |
| H19 | Clean recovery files offers an opened recovered-*.md dump for deletion | triage | feature | TBD |  | Clean recovery files offers an opened recovered-*.md dump for deletion |
| H26 | fs-chokepoint guard: use-tree parsing for full soundness | triage | feature | TBD |  | fs-chokepoint guard: use-tree parsing for full soundness |
| H27 | dispatch signatures: pass DispatchCtx instead of 8 loose args | triage | feature | TBD |  | dispatch signatures: pass DispatchCtx instead of 8 loose args |
| H28 | Un-pumped picker tests assert unreachable states | triage | feature | TBD |  | Un-pumped picker tests assert unreachable states |
| H3 | Incremental-parser tail divergences | watch | debt | TBD |  | Cosmetic, self-healing via reconcile; NOT open correctness debt; chase only if a real case appears. |
| H30 | subprocess pipes are non-CLOEXEC — concurrent spawn can inherit another child's pipes | triage | bug | TBD |  | subprocess pipes are non-CLOEXEC — concurrent spawn can inherit another child's pipes |
| H32 | Consolidate the 13 duplicated test scratch-path helpers into one crate-wide seam | triage | feature | TBD |  | Consolidate the 13 duplicated test scratch-path helpers into one crate-wide seam |
| H33 | Test set_var(HOME) mutates process-wide state read by three config tests as an oracle | triage | feature | TBD |  | Test set_var(HOME) mutates process-wide state read by three config tests as an oracle |
| H34 | cursor_style restore_caret_if_written_gated_by_latch flake — 1/30, then 0/470 | watch | bug | TBD |  | Seen once (1/30) on a deliberately-broken tree; 0/470 on clean main at two concurrencies — rate bounded <0.64% |
| H35 | Position-space newtypes to tag confusable byte spaces | triage | feature | TBD |  | Position-space newtypes to tag confusable byte spaces |
| PA | Analysis / policy plugins | watch | research | TBD |  | Post-P candidates: writing goals/streaks, readability lens, CMS publish, backlinks. NOTE (triage 2026-07-13): the readability-lens slice is largely SUBSUMED — Hemingway = sentence length (S6 rhythm gutter, SHIPPED) + adverbs/passives (S8). Keep at most one slice as an E8 plugin-lens proof-case; do not rebuild it. |
| PB | Custom-markup plugins | watch | research | TBD |  | Post-P candidates clustering on a markup-extension API: CriticMarkup, Fountain, wiki-links. |
| PC | Lower-fit / principled plugin candidates | watch | research | TBD |  | Post-P: AI continuation (plugin-only on principle), book design, genre benchmarking. |
| S10 | Prose objects — Phrase/Clause select-only + D5 clause-splitting | triage | feature | TBD |  | The arc tail DEFERRED from S8 (spec 2026-07-17-s8-prose-lenses; the spec's 'S9' refs mean THIS item — id S9 was already taken). Build on S8's prose-lens spine + wordcartel-nlp classifier: a Phrase object (the chunker's NP runs — np flags already stored on TokenTag) and a Clause object (POS-informed clause-splitting; CCONJ/SCONJ/ADP disambiguates for/so/yet), both SELECT-ONLY. D5 THE LAW: POS-informed clause splitting is SELECT-ONLY behind a MEASURED precision gate (Brill is newswire-trained; it WILL mistag fiction/dialect/verse — so ship as selection, never mutation, and gate on measured precision). Reuses S8's PosStore/sweep substrate + the range-select nav pattern. Arc: docs/design/prose-structure-arc.md. |
| S9 | In-lens editing feel — refine caret/motion/reflow inside the ventilate lens | triage | feature | TBD |  | In-lens editing feel — refine caret/motion/reflow inside the ventilate lens |

## Shipped

<details><summary>72 shipped</summary>

| id | title | date | commit |
|---|---|---|---|
| H31 | config::files_type_filter_unknown flake — shared test temp path | 2026-07-20 | 44f1c14 |
| C5 | File interface — unify save/write onto the picker + favorites/recent | 2026-07-19 | 30f502a |
| H20 | Flaky test: filter::run_filter_non_zero_exit_carries_stderr | 2026-07-19 | b30f5aa |
| H29 | recovery::LAST_GOOD process-global race makes the test gate non-deterministic | 2026-07-19 | b30f5aa |
| S8 | Prose lenses — POS-driven stylistic X-rays (4-lens spine) | 2026-07-18 | 1570ea8 |
| A16 | Format menu: drop redundant Transform entry | 2026-07-17 | 505f093 |
| A21 | Overlay mouse interaction model — hover-highlights, wheel-scrolls-viewport | 2026-07-17 | 3f0bc11 |
| B7 | Selected menu-item text too light | 2026-07-17 | 505f093 |
| S7 | Linguistic substrate — harper-brill POS tagger + NP chunker in-process | 2026-07-17 | f7c4285 |
| B17 | Soft-wrap trailing space at margin gives no caret feedback — should wrap caret to next line, continuation flush at left margin (no leading-space indent) | 2026-07-16 | 2d6a2a3 |
| C6 | cut() writes register/clipboard BEFORE apply — a read-only Cut still syncs the clipboard though nothing is deleted | 2026-07-16 | 4003b6c |
| H10 | reduce's 10-stage intercept chain boilerplate | 2026-07-16 | e5d9b42 |
| H21 | Input-overlay dispatch table — OverlayId enum + OVERLAYS fn-ptr seam | 2026-07-16 | e5d9b42 |
| H22 | Universal edit chokepoint — route all internal edits through submit_transaction (M2 boundary) | 2026-07-16 | cf42284 |
| H24 | H22 follow-up hardening — EditOutcome #[must_use] + defensive finish_topic on read-only-reject async arms + INV-SEAM scan limit | 2026-07-16 | 94f0338 |
| A17 | Messaging / notification system — routed, browsable, plugin-emittable | 2026-07-15 | 2efc9fe |
| B10 | EOF caret glued to last content line (shared caret_line clamp) | 2026-07-14 | 44eacab |
| B11 | Modal/overlay caret parked under the modal (query field shows no caret) | 2026-07-14 | c740ba4 |
| B8 | Writing caret — DECSCUSR shape/blink config + cursor picker + panic-safe restore | 2026-07-14 | c740ba4 |
| S4 | Prose text objects — structural selection + operator layer | 2026-07-14 | 10b847e |
| S5 | Sentence authority — fix select_sentence, differential suite, sentence motions | 2026-07-13 | ab91584 |
| S6 | Ventilate-as-a-lens — non-destructive sentence view + rhythm gutter | 2026-07-13 | 04a2748 |
| P | Effort P — in-process Lua plugin system (P1 commands + P2 events/config/reload + P3 timers) | 2026-07-12 | 2988178 |
| E7 | Grammar/spelling as a deliberate Analysis view (F1 RenderMode); draft stays quiet | 2026-07-11 | 17ba839 |
| H12 | PTY smoke suite live-splash coverage (S9) | 2026-07-11 | 0dae170 |
| H17 | Pre-P public-API doc-coverage sweep | 2026-07-11 | 11408b8 |
| H18 | Supply-chain audit (cargo audit / cargo deny) | 2026-07-11 | ce403ac |
| H2 | Interrogate the burn/harper dependency weight | 2026-07-11 | ce403ac |
| H5 | App-managed cleanup of swap files / state-dir debris | 2026-07-11 | 0dae170 |
| M8 | M5 follow-up: undo louder-hint for buffer-level merges | 2026-07-11 | 0dae170 |
| A10 | Dedicated Block menu for marked-block commands | 2026-07-10 | 1f0a275 |
| A11 | Filter + transform SCOPE + filter docs | 2026-07-10 | 1f0a275 |
| A12 | Scratch buffer = a dedicated toggle | 2026-07-10 | 1f0a275 |
| A13 | Overlay mouse parity | 2026-07-10 | 1f0a275 |
| A14 | Emacs-parity prose editing commands (transpose, word-case, join-line, whitespace fixups) | 2026-07-10 | 1f0a275 |
| A3b | Item-by-item menu-curation pass | 2026-07-10 | 1f0a275 |
| A8 | Menu listing the open documents to switch between | 2026-07-10 | 1f0a275 |
| A9 | Wrap Column state-in-label | 2026-07-10 | 1f0a275 |
| E6 | Splash / start screen | 2026-07-10 | 242c987 |
| H11 | Decompose commands::run god-function | 2026-07-10 | 2437fca |
| H14 | Split the render() body by paint surface | 2026-07-10 | 2437fca |
| H7 | Panic-safety & arithmetic-soundness audit | 2026-07-10 | a49743e |
| H9 | Lift logical-line helpers out of derive | 2026-07-10 | 2437fca |
| H1 | God-object SEAM decomposition (app.rs/render.rs) | 2026-07-09 | 304e263 |
| H6 | Point-release version scheme + release process | 2026-07-09 | 50b449a |
| H8 | Remove dead fold/outline accessors | 2026-07-09 | b5a664a |
| A7 | Right-justify the state value in stateful menu rows | 2026-07-08 | 111e9b2 |
| B5 | Low heading-glyph collision (H5/H6) | 2026-07-08 | 111e9b2 |
| E5 | Chrome text intensity recede | 2026-07-08 | 111e9b2 |
| H16 | active_line end-of-buffer clamp | 2026-07-08 | 111e9b2 |
| H4 | PKGBUILD pandoc + TeX optdepends | 2026-07-08 | 111e9b2 |
| A3 | Option reachability + preset-aware hints | 2026-07-07 | d7a5494 |
| B4 | SRC-HI per-construct syntax highlighting | 2026-07-07 | 1bbd82b |
| C3 | Cross-app clipboard over SSH/tmux | 2026-07-07 | 16457f9 |
| R1 | Typing latency + double-Return / line-jump | 2026-07-07 | 02ac906 |
| E1 | Chrome/density presets (zen|full) | 2026-07-06 | f7b7b10 |
| E2 | Visual polish pass | 2026-07-06 | f7b7b10 |
| E3 | Chrome theming coherence | 2026-07-06 | eb9cfd1 |
| E4 | Bundled themes | 2026-07-06 | eb9cfd1 |
| A5 | Switch keymap preset from the menu | 2026-07-05 | 4670eaf |
| C2 | Transform scope (Reflow/Unwrap/Ventilate) | 2026-07-05 | 642290b |
| C2b | Repar 1.0 integration (width + fixups) | 2026-07-05 | c9b64d8 |
| D1 | Save settings from the session | 2026-07-05 | 4670eaf |
| A1 | Menu bar states + mouse reveal | 2026-07-04 | 7273327 |
| A6 | Palette/overlay full-list scrolling + wheel | 2026-07-04 | e2c7667 |
| B1 | Word-boundary wrap (UAX #14) | 2026-07-04 | 25b9776 |
| B2 | Sub-list bullet indent + hanging indent | 2026-07-04 | 25b9776 |
| C4 | Close-buffer Save/Discard/Cancel prompt | 2026-07-04 | b185e0b |
| H15 | app.rs/render.rs leaf extraction (first pass) | 2026-07-04 | 9e13164 |
| A2 | Full-width menu bar fill | 2026-07-03 | 097dcae |
| B3 | Heading glyphs default ON in every theme | 2026-07-03 | 097dcae |
| C1 | LaTeX export + xelatex PDF + export typography | 2026-07-03 | 097dcae |

</details>

## Dropped

<details><summary>2 dropped</summary>

| id | title | reason |
|---|---|---|
| A4 | Menu accelerators (Alt+F/Alt+E) | Category is 2 keystrokes / dwell+click away; Alt+letter conflict surface not worth a layer nobody asked for. Revisit on real demand. |
| E9 | Diagnostics lens: per-buffer vs global scope | Not a standalone effort — per-buffer vs global diagnostics scope is one of E8's lens axes (the 'scope axis' the code surfaced). Decide it within E8's design. Folded per the backlog-relationship triage 2026-07-13 (docs/design/backlog-integration-relationships.md). |

</details>
