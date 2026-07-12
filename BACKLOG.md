<!-- GENERATED from backlog.toml — do not edit by hand. -->
<!-- Regenerate: BLESS_BACKLOG=1 cargo test -p wordcartel --test backlog -->

# Backlog

**21 open · 49 shipped · 1 dropped**

Blocking Effort P: **0**

## Open

| id | title | status | kind | size | P? | hook |
|---|---|---|---|---|---|---|
| M9 | Optional: upgrade/patch pulldown-cmark | watch | chore | S |  | M4-rest only ISOLATES its parse panic; a real upgrade is optional, low priority. |
| B6 | Heading-glyph STYLE toggle | needs-design | feature | SM |  | Cycle shades / Nerd numerals / inverted numerals; default stays universal Shades. |
| S3 | Snapshots — durable revision checkpoints | needs-design | feature | SM |  | Capture/list/diff/restore; reuses rope snapshot + ChangeSet; one net-new display diff. |
| S1 | Rearrangeable outline / heading-subtree corkboard | needs-design | feature | M |  | Structure mode: atomic heading-subtree move via submit_transaction; drag-reorder. |
| S2 | Directory-as-binder | needs-design | feature | L |  | Directory of .md as a manuscript: ordered manifest + compile step (post-Effort-P plugin). |
| P | Effort P — in-process Lua plugin system (1.0 capstone) | needs-design | feature | XL |  | The plugin/automation spine; registers into the command/hook/job seams. See docs/design/effort-p-plugin-system-design-space.md. |
| S4 | Prose text objects — structural selection + operator layer | triage | feature | XL |  | Named, composable prose objects (sentence/clause/quotation/section) × operators (select/delete/transpose/reflow/count) over a document→section→block→sentence→clause→word hierarchy; expand/shrink selection; heuristic sentence-detection module; Markdown-tree-backed structural objects; seam with shipped repar (C2/C2b) + A14 operators; underpins S1. XL — may promote to own theme. Design-space: docs/design/prose-text-objects-design-space.md. |
| A15 | About command/menu item that shows the splash | triage | feature | TBD |  | About command/menu item that shows the splash |
| A16 | Format menu: drop redundant Transform entry | triage | feature | TBD |  | Format menu: drop redundant Transform entry |
| A17 | Messaging / notification system — routed, browsable, plugin-emittable | triage | feature | TBD |  | One routed path for all user messages: kinds/severity, browsable history, plugin emit API. noice.nvim = the anti-pattern (override-on-top) — we register into a seam. Effort-P design input; enables per-kind user routing/verbosity. |
| B7 | Selected menu-item text too light | needs-design | bug | TBD |  | Possible E5 regression; selected item may need a distinct legible highlight fg. |
| B8 | Configurable terminal caret shape / colour | needs-design | feature | TBD |  | Emit DECSCUSR (block/beam/underline, blink, colour); restore on exit/panic. |
| B9 | Menu bar horizontal overflow — clip/windowing for narrow terminals (<62 cols) | triage | feature | TBD |  | Menu bar horizontal overflow — clip/windowing for narrow terminals (<62 cols) |
| H10 | reduce's 10-stage intercept chain boilerplate | watch | debt | TBD |  | Verbatim flat-dispatch; NOT a defect. Collapse only when Effort P adds plugin intercept stages. |
| H13 | Editor is a 58-field data god-object | watch | debt | TBD |  | Field-clustering, not dispatch; NOT a defect. Peel PendingActions/ClipboardState only if a refactor wants it. |
| H19 | Clean recovery files offers an opened recovered-*.md dump for deletion | triage | feature | TBD |  | Clean recovery files offers an opened recovered-*.md dump for deletion |
| H20 | Flaky test: filter::run_filter_non_zero_exit_carries_stderr | triage | feature | TBD |  | Flaky test: filter::run_filter_non_zero_exit_carries_stderr |
| H3 | Incremental-parser tail divergences | watch | debt | TBD |  | Cosmetic, self-healing via reconcile; NOT open correctness debt; chase only if a real case appears. |
| PA | Analysis / policy plugins | watch | research | TBD |  | Post-P candidates: writing goals/streaks, readability lens, CMS publish, backlinks. |
| PB | Custom-markup plugins | watch | research | TBD |  | Post-P candidates clustering on a markup-extension API: CriticMarkup, Fountain, wiki-links. |
| PC | Lower-fit / principled plugin candidates | watch | research | TBD |  | Post-P: AI continuation (plugin-only on principle), book design, genre benchmarking. |

## Shipped

<details><summary>49 shipped</summary>

| id | title | date | commit |
|---|---|---|---|
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

<details><summary>1 dropped</summary>

| id | title | reason |
|---|---|---|
| A4 | Menu accelerators (Alt+F/Alt+E) | Category is 2 keystrokes / dwell+click away; Alt+letter conflict surface not worth a layer nobody asked for. Revisit on real demand. |

</details>
