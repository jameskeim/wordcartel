<!-- GENERATED from backlog.toml — do not edit by hand. -->
<!-- Regenerate: BLESS_BACKLOG=1 cargo test -p wordcartel --test backlog -->

# Backlog

**27 open · 34 shipped · 1 dropped**

Blocking Effort P: **0**

## Open

| id | title | status | kind | size | P? | hook |
|---|---|---|---|---|---|---|
| A13 | Overlay mouse parity | needs-design | feature | S |  | Click-to-select for theme picker + file browser; outline click-to-jump. |
| A3b | Item-by-item menu-curation pass | ready | chore | S |  | Per-command menu vs palette-only curation; concrete Q: move Filter Edit->Format? |
| A9 | Wrap Column state-in-label | needs-design | feature | S |  | Rename 'Set Wrap Column...' to stateful 'Wrap Column: <value>'. |
| H12 | PTY smoke suite live-splash coverage (S9) | ready | chore | S |  | Add an S9 check that launches WITHOUT --no-splash and asserts the real splash journey. |
| H18 | Supply-chain audit (cargo audit / cargo deny) | needs-design | chore | S |  | Add CVE + license + duplicate/ban scanning (cargo audit and/or cargo deny config); pairs with H2's pre-P dependency pass. No config today. |
| M8 | M5 follow-up: undo louder-hint for buffer-level merges | ready | debt | S |  | Finish the louder undo-eviction hint for buffer-level merges (the last M5 follow-up). |
| M9 | Optional: upgrade/patch pulldown-cmark | watch | chore | S |  | M4-rest only ISOLATES its parse panic; a real upgrade is optional, low priority. |
| B6 | Heading-glyph STYLE toggle | needs-design | feature | SM |  | Cycle shades / Nerd numerals / inverted numerals; default stays universal Shades. |
| H17 | Pre-P public-API doc-coverage sweep | ready | debt | SM |  | Doc-comment public items (~180 in core alone) + enable #![warn(missing_docs)] as a gate; Effort P exposes this surface to plugins. |
| S3 | Snapshots — durable revision checkpoints | needs-design | feature | SM |  | Capture/list/diff/restore; reuses rope snapshot + ChangeSet; one net-new display diff. |
| S1 | Rearrangeable outline / heading-subtree corkboard | needs-design | feature | M |  | Structure mode: atomic heading-subtree move via submit_transaction; drag-reorder. |
| S2 | Directory-as-binder | needs-design | feature | L |  | Directory of .md as a manuscript: ordered manifest + compile step (post-Effort-P plugin). |
| P | Effort P — in-process Lua plugin system (1.0 capstone) | needs-design | feature | XL |  | The plugin/automation spine; registers into the command/hook/job seams. See docs/design/effort-p-plugin-system-design-space.md. |
| A10 | Dedicated Block menu for marked-block commands | needs-design | feature | TBD |  | Move the blocks_marked command family into its own MenuCategory::Block. |
| A11 | Filter + transform SCOPE + filter docs | needs-design | feature | TBD |  | Unify buffer vs marked-block vs selection scope; document Filter. |
| A12 | Scratch buffer = a dedicated toggle | needs-design | feature | TBD |  | toggle_scratch round-trip; exclude scratch from cycle/switcher/open-docs menu. |
| A8 | Menu listing the open documents to switch between | needs-design | feature | TBD |  | Dynamic Window/Buffers/Documents menu auto-populated from open buffers. |
| B7 | Selected menu-item text too light | needs-design | bug | TBD |  | Possible E5 regression; selected item may need a distinct legible highlight fg. |
| B8 | Configurable terminal caret shape / colour | needs-design | feature | TBD |  | Emit DECSCUSR (block/beam/underline, blink, colour); restore on exit/panic. |
| H10 | reduce's 10-stage intercept chain boilerplate | watch | debt | TBD |  | Verbatim flat-dispatch; NOT a defect. Collapse only when Effort P adds plugin intercept stages. |
| H13 | Editor is a 58-field data god-object | watch | debt | TBD |  | Field-clustering, not dispatch; NOT a defect. Peel PendingActions/ClipboardState only if a refactor wants it. |
| H2 | Interrogate the burn/harper dependency weight | needs-design | research | TBD |  | 672-crate lockfile pulls a tensor stack transitively for grammar; keep/feature-gate/lighter backend? |
| H3 | Incremental-parser tail divergences | watch | debt | TBD |  | Cosmetic, self-healing via reconcile; NOT open correctness debt; chase only if a real case appears. |
| H5 | App-managed cleanup of swap files / state-dir debris | needs-design | chore | TBD |  | Auto-prune on launch vs an explicit 'Clean recovery files' command vs leave-to-user. |
| PA | Analysis / policy plugins | watch | research | TBD |  | Post-P candidates: writing goals/streaks, readability lens, CMS publish, backlinks. |
| PB | Custom-markup plugins | watch | research | TBD |  | Post-P candidates clustering on a markup-extension API: CriticMarkup, Fountain, wiki-links. |
| PC | Lower-fit / principled plugin candidates | watch | research | TBD |  | Post-P: AI continuation (plugin-only on principle), book design, genre benchmarking. |

## Shipped

<details><summary>34 shipped</summary>

| id | title | date | commit |
|---|---|---|---|
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
