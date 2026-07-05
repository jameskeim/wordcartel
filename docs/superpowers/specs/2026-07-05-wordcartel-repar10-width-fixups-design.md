# Repar 1.0 integration — width wiring, fixups baseline, contract pins (design)

Status: DRAFT (user-approved design 2026-07-05; four forks resolved one at a time)
Effort: user-directed insertion ahead of E3 in the working order (reconstructs the
repar-integration intent from the par-command session of 2026-07-04, which this session
never received). Companion context: repar 1.0.0 landed on disk mid-D1+A5 (the T1
adaptation @ a5a25df + lock sync @ cac45af were emergency fixes; this effort owns the
integration deliberately).

## Goals

1. **Width:** transforms format to a user-definable column — `view.wrap_column` becomes
   the single knob (guide position + formatting width); the hardcoded 72 dies.
2. **Setter:** a `set_wrap_column` minibuffer command makes it live-adjustable, and
   `wrap_column` joins the Save Settings persisted inventory.
3. **Fixups:** the transform baseline becomes the pinned stack `none,all,prose,markdown`
   — picking up `prose` (sentence-boundary handling; the ventilate→reflow stranded-period
   fix the user drove in the repar project) and making the 1.0-inherited `all` explicit.
4. **Contract pins:** behavior tests that make the next repar bump unable to move
   wordcartel's transform output silently.

## Non-goals

- Soft-wrap stays display-driven — B1 untouched; `wrap_column` does NOT drive visual
  wrap (E1 territory; user-ratified fork 1 rejected the "auto" window-tracking mode).
- No separate `[transform]` config section (fork 1 rejected the two-knob split).
- No exposure of further repar options (`strict_par`, prefix/suffix/hang, `--width=auto`
  semantics are CLI-only concerns; the library always receives an explicit width).
- No new e2e journey — C2's transform journeys already drive the dispatch path; this
  effort's coverage is unit/behavior-level.

## Grounded facts (verified 2026-07-05 at fd1d06b + repar 1.0.0 source)

- `DEFAULT_REFLOW_WIDTH: u32 = 72` (transform.rs:4) feeds `run_transform` at the async
  (transform.rs:229) and sync (transform.rs:238) call sites. `run_transform(kind, input,
  width)` (transform.rs:309-317) builds `repar::Options::new().width(width)` (PResult
  since 1.0 — adapted @ a5a25df) then `apply_par_args([kind.verb()])`,
  `apply_fixups("markdown")`, `format(input)`; errors ride `TransformError::from_repar`
  → the status line.
- `view.wrap_column: u16` (config.rs:94, default 80 at :102, RawView Option at :271,
  load clamp ≥20 with warning at :365-368). Sole consumer: the wrap-guide x-position
  (render.rs:288, gated on `view_opts.wrap_guide`, default off). NOT persisted by D1+A5
  (the spec's NEVER list cites "no runtime mutator exists" — this effort dissolves that
  rationale; the shipped D1+A5 spec text stays as history).
- repar 1.0.0 (path dep `../../par-command/repar`, Cargo.lock pins 1.0.0): the contract
  release. `Options::new()` now carries the `all` fixups bundle (UNICODE_CLASSES |
  REAL_ZERO_WIDTH — options.rs:293); fixup tokens are additive OR-flags; `"none"` CLEARS
  all fixups and overrides earlier tokens, later tokens re-add (options.rs:273/:292);
  `"prose"` = Compat::PROSE (:298), `"markdown"` = Compat::MARKDOWN (:299). The
  `--fixups=` vocabulary is frozen additive-only; the library surface is pinned
  (public_surface_pin_v1). Width: parse-time ceiling `WIDTH_PARSE_MAX` (huge;
  options.rs:500/:563), format-time floor via `ParError::WidthTooSmall` (driver.rs:133)
  — our ≥20 clamp clears it. The repar-nvim plugin pins `--fixups=none,<list>` precisely
  so editor behavior is independent of default flips (CHANGELOG 1.0.0) — the embedding
  pattern D3 adopts.
- Consequence already live but unpinned: wordcartel's transforms inherited `all` at the
  lock sync (multibyte width handling changed vs 0.9.x); the ASCII-only transform test
  corpus never noticed. This effort pins the post-1.0 behavior.
- Minibuffer precedent: `goto_line` (registry.rs:424-425) opens
  `open_minibuffer("Go to line: ", MinibufferKind::GotoLine)`.
- Save Settings machinery (D1+A5, shipped @ 4670eaf): `settings.rs` — `SettingsSnapshot`,
  `OView` (five Option<bool> toggles), `snapshot_of`/`runtime_snapshot`,
  `compute_overrides` (the four-rule diff law + per-key mask-guard), round-trip test in
  config.rs. All extend per-key generically.

## D1. Width wiring

- `DEFAULT_REFLOW_WIDTH` is REMOVED. `dispatch_transform` captures
  `let width = u32::from(editor.view_opts.wrap_column);` once, before the branch; the
  sync arm passes it to `run_transform`; the async arm moves it into the worker closure
  (like the region). `run_transform`'s signature is unchanged.
- Default change (user-ratified fork 1a-A): `ViewConfig::default().wrap_column` becomes
  **72** (from 80). Default-config transforms stay byte-identical to today; the only
  default-visible shift is the wrap guide's position, and the guide defaults off. The
  clamp floor (20) and its warning are unchanged.
- Belt-and-braces: repar width errors (unreachable via the clamp, reachable if repar's
  floor ever rises) surface through the existing `TransformError` status path — no new
  error handling.
- The wrap guide keeps reading the same field — one knob, two honest consumers.

## D2. The setter + persistence

- Command `set_wrap_column`, label "Set Wrap Column…", `MenuCategory::Settings` (beside
  the keymap commands). Handler opens the minibuffer:
  `open_minibuffer("Wrap column: ", MinibufferKind::WrapColumn)` (new kind).
- Accept path (the GotoLine arm's shape): parse `u16`; non-numeric → status
  "wrap column: not a number" (copy pinned by plan against the goto_line copy family);
  below 20 → clamp to 20 with the SAME message shape load() uses ("wrap_column {n} below
  min 20; clamped to 20" adapted to status casing — plan pins exact copy); success →
  `view_opts.wrap_column = n` + status "wrap column: {n}". Esc cancels (minibuffer
  default behavior, nothing to add).
- **Persistence (user-ratified fork 2-A):** `wrap_column` joins the persisted inventory —
  `SettingsSnapshot` gains `view_wrap_column: u16`; `OView` gains
  `wrap_column: Option<u16>` (serde skip-if-none like its siblings); `snapshot_of`/
  `runtime_snapshot`/`compute_overrides`' view section/the per-key mask predicate extend
  with the same `diff_key` shape; the config.rs round-trip test gains the key. The
  D1+A5 NEVER-persisted list shrinks by exactly this entry (rationale dissolved: a
  runtime mutator now exists).

## D3. Fixups baseline (user-ratified fork 3-A)

- `run_transform` replaces `apply_fixups("markdown")` with
  `apply_fixups("none,all,prose,markdown")` — clear-then-rebuild, the repar-nvim
  embedding pattern. Comment names the contract: "repar upgrades change wordcartel
  output only when this string changes; `none` first makes the stack independent of
  Options::new()'s defaults."
- Net behavior change vs today: `prose` is ADDED (sentence-boundary handling — the
  ventilate→reflow stranded-period class); `all` becomes explicit (already inherited
  since the 1.0 lock sync); `markdown` unchanged.

## D4. Contract pins (behavior, not strings)

New transform.rs tests (corpora chosen at implementation, probe-verified):
- `reflow_multibyte_corpus_is_stable`: a UTF-8 corpus (é / 中 / 🙂 — the house multibyte
  convention the transform battery currently lacks) reflowed at a fixed width; expected
  output asserted byte-exact. This is the pin that makes the next repar bump's drift
  loud.
- `ventilate_then_reflow_respects_sentence_boundaries`: the user-found bug class — after
  ventilate → reflow at a width chosen to tempt the old artifact, assert no period is
  DETACHED from its word (the observed 0.9.x artifact was punctuation pushed to the far
  right column separated from its sentence): no output line begins with `.`, and no `.`
  is preceded by a space. Assert the invariant, not exact layout.
- `fixups_stack_is_actually_applied`: a corpus whose reflow output DIFFERS between the
  D3 baseline and `"none"` (e.g. a zero-width or accented-measure case); assert the two
  differ and the baseline output is the expected one — proves the stack reaches repar.
- `transform_width_follows_wrap_column`: `view_opts.wrap_column = 40` → dispatch (sync
  arm) reflows at 40 (no output line exceeds 40 columns; precondition: the corpus would
  exceed 40 under the old 72). An async sibling asserts the worker receives the same
  width (the 1.5 MB corpus shape from the existing async test, wrap_column asserted in
  the TransformDone output shape or via a narrower honest observable — plan grounds it).
- Setter pins: parse/clamp/cancel/status per D2; registry membership (Settings, label).
- Persistence pins: set → save → the overrides file carries `[view] wrap_column`;
  reload round-trips through the real `config::load`; the diff-law matrix gains the key
  (write-on-divergence + keep/remove/mask arms via the existing generic tests' pattern).

## Error handling

- Non-numeric / out-of-range minibuffer input: status line, buffer unchanged (D2).
- repar errors: unchanged path (`TransformError` → status; guarded_transform panic
  isolation intact).
- No new IO, no new refusal states; Save Settings semantics inherit D1+A5's shipped
  behavior with one more key.

## Deferred (recorded)

- Exposing further repar knobs (prefix/suffix/hang, strict mode) — on demand.
- Soft-wrap/measure interaction with wrap_column — E1.
- A wrap-guide-follows-reflow-width visual affordance beyond today's guide — E3/E1.
