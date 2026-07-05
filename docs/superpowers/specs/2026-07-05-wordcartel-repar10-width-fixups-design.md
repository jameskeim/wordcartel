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
  load clamp ≥20 with warning at :365-368). TWO consumers (Codex r1 I-1): the wrap-guide
  x-position (render.rs:282-288, gated on `wrap_guide`, default off) AND
  `nav::text_geometry` (nav.rs:25) — when `measure = true` it sets the centered column's
  `text_left`/`text_width` (pinned by existing tests at nav.rs:978 with wrap_column=40). NOT persisted by D1+A5
  (the spec's NEVER list cites "no runtime mutator exists" — this effort dissolves that
  rationale; the shipped D1+A5 spec text stays as history).
- repar 1.0.0 (path dep `../../par-command/repar`, Cargo.lock pins 1.0.0): the contract
  release. `Options::new()` now carries the `all` fixups bundle (UNICODE_CLASSES |
  REAL_ZERO_WIDTH — options.rs:293); fixup tokens are additive OR-flags; `"none"` CLEARS
  all fixups and overrides earlier tokens, later tokens re-add (options.rs:273/:292);
  `"prose"` = Compat::PROSE (:298), `"markdown"` = Compat::MARKDOWN (:299). The
  `--fixups=` vocabulary is frozen additive-only; the library surface is pinned
  (public_surface_pin_v1). Width: parse-time ceiling `WIDTH_PARSE_MAX` (huge;
  options.rs:500/:563); the format-time floor `ParError::WidthTooSmall` is RELATIVE —
  `width <= prefix + suffix` (driver.rs:128, error.rs:28), and `prose` suppresses only
  inferred SUFFIX (segment.rs:465), so a width of 20 can still fail on input whose
  inferred/explicit affixes total ≥20 (Codex r1 I-2). The clamp makes the error unlikely,
  not unreachable — the error path is REQUIRED, not belt-and-braces. The repar-nvim plugin pins `--fixups=none,<list>` precisely
  so editor behavior is independent of default flips (CHANGELOG 1.0.0) — the embedding
  pattern D3 adopts.
- Consequence already live but unpinned: wordcartel's transforms inherited `all` at the
  lock sync (multibyte width handling changed vs 0.9.x); the transform INPUT corpus is
  ASCII-only (transform.rs:386/:402/:412 area — Codex r1 m-2 precision) so no test
  noticed. This effort pins the post-1.0 behavior.
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
  **72** (from 80). Default-config transforms stay byte-identical to today. Two
  default-visible shifts, both honest (Codex r1 I-1): the wrap guide's position (guide
  defaults off) and the CENTERED MEASURE column narrows 80 → 72 for `measure = true`
  users who never set `wrap_column` (measure also defaults off; users who set the key
  keep their value). The clamp floor (20) and its warning are unchanged.
- repar width errors are REACHABLE (the floor is `width <= prefix + suffix` — affix
  inference on exotic input can exceed a small width; Codex r1 I-2) and surface through
  the existing `TransformError` status path — no new error handling, but the spec treats
  this as a live path, not belt-and-braces.
- The wrap guide keeps reading the same field — one knob, two honest consumers.

## D2. The setter + persistence

- Command `set_wrap_column`, label "Set Wrap Column…", `MenuCategory::Settings` (beside
  the keymap commands). Handler opens the minibuffer:
  `open_minibuffer("Wrap column: ", MinibufferKind::WrapColumn)` (new kind).
- Accept path — TWO real modification sites (Codex r1 I-3: Enter dispatch lives in
  app.rs:806's minibuffer match, and the parse/submit fns live in prompts.rs — e.g.
  `goto_line_submit` at prompts.rs:243): a new `MinibufferKind::WrapColumn` arm in
  app.rs's dispatch + a new `prompts::wrap_column_submit(editor, text)`.
  Semantics: parse `u16`; non-numeric → status "wrap column: not a number"; below 20 →
  clamp to 20 with status "wrap column: 20 (minimum)"; success → set + status
  "wrap column: {n}". DELIBERATE divergences from the goto_line family, chosen not
  inherited (Codex r1 I-4): goto_line says "not a line number" and clamps SILENTLY —
  this command names its own noun and surfaces the clamp, because a silently-moved
  formatting width is a surprise-diff class and a silently-moved scroll target is not.
  On ANY successful set the handler also triggers `derive::rebuild` (Codex r1 I-5:
  wrap_column feeds nav::text_geometry when measure is on — a bare field write would
  leave stale layout until the next edit). Esc cancels (minibuffer default).
- **Persistence (user-ratified fork 2-A):** `wrap_column` joins the persisted inventory —
  `SettingsSnapshot` gains `view_wrap_column: u16`; `OView` gains
  `wrap_column: Option<u16>` (serde skip-if-none like its siblings); the extension FOLLOWS THE SAME PER-FIELD PATTERN (not "generic" — Codex r1 m-1
  enumerates the real edits): `SettingsSnapshot` (settings.rs:32), `OView`
  (settings.rs:88), `snapshot_of` (:128), `runtime_snapshot` (:143), the view diff block
  + `any_view` (:268), the per-key mask predicate, every hand-built `SettingsSnapshot`
  literal in tests (e.g. config.rs:827), and the config.rs round-trip test gains the
  key. `diff_key<T: PartialEq + Clone>` handles u16 as-is. The
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
  exceed 40 under the old 72). An async sibling asserts the width reached the worker by observing the RETURNED
  TEXT's line widths in `Msg::TransformDone` (the message carries only the result text —
  transform.rs:230, Codex r1 m-3; assert no output line exceeds the set width on a
  corpus that would exceed it under the old default).
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
