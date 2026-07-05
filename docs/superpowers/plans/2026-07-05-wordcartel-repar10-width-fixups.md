# Repar 1.0 Integration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** transforms format to `view.wrap_column` (default 72, the hardcode dies), the fixups baseline becomes the pinned `none,all,prose,markdown` stack, a Set Wrap Column minibuffer setter makes the width live and persisted, and contract pins freeze repar-1.0 behavior.

**Architecture:** T1 rewires transform.rs (width capture before the branch, the fixups swap, the config default flip) and lands the four probe-grounded contract pins — with genuine RED evidence, since the ventilate and differentiation pins FAIL under today's stack. T2 adds the setter (one new `MinibufferKind` arm + one prompts submit fn + one registry command) and threads `wrap_column` through the Save Settings machinery per-field.

**Tech Stack:** Rust; shell crate only; repar 1.0.0 path dep (already locked); no new dependencies; no core changes.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-05-wordcartel-repar10-width-fixups-design.md` (CLEAN — Codex ×4 + Fable ×2 probe-verified; four user-ratified forks). Grounding with probe-generated expected literals: `.superpowers/sdd/repar10-grounding.md` (§B strings are copy-paste truth — generated against the locked repar 1.0.0; if a pin fails, suspect transcription first, then report BLOCKED with evidence — never adjust an expected literal to pass).
- **Gates after EVERY commit:** `cargo test -p wordcartel-core -p wordcartel` green; `cargo clippy --workspace --all-targets` clean (deny gate LIVE); `cargo build` warning-free. NO `cargo fmt`; `—` em-dash prose comments; no emoji IN CODE (the multibyte TEST CORPORA use é/中/🙂 by house convention — that is the sanctioned exception, and B1's expected literal is pinned in `\u{…}`-escaped form).
- **Trailing-space discipline (spec I-2):** B1's expected output CONTAINS trailing spaces (one per double-width char — the deliberately-pinned repar-1.0 artifact). The implementer must NOT trim them; reviewers must NOT flag them; the test carries the naming comment. Editors/linters that strip trailing whitespace are why the pin uses the `\u`-escaped single-line literal.
- Status copy byte-exact (spec D2): `"wrap column: not a number"`, `"wrap column: 20 (minimum)"`, `"wrap column: {n}"`.
- Line anchors are HEAD (`848c973`) references from the grounding; locate by quoted code if drifted.
- Exclude Cargo.lock drift from commits (`git checkout -- Cargo.lock` if the repar sibling bumps again mid-effort — and REPORT it if the version moves past 1.0.0, since the pins target 1.0.0).
- Every commit ends with the trailers, verbatim (use `git commit -F -` with a quoted 'EOF' heredoc — `!` breaks zsh inside double-quoted `-m`):
  ```
  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6
  ```

---

### Task 1: width wiring + fixups baseline + contract pins

**Files:**
- Modify: `wordcartel/src/transform.rs` (width capture, fixups swap, const removal, four new pins)
- Modify: `wordcartel/src/config.rs` (default 80 → 72 only)
- Modify: `wordcartel/src/app.rs` (the two dispatch-level pins in the test module — Codex plan r1 m-1)

**Interfaces:**
- Consumes: `editor.view_opts.wrap_column: u16` (exists); repar 1.0.0 (`Options::new().width(u32) -> PResult<Options>`, `apply_fixups(&str)`, verbs `"--reflow"`/`"--unwrap"`/`"--ventilate"`).
- Produces: `dispatch_transform` reads the width internally — signature UNCHANGED, all 6 callers untouched; `run_transform(kind, input, width)` unchanged; `DEFAULT_REFLOW_WIDTH` GONE (T2 consumes nothing from T1).

- [ ] **Step 1: the failing pins (genuine RED).** Add to transform.rs's test module (grounding §B literals verbatim — every expected string was probe-generated against the locked repar 1.0.0):

```rust
    #[test]
    fn ventilate_then_reflow_respects_sentence_boundaries() {
        // The user-found 0.9.x bug class, probe-reproduced at width 40 under the OLD
        // stack (periods detached + space-padded to the column, and one EXTRA period
        // fabricated: count 6 != 5). Corpus constraint (spec I-3): DISTINCT sentence
        // openings — par's common-PREFIX inference is untouched by prose and mangles
        // anaphoric corpora under every stack (recorded upstream candidate).
        let para = "Alpha wolves roam the northern ridge. Bright lanterns lit the harbor at dusk. Careful readers noticed the missing comma. Distant thunder rolled across the plain. Every morning she brewed strong coffee.\n";
        let ventilated = run_transform(TransformKind::Ventilate, para, 72).unwrap();
        assert_eq!(ventilated.lines().filter(|l| !l.trim().is_empty()).count(), 5,
            "precondition: one sentence per line");
        let reflowed = run_transform(TransformKind::Reflow, &ventilated, 40).unwrap();
        // Detector 1: no line-initial detached period.
        assert!(reflowed.lines().all(|l| !l.trim_start().starts_with('.')),
            "line-initial detached period: {reflowed:?}");
        // Detector 2: no space-before-period (the padding artifact).
        assert!(!reflowed.contains(" ."), "space-padded period: {reflowed:?}");
        // Detector 3 (the loss detector): period count == sentence count.
        assert_eq!(reflowed.matches('.').count(), 5, "periods lost or fabricated: {reflowed:?}");
    }

    #[test]
    fn fixups_stack_is_actually_applied() {
        // DECOMPOSED e + U+0301 (spec m-1: precomposed é and bare CJK are byte-identical
        // under both stacks — only zero-width handling differentiates). The D3 stack
        // zero-widths the combining mark; "none" counts it → earlier wrap.
        let input = "cafe\u{301} au lait cafe\u{301} noir cafe\u{301} creme cafe\u{301} latte cafe\u{301} mocha cafe\u{301} flat white here done.\n";
        let d3 = run_transform(TransformKind::Reflow, input, 40).unwrap();
        assert_eq!(d3, "cafe\u{301} au lait cafe\u{301} noir cafe\u{301} creme cafe\u{301}\nlatte cafe\u{301} mocha cafe\u{301} flat white here\ndone.\n");
        // Prove the stack reaches repar: "none" must produce a DIFFERENT wrap. This
        // arm needs a raw-repar comparison, not run_transform (which owns the stack):
        let mut none_opts = repar::Options::new().width(40).unwrap();
        none_opts.apply_par_args(["--reflow"]).unwrap();
        none_opts.apply_fixups("none").unwrap();
        let none_out = none_opts.format(input).unwrap();
        assert_ne!(d3, none_out, "the fixups stack must change behavior vs none");
    }

    #[test]
    fn reflow_multibyte_corpus_is_stable() {
        // Contract pin: byte-exact repar-1.0 output for mixed-width text at width 40.
        // KNOWN repar-1.0 artifact, pinned DELIBERATELY (spec I-2, upstream candidate):
        // one trailing space per double-width char per line (中文 → 2, 🙂 → 1). In
        // markdown, two-plus trailing spaces is a hard <br> — pre-existing repar
        // behavior, NOT this effort's bug. Do not trim; do not "fix" the literal.
        let input = "café serves thé while 中文 characters and 🙂 emoji flow together in one long prose paragraph that must wrap somewhere around forty columns wide here now.\n";
        let out = run_transform(TransformKind::Reflow, input, 40).unwrap();
        assert_eq!(out, "caf\u{e9} serves th\u{e9} while \u{4e2d}\u{6587} characters  \nand \u{1f642} emoji flow together in one long \nprose paragraph that must wrap somewhere\naround forty columns wide here now.\n");
    }

    #[test]
    fn transform_width_follows_wrap_column() {
        // run_transform-level width proof at 40 (the dispatch-level proof is below).
        let input = "The quick brown fox jumps over the lazy dog while seven bright birds sing above.\n";
        let at72 = run_transform(TransformKind::Reflow, input, 72).unwrap();
        assert!(at72.lines().next().unwrap().len() > 40, "precondition: 72-width line exceeds 40");
        let at40 = run_transform(TransformKind::Reflow, input, 40).unwrap();
        assert_eq!(at40, "The quick brown fox jumps over the lazy\ndog while seven bright birds sing above.\n");
    }
```

And in app.rs's test module (beside the existing transform behavior tests), the dispatch-level pins:

```rust
    #[test]
    fn dispatch_uses_wrap_column_for_width() {
        use crate::editor::Editor;
        use crate::transform::TransformKind;
        let text = "The quick brown fox jumps over the lazy dog while seven bright birds sing above.\n";
        let mut e = Editor::new_from_text(text, None, (80, 24));
        e.view_opts.wrap_column = 40;
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::transform::dispatch_transform(&mut e, TransformKind::Reflow, None, &TestClock(0), &tx);
        let after = e.active().document.buffer.to_string();
        assert!(after.lines().all(|l| l.len() <= 40), "reflow must honor wrap_column=40: {after:?}");
        assert!(after.lines().count() >= 2, "the corpus must actually have wrapped");
    }

    #[test]
    fn async_dispatch_uses_wrap_column_for_width() {
        use crate::editor::Editor;
        use crate::transform::TransformKind;
        // >1 MiB forces the async branch; the width must ride into the worker
        // (Msg::TransformDone carries only the result text — spec m-3 observable).
        let big = "word ".repeat(300_000);
        let mut e = Editor::new_from_text(&big, None, (80, 24));
        e.view_opts.wrap_column = 40;
        let (tx, rx) = std::sync::mpsc::channel::<Msg>();
        crate::transform::dispatch_transform(&mut e, TransformKind::Reflow, None, &TestClock(0), &tx);
        assert!(e.transform_in_flight);
        match rx.recv().expect("TransformDone must arrive") {
            Msg::TransformDone { result: Ok(out), .. } =>
                assert!(out.lines().all(|l| repar::display_width(l, 0, 8, repar::Compat::empty()) <= 40),
                    "worker must reflow at wrap_column=40"),
            other => panic!("expected TransformDone Ok, got {other:?}"),
        }
    }
```

- [ ] **Step 2: record the RED — honest accounting (Codex plan r1 F1).** Run `cargo test -p wordcartel -- ventilate_then_reflow fixups_stack reflow_multibyte transform_width dispatch_uses async_dispatch`. Expected RED (exactly three): `ventilate_then_reflow_respects_sentence_boundaries` FAILS (detectors 2+3 — the probe-recorded artifact: `" ."` padding, period count 6≠5 — THE prose-RED); `dispatch_uses_wrap_column_for_width` and `async_dispatch_uses_wrap_column_for_width` FAIL (both dispatch arms still hardcode 72). Expected GREEN-FROM-BIRTH (contract pins, not feature tests): `fixups_stack_is_actually_applied`, `reflow_multibyte_corpus_is_stable`, `transform_width_follows_wrap_column` — the current stack (`Options::new()` already carries `all`; `"markdown"` is additive) produces identical output on these corpora, and `run_transform` already takes the width parameter; these pins guard REGRESSION, they do not drive the change. Quote the three failures verbatim in the report; if a green-from-birth pin unexpectedly fails, STOP and report BLOCKED (a literal transcription error or a repar drift).

- [ ] **Step 3: implement.** transform.rs: DELETE `pub const DEFAULT_REFLOW_WIDTH: u32 = 72;` (:4). In `dispatch_transform`, after the `range.is_empty()` guard (post-:218), insert:

```rust
    // The one width knob (spec repar10 D1): transforms format to the same column the
    // wrap guide and the centered measure use. Captured as a Copy local so the async
    // worker closure moves a u32, never a borrow of editor.
    let width = u32::from(editor.view_opts.wrap_column);
```

Replace both `DEFAULT_REFLOW_WIDTH` arguments (:229 async closure, :238 sync arm) with `width`. In `run_transform` (:313):

```rust
    // The pinned fixups baseline (spec repar10 D3, the repar-nvim embedding pattern):
    // "none" first makes the stack independent of Options::new()'s defaults — repar
    // upgrades change wordcartel's output only when THIS string changes.
    opts.apply_fixups("none,all,prose,markdown").map_err(TransformError::from_repar)?;
```

config.rs (:102): `wrap_column: 80,` → `wrap_column: 72,` (fork 1a-A — the width default change is output-invisible for transforms; the doc row comment updates if one names 80). Grep `wrap_column` and `= 80` across tests for any default-80 assumption (the grounding found none; confirm and say so in the report).

- [ ] **Step 4: GREEN.** All six new pins pass; the five existing repar corpora pass UNCHANGED (probe-verified byte-identical under the new stack); the full two-crate suite green. Full gates.

- [ ] **Step 5: commit** — `feat(repar10): transforms format to wrap_column; pinned none,all,prose,markdown baseline; repar-1.0 contract pins`.

---

### Task 2: the setter + persistence

**Files:**
- Modify: `wordcartel/src/minibuffer.rs` (the variant), `wordcartel/src/app.rs` (one match arm), `wordcartel/src/prompts.rs` (submit fn + tests), `wordcartel/src/registry.rs` (command + membership test), `wordcartel/src/settings.rs` (per-field extension + pin), `wordcartel/src/config.rs` (round-trip additions)

**Interfaces:**
- Consumes: T1 — INCLUDING its config default flip to 72 (Codex plan r1 F3: `snap()` gains
  `view_wrap_column: 72`, which must equal `ViewConfig::default().wrap_column` or the
  existing `save_success_sets_status_and_returns_snapshot` sees a phantom divergence
  between a real Editor's runtime snapshot and the snap-built expectation; T2 is
  sequenced strictly after T1, and the implementer verifies the default is already 72).
- Produces: command `set_wrap_column` ("Set Wrap Column…", `MenuCategory::Settings`); `MinibufferKind::WrapColumn`; `prompts::wrap_column_submit`; `SettingsSnapshot.view_wrap_column: u16` + `OView.wrap_column: Option<u16>`.

- [ ] **Step 1: failing setter tests** (prompts.rs test module, the `goto_line_clamps_and_rejects_garbage` idiom at prompts.rs:324-336):

```rust
    #[test]
    fn wrap_column_submit_parses_clamps_and_rejects() {
        use crate::editor::Editor; // the prompts test module has only `use super::*` (Codex plan r1 F2)
        let mut e = Editor::new_from_text("a\n", None, (40, 10));
        crate::derive::rebuild(&mut e);
        let initial = e.view_opts.wrap_column;
        wrap_column_submit(&mut e, "xyz");                 // parse failure → UNCHANGED
        assert_eq!(e.view_opts.wrap_column, initial);
        assert_eq!(e.status, "wrap column: not a number");
        wrap_column_submit(&mut e, "99999");               // u16 overflow → UNCHANGED
        assert_eq!(e.view_opts.wrap_column, initial);
        assert_eq!(e.status, "wrap column: not a number");
        wrap_column_submit(&mut e, "15");                  // below min → CLAMPED SET
        assert_eq!(e.view_opts.wrap_column, 20);
        assert_eq!(e.status, "wrap column: 20 (minimum)");
        wrap_column_submit(&mut e, "55");                  // success
        assert_eq!(e.view_opts.wrap_column, 55);
        assert_eq!(e.status, "wrap column: 55");
    }
```

Registry: extend `settings_commands_registered_in_settings_category` (registry.rs:716-728) with `("set_wrap_column", "Set Wrap Column\u{2026}")` — note the label's `…` must match the register call's form (grounding shows goto_line uses `\u{2026}` in the label literal; match that convention). Settings: a diff-law pin (the `rule1…` shape, settings.rs:433-442):

```rust
    #[test]
    fn wrap_column_persists_through_the_diff_law() {
        let mut rt = snap("cua", ThemeIdentity::Builtin("default".into()), false);
        rt.view_wrap_column = 60;                          // diverged
        let base = snap("cua", ThemeIdentity::Builtin("default".into()), false);
        let of = compute_overrides(&rt, &base, &OverridesFile::default(), &OverridesFile::default());
        assert_eq!(of.view.as_ref().unwrap().wrap_column, Some(60), "rule 1 writes");
        // rule 3 + mask-guard arms:
        let rt2 = snap("cua", ThemeIdentity::Builtin("default".into()), false);
        let existing = parse_overrides("[view]\nwrap_column=60\n");
        let of2 = compute_overrides(&rt2, &base, &existing, &OverridesFile::default());
        assert!(of2.view.is_none(), "rule 3 removes the contradicted key");
        let mask = parse_mask("[view]\nwrap_column=90\n");
        let of3 = compute_overrides(&rt2, &base, &existing, &mask);
        assert_eq!(of3.view.as_ref().unwrap().wrap_column, Some(60), "mask-guard keeps");
    }
```

Plus the LIVE-PATH persistence pin (Codex plan r1 F4 — spec D4's "set → save → the file
carries [view] wrap_column" through the real setter + save pipeline, not synthetic
snapshots; home: settings.rs tests, which can reach prompts and perform_settings_save):

```rust
    #[test]
    fn set_wrap_column_then_save_writes_the_key() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("a\n", None, (40, 10));
        crate::prompts::wrap_column_submit(&mut e, "40");
        assert_eq!(e.view_opts.wrap_column, 40, "precondition: the setter took");
        let d = tempdir();
        let path = d.join("settings-overrides.toml");
        let base = snap("cua", ThemeIdentity::Builtin("default".into()), false); // baseline wrap 72
        let of = perform_settings_save(&mut e, false, Some(&path),
            &base, &OverridesFile::default(), &OverridesFile::default(), &crate::fsx::RealFs);
        assert!(of.is_some(), "save succeeds: {}", e.status);
        assert_eq!(e.status, "settings saved");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("wrap_column = 40"), "the file carries the key: {text}");
    }
```

(`runtime_snapshot` inside `perform_settings_save` reads the editor's live 40 vs the
baseline's 72 → rule-1 write. Reuse the settings test module's existing `tempdir` idiom.)
RED: none of the fields/fns exist.

- [ ] **Step 2: implement the setter.** minibuffer.rs: add `WrapColumn,` to `MinibufferKind` (:7-15 — doc comment `/// Numeric input for Set Wrap Column.`). app.rs (:812): add the arm `crate::minibuffer::MinibufferKind::WrapColumn => crate::prompts::wrap_column_submit(editor, &mb.text),` (the match is exhaustive — the compiler confirms this is the only site; grounding surprise 3 verified no other exhaustive match exists). prompts.rs (beside goto_line_submit):

```rust
/// Submit handler for Set Wrap Column (spec repar10 D2). Deliberate divergences from
/// the goto_line family: this command names its own noun and SURFACES the clamp — a
/// silently-moved formatting width is a surprise-diff class; a moved scroll target is
/// not. Parse failure leaves wrap_column unchanged; below-minimum is a SUCCESSFUL
/// clamped set. Any successful set rebuilds layout — wrap_column drives the centered
/// measure geometry, and a bare field write would leave stale layout until the next edit.
pub(crate) fn wrap_column_submit(editor: &mut crate::editor::Editor, text: &str) {
    let n: u16 = match text.trim().parse() {
        Ok(n) => n,
        Err(_) => { editor.status = "wrap column: not a number".to_string(); return; }
    };
    let (value, msg) = if n < 20 { (20, "wrap column: 20 (minimum)".to_string()) }
                       else { (n, format!("wrap column: {n}")) };
    editor.view_opts.wrap_column = value;
    editor.status = msg;
    crate::derive::rebuild(editor);
    crate::nav::ensure_visible(editor);
}
```

registry.rs (after save_settings, :474):

```rust
        r.register("set_wrap_column", "Set Wrap Column\u{2026}", Some(MenuCategory::Settings), |c| {
            c.editor.open_minibuffer("Wrap column: ", crate::minibuffer::MinibufferKind::WrapColumn);
            CommandResult::Handled
        });
```

- [ ] **Step 3: the persistence extension** (the per-field pattern — grounding §A settings.rs, every site enumerated): `SettingsSnapshot` gains `pub view_wrap_column: u16,` (:39); `OView` gains `#[serde(skip_serializing_if = "Option::is_none")] pub wrap_column: Option<u16>,` (:93); `snapshot_of` gains `view_wrap_column: cfg.view.wrap_column,` (:136); `runtime_snapshot` gains `view_wrap_column: editor.view_opts.wrap_column,` (:151); the view diff block gains (byte-parallel to the bool siblings — `Option<u16>` is Copy, so the mask predicate is `mk_view.and_then(|v| v.wrap_column).is_some()` and the existing arg `ex_view.and_then(|v| v.wrap_column.as_ref())`):

```rust
    let wrap_column = diff_key(
        &runtime.view_wrap_column, &baseline.view_wrap_column,
        ex_view.and_then(|v| v.wrap_column.as_ref()),
        mk_view.and_then(|v| v.wrap_column).is_some(),
    );
```

with `wrap_column.is_some()` OR'd into `any_view` and the field added to the `OView` literal. The `snap()` test helper (:426-431) gains `view_wrap_column: 72,` (the new default — every existing diff-law test stays semantically identical since rt and base then agree). config.rs round-trip (:827): the literal gains `view_wrap_column: 100,` and the assertions gain `assert_eq!(cfg.view.wrap_column, 100);` (100 = distinct from both defaults, above the clamp).

- [ ] **Step 4: GREEN + full gates.** Also verify the two hand-built `SettingsSnapshot` literals were the complete set (grounding surprise 5: settings.rs:426 + config.rs:827 — the compiler enforces it; say so in the report).

- [ ] **Step 5: smoke.** Run `scripts/smoke/run.sh` once; quote the one-line summary VERBATIM in the report (advisory).

- [ ] **Step 6: commit** — `feat(repar10): Set Wrap Column minibuffer setter — live width, measure rebuild, Save Settings persistence`.

---

## Verification appendix (final gates charge)

- The four ratified forks hold: one knob (transforms + guide + measure agree); default 72 (transform-output-invisible); minibuffer setter with the distinguished input semantics (parse failure = unchanged; below-min = clamped SET with rebuild); the pinned `none,all,prose,markdown` baseline.
- The contract pins are probe-true (grounding §B literals) and carry their mandated comments: the trailing-space artifact named as repar's (never trimmed), the distinct-openings constraint, the decomposed-accent requirement.
- `dispatch_transform`'s signature unchanged — all 6 callers byte-identical; `DEFAULT_REFLOW_WIDTH` gone from the tree.
- The five pre-existing repar corpora pass unchanged — enforced by the suite itself
  (they are existing tests that must stay green through T1's stack swap; the probe
  predicted byte-identity, the suite proves it); no test asserts the old default 80.
- The upstream-report candidates (CJK trailing spaces → markdown hard-break; prefix-inference anaphora mangling) are recorded in the spec's Deferred — surface them in the ship report for the user's par-command work.
- Pre-merge: smoke verbatim + a live tmux sanity (set wrap column 40 via the Settings menu → reflow a paragraph → verify 40-column output and the measure column narrows when measure is on; Save Settings → `[view] wrap_column = 40` in the overrides file).
- Ship-time bookkeeping: backlog — record the repar10 effort (not a lettered backlog item; note it under the transforms/C2 lineage), update the E1 chrome table's wrap_column row, working order still points at E3 next.
