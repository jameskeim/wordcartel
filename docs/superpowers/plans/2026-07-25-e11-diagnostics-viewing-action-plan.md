# E11 — Diagnostics Viewing/Action Layer + Fix-Pipeline Repair Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Repair the fix pipeline for all three engines (per-engine fix kinds, both edit shapes,
attach-all), move fix requests to on-demand-at-overlay-open (deleting the Assembly/parking
machinery), and build the viewing/action layer: row-model refactor, kind-aware rows, the
sentence+line-pair session dismiss, the Learn-more copy row, and the bottom-anchored detail box.

**Architecture:** Spec `docs/superpowers/specs/2026-07-25-e11-diagnostics-viewing-action-design.md`
(Codex-gated READY, round 6 — READ the sections each task cites; its D1–D10 decisions and its
exactly-once/attribution rules are LAW here). Branch `effort-e11-diag-viewing-action` off main.
T1 is a pre-code wire probe whose findings BIND T4's request construction. T2–T4 rework
`lsp_rpc.rs`/`lsp_client.rs` (mapping, parking deletion, the `pending_fix` slot + attribution);
T5 threads the provider/editor/reduce seams; T6–T9 build the overlay/UX; T10 is the advisory
live probe.

**Tech Stack:** Rust, serde_json, std. No new dependencies.

## Global Constraints

- **Command-surface contract** (`docs/design/command-surface-contract.md`): E11 adds NO command,
  NO user-settable option, NO menu row, NO keybinding. The new overlay rows follow the SHIPPED
  precedent: "Ignore once"/"Add to dictionary" are overlay-internal list rows, not registry
  commands (never in the palette; overlay keys are hardcoded in `diag_overlay::intercept`, not
  `KeyTrie`-routed). `session_dismissals` is session-transient working state like
  `session_ignores` (law 2 not triggered); the row handlers are single-caller (law 6 not
  implicated). `quick_fix`/`diag_next`/`diag_prev` keep their ids/labels/bindings. **No contract
  amendment.**
- **The E10 pin is OVER, deliberately:** E11 rewrites pipeline behavior harper's inline tests
  encode (publish→park→batched-codeAction→attach). T3 rewrites/deletes those tests WITH the
  behavior change, and its commit message says so (spec §3.7 carries the full inventory).
- Constants (spec-fixed): `FIX_REQUEST_TIMEOUT_MS: u64 = 10_000` (lsp_client.rs).
- Every commit leaves `cargo test --workspace` green and `cargo clippy --workspace --all-targets`
  clean. Do NOT run `cargo fmt` (hand-formatted repo). House style: match neighbors, `—` in
  prose comments, no emoji outside multibyte-text tests.
- Anchor on symbol names; line numbers drift as tasks land.
- Commit messages end with the project trailers (CLAUDE.md): the `Co-Authored-By: Claude Opus
  4.8 <noreply@anthropic.com>` line, then `Claude-Session:` with YOUR harness-supplied session
  URL (from your own harness instructions — never constructed, never invented).

**Dependency edges:** T1 → T4; T2 → T3 → T4 → T5 → T6 → {T7, T8}; T9 after T6; T10 last.

---

### Task 1: The ltex per-diagnostic codeAction wire probe (NO app code)

**Files:**
- Create: `scratchpad/e11/probe/t1-perdiag-results.md` (+ a probe script beside it)
- No source changes. This task BINDS T4's request-construction step.

**What it must establish (spec §2.3):** the E11 pre-spec probe's single-diagnostic ltex request
(exact range, one diagnostic echoed in `context.diagnostics`) returned ONLY the three
command-only actions — NO `quickfix.ltex.acceptSuggestions` — while the whole-document batch
returned an accept-suggestion for the same diagnostic. T1 re-runs the probe
(`scratchpad/e11/probe/ltex_probe.py` is the template) against real `ltex-ls-plus`, varying ONLY
the request construction, echoing the server's RAW diagnostic bytes verbatim:

- [ ] **Step 1:** Variant A — range = the diagnostic's exact range; `context.diagnostics` =
  [that one raw diagnostic, verbatim].
- [ ] **Step 2:** Variant B — range = the diagnostic's exact range; `context.diagnostics` =
  the FULL raw array from the last publish (all diagnostics, verbatim).
- [ ] **Step 3:** Variant C — range = a caret-style empty range inside the diagnostic;
  context as A.
- [ ] **Step 4:** Record verbatim JSON per variant in the results file with a one-line verdict:
  which variants elicit `acceptSuggestions` for the targeted diagnostic.

**Binding outcomes (write the chosen letter into the results file; T4 Step 5 reads it):**
- **Outcome A (any single-raw variant works):** T4's default construction stands — echo the
  triple-matched raw alone.
- **Outcome B (only all-raws-in-context works):** T4 uses the spec's documented per-engine
  fallback — a `LspEngine` request-shape hook where ltex sends range = the anchor's range,
  `context.diagnostics` = ALL retained raws (still ONE request at overlay-open); the §4
  mapping's anchor-range equality filters the response to the anchor's own fixes. Harper/vale
  keep the single-raw shape (vale's per-diagnostic behavior is probe-proven: 7/7 repetition
  fixes in isolation).
- **Outcome C (neither works):** STOP and report to the controller — a finding against the
  spec's D2 shape, not something to improvise around.

No commit (scratch results only).

---

### Task 2: The engine-parameterized suggestion mapping (TDD, scaffolded)

**Files:**
- Modify: `wordcartel/src/lsp_rpc.rs`, `wordcartel/src/lsp_client.rs` (trait),
  `wordcartel/src/harper_ls.rs` / `ltex_ls.rs` / `vale_ls.rs` (one hook impl each)
- Test: inline in `lsp_rpc.rs` (new module section), using VERBATIM probe JSON as fixtures

**Interfaces (produces):** `LspEngine::is_fix_kind(kind: &str) -> bool` (required trait fn);
`lsp_rpc::action_fix_suggestion(action, our_uri, doc_text, anchor, accept_kind) -> Option<Suggestion>`
and `lsp_rpc::collect_fix_suggestions(actions, our_uri, doc_text, anchor, accept_kind) -> Vec<Suggestion>`.
T4 consumes both. The OLD `quickfix_suggestion` and its caller stay untouched this task (deleted
in T3) — the tree stays green with the shipped pipeline running.

**Greenness note (explicit):** the two new fns are unused until T4 wires them; each carries
`#[allow(dead_code)] // E11 T2 scaffold — consumed by T4's on_fix_response; allow removed there`
(the E10 scaffold precedent; T4's checklist removes the allows).

- [ ] **Step 1: Write the failing tests** (append to `lsp_rpc.rs::tests`). Two fixture tiers
  (plan-gate finding 10): the shape tests below use REDUCED SYNTHETIC fixtures (labeled as
  such — simplified URIs/positions for readable assertions), and two VERBATIM-capture tests
  follow them, copied byte-for-byte from `scratchpad/e11/probe/ltex-probe-results.md` Q2 and
  `scratchpad/e11/probe/vale/vale-probe-results.md` §2 — the wire-regression protection:

```rust
    // ── VERBATIM probe captures (wire-regression tier; do not simplify) ─────────────────────

    #[test]
    fn verbatim_ltex_recieve_accept_action_maps() {
        // ltex-probe-results.md Q2, the `recieve` → `receive` accept-suggestion action,
        // fields as captured (documentChanges shape; diagnostics echo included).
        let doc = "# Probe Document\n\nThis is a probe document for ltex-ls-plus. It was written by us to test the checker.\n\nThe the cake was eaten by the dog.\n\nI recieve many emails every day due to the fact that people email me constantly.\n";
        let action = serde_json::json!({
            "title": "Use 'receive'",
            "kind": "quickfix.ltex.acceptSuggestions",
            "diagnostics": [{
                "range": {"start": {"line": 6, "character": 2}, "end": {"line": 6, "character": 9}},
                "severity": 3, "code": "MORFOLOGIK_RULE_EN_US",
                "codeDescription": {"href": "https://community.languagetool.org/rule/show/MORFOLOGIK_RULE_EN_US?lang=en-US"},
                "source": "LTeX", "message": "'recieve': Possible spelling mistake found."
            }],
            "edit": {"documentChanges": [{
                "textDocument": {"version": 1, "uri": "file:///probe/doc.md"},
                "edits": [{"range": {"start": {"line": 6, "character": 2},
                                     "end": {"line": 6, "character": 9}},
                           "newText": "receive"}]}]}
        });
        let anchor = lsp_range_to_bytes(doc, (6, 2), (6, 9)).expect("anchor range");
        assert_eq!(action_fix_suggestion(&action, "file:///probe/doc.md", doc, &anchor,
            |k| k == "quickfix.ltex.acceptSuggestions"),
            Some(Suggestion::ReplaceWith("receive".into())));
    }

    #[test]
    fn verbatim_vale_misspelling_action_maps() {
        // vale-probe-results.md §1+§2 verbatim: the first of the 5 spelling quickfixes, with
        // the captured diagnostic echoed IN FULL — including the native `data` object and the
        // TYPOGRAPHIC-quote title (multibyte, deliberate — the capture's exact bytes). The
        // doc string places `mispeling` at line 2, chars 20..29 (the captured positions);
        // fix the DOC string, never the captured JSON.
        let doc = "# Probe\n\nThe word here has a mispeling in it.\n";
        let uri = "file:///home/jkeim/projects/groundwords/scratchpad/e11/probe/vale/test.md";
        let action = serde_json::json!({
            "diagnostics": [{
                "code": "Vale.Spelling",
                "data": {
                    "Action": { "Name": "suggest", "Params": ["spellings"] },
                    "Check": "Vale.Spelling",
                    "Description": "",
                    "Line": 3,
                    "Link": "",
                    "Match": "mispeling",
                    "Message": "Did you really mean 'mispeling'?",
                    "Severity": "error",
                    "Span": [21, 29]
                },
                "message": "Did you really mean 'mispeling'?",
                "range": {"end": {"character": 29, "line": 2}, "start": {"character": 20, "line": 2}},
                "severity": 1,
                "source": "vale-ls"
            }],
            "edit": {"changes": { uri: [{
                "newText": "misspelling",
                "range": {"end": {"character": 29, "line": 2}, "start": {"character": 20, "line": 2}}}]}},
            "kind": "quickfix",
            "title": "Replace with \u{2018}misspelling\u{2019}"
        });
        let anchor = lsp_range_to_bytes(doc, (2, 20), (2, 29)).expect("anchor range");
        assert_eq!(&doc[anchor.clone()], "mispeling", "the captured range targets the word");
        assert_eq!(action_fix_suggestion(&action, uri, doc, &anchor, |k| k == "quickfix"),
            Some(Suggestion::ReplaceWith("misspelling".into())));
    }
```

  (The verbatim fixtures pin field NESTING and extra fields — `diagnostics` echoes,
  `severity`, `codeDescription` — so a parser regression that trips on real wire shapes
  fails here even when the reduced fixtures pass. The `doc` strings place the captured
  line/char positions on real text; if a position does not land on the flagged word at
  transcription, fix the DOC string, never the captured JSON.) The reduced synthetic tests:

```rust
    // ── E11 T2: the engine-parameterized fix mapping ────────────────────────────────────────

    /// ltex accept-suggestion action (probe Q2 verbatim shape): namespaced kind +
    /// edit.documentChanges[].
    fn ltex_accept_action(uri: &str, new_text: &str) -> serde_json::Value {
        serde_json::json!({
            "title": format!("Use '{new_text}'"),
            "kind": "quickfix.ltex.acceptSuggestions",
            "edit": {"documentChanges": [{
                "textDocument": {"version": 1, "uri": uri},
                "edits": [{"range": {"start": {"line": 0, "character": 2},
                                     "end": {"line": 0, "character": 9}},
                           "newText": new_text}]}]}
        })
    }

    #[test]
    fn ltex_documentchanges_accept_action_maps_to_replace_with() {
        let text = "a recieve b";
        let anchor = 2..9usize;
        let a = ltex_accept_action("untitled:wcartel-0-1", "receive");
        assert_eq!(
            action_fix_suggestion(&a, "untitled:wcartel-0-1", text, &anchor,
                |k| k == "quickfix.ltex.acceptSuggestions"),
            Some(Suggestion::ReplaceWith("receive".into())));
    }

    #[test]
    fn ltex_command_only_kinds_are_excluded_by_engine_knowledge() {
        // Probe Q2: addToDictionary / hideFalsePositives / disableRules are command-only.
        let a = serde_json::json!({"title": "Add 'x' to dictionary",
            "kind": "quickfix.ltex.addToDictionary",
            "command": {"title": "Add", "command": "_ltex.addToDictionary", "arguments": []}});
        assert_eq!(action_fix_suggestion(&a, "u", "abc", &(0..1),
            |k| k == "quickfix.ltex.acceptSuggestions"), None);
    }

    #[test]
    fn vale_changes_shape_still_maps_and_collect_attaches_all_candidates() {
        // Probe §2: bare "quickfix" + edit.changes[uri]; 5 candidates for one diagnostic.
        let text = "abc mispeling xyz";
        let anchor = 4..13usize;
        let mk = |t: &str| serde_json::json!({"title": format!("Replace with '{t}'"),
            "kind": "quickfix",
            "edit": {"changes": {"file:///t.md": [{"newText": t,
                "range": {"start": {"line": 0, "character": 4},
                          "end": {"line": 0, "character": 13}}}]}}});
        let actions: Vec<serde_json::Value> =
            ["misspelling", "dispelling", "misdealing"].iter().map(|t| mk(t)).collect();
        let got = collect_fix_suggestions(&actions, "file:///t.md", text, &anchor,
            |k| k == "quickfix");
        assert_eq!(got, vec![
            Suggestion::ReplaceWith("misspelling".into()),
            Suggestion::ReplaceWith("dispelling".into()),
            Suggestion::ReplaceWith("misdealing".into()),
        ], "ALL matching actions attach, response order kept — the `break` is gone");
    }

    #[test]
    fn collect_dedupes_identical_suggestions_and_keeps_range_equality_gate() {
        let text = "abc mispeling xyz";
        let anchor = 4..13usize;
        let same = serde_json::json!({"kind": "quickfix",
            "edit": {"changes": {"u": [{"newText": "misspelling",
                "range": {"start": {"line": 0, "character": 4},
                          "end": {"line": 0, "character": 13}}}]}}});
        let elsewhere = serde_json::json!({"kind": "quickfix",
            "edit": {"changes": {"u": [{"newText": "zzz",
                "range": {"start": {"line": 0, "character": 0},
                          "end": {"line": 0, "character": 3}}}]}}});
        let got = collect_fix_suggestions(
            &[same.clone(), same, elsewhere], "u", text, &anchor, |k| k == "quickfix");
        assert_eq!(got, vec![Suggestion::ReplaceWith("misspelling".into())],
            "duplicates collapse; an edit not targeting the ANCHOR range never attaches \
             (the apply-safety invariant: server ranges are a matching key only)");
    }

    #[test]
    fn engine_is_fix_kind_tables_match_the_probes() {
        use crate::lsp_client::LspEngine;
        assert!(crate::harper_ls::HarperEngine::is_fix_kind("quickfix"));
        assert!(!crate::harper_ls::HarperEngine::is_fix_kind("quickfix.ltex.acceptSuggestions"));
        assert!(crate::ltex_ls::LtexEngine::is_fix_kind("quickfix.ltex.acceptSuggestions"));
        assert!(!crate::ltex_ls::LtexEngine::is_fix_kind("quickfix.ltex.addToDictionary"));
        assert!(!crate::ltex_ls::LtexEngine::is_fix_kind("quickfix"));
        assert!(crate::vale_ls::ValeEngine::is_fix_kind("quickfix"));
    }
```

  (`Suggestion` is already imported at `lsp_rpc.rs` top level; the test module uses
  `use super::*;`. `LtexEngine`/`ValeEngine` are `pub(crate)` — reachable from this module.)

- [ ] **Step 2: Run to verify the reds**

Run: `cargo test -p wordcartel lsp_rpc:: 2>&1 | tail -6`
Expected: FAIL to COMPILE (`action_fix_suggestion`, `collect_fix_suggestions`, `is_fix_kind`
undefined).

- [ ] **Step 3: Implement**

In `lsp_client.rs`, add to the `LspEngine` trait (beside `classify`):

```rust
    /// E11 §4: is this CodeAction `kind` a FIX this engine delivers as an edit?
    /// Probe-grounded per engine — command-only kinds are excluded by knowledge, not luck.
    fn is_fix_kind(kind: &str) -> bool;
```

Engine impls (one line each, in the existing `impl LspEngine for …` blocks):

```rust
    // harper_ls.rs::HarperEngine — probe-verified bare kind + edit.changes:
    fn is_fix_kind(kind: &str) -> bool { kind == "quickfix" }
    // ltex_ls.rs::LtexEngine — ONLY acceptSuggestions carries an edit (probe Q2):
    fn is_fix_kind(kind: &str) -> bool { kind == "quickfix.ltex.acceptSuggestions" }
    // vale_ls.rs::ValeEngine — probe §2:
    fn is_fix_kind(kind: &str) -> bool { kind == "quickfix" }
```

The `#[cfg(test)] TestEngine` in `lsp_client.rs::tests` also needs the fn (compiler-forced):
`fn is_fix_kind(kind: &str) -> bool { kind == "quickfix" }`.

In `lsp_rpc.rs`, below `quickfix_suggestion` (which stays untouched this task):

```rust
/// E11 §4: map ONE CodeAction to a `Suggestion` targeting `anchor`, accepting fix kinds via
/// the ENGINE's table (`accept_kind` — a fn so this stays engine-agnostic plumbing) and BOTH
/// edit shapes: `edit.changes[uri]` (harper/vale, probe-verified) and
/// `edit.documentChanges[]` (ltex, probe-verified exclusive). The action's edit range is a
/// MATCHING KEY against `anchor` and is then discarded — `Suggestion` carries text only
/// (the apply-safety invariant, spec §3.3).
#[allow(dead_code)] // E11 T2 scaffold — consumed by T4's on_fix_response; allow removed there
pub(crate) fn action_fix_suggestion(
    action: &serde_json::Value, our_uri: &str, doc_text: &str,
    anchor: &std::ops::Range<usize>, accept_kind: impl Fn(&str) -> bool,
) -> Option<Suggestion> {
    let kind = action.get("kind").and_then(|k| k.as_str())?;
    if !accept_kind(kind) { return None; }
    let edit = action.get("edit")?;
    // Shape 1: edit.changes[uri] = [TextEdit].
    if let Some(edits) = edit.get("changes").and_then(|c| c.as_object())
        .and_then(|c| c.get(our_uri)).and_then(|e| e.as_array())
    {
        return edits_to_suggestion(edits, doc_text, anchor);
    }
    // Shape 2: edit.documentChanges[] = [{textDocument{uri}, edits: [TextEdit]}].
    if let Some(dcs) = edit.get("documentChanges").and_then(|d| d.as_array()) {
        for dc in dcs {
            if dc.get("textDocument").and_then(|t| t.get("uri")).and_then(|u| u.as_str())
                != Some(our_uri) { continue; }
            if let Some(edits) = dc.get("edits").and_then(|e| e.as_array()) {
                if let Some(s) = edits_to_suggestion(edits, doc_text, anchor) { return Some(s); }
            }
        }
    }
    None
}

/// The shared TextEdit→Suggestion rules (extracted verbatim from `quickfix_suggestion`'s
/// loop): an edit range equal to `anchor` ⇒ ReplaceWith/Remove; an empty range at
/// `anchor.end` ⇒ InsertAfter; anything else ⇒ no match.
fn edits_to_suggestion(edits: &[serde_json::Value], doc_text: &str,
    anchor: &std::ops::Range<usize>) -> Option<Suggestion> {
    for te in edits {
        let new_text = te.get("newText")?.as_str()?.to_string();
        let r = te.get("range")?;
        let s = (r["start"]["line"].as_u64()? as u32, r["start"]["character"].as_u64()? as u32);
        let e = (r["end"]["line"].as_u64()? as u32, r["end"]["character"].as_u64()? as u32);
        let er = lsp_range_to_bytes(doc_text, s, e)?;
        if er == *anchor {
            return Some(if new_text.is_empty() { Suggestion::Remove }
                        else { Suggestion::ReplaceWith(new_text) });
        }
        if er.is_empty() && er.start == anchor.end {
            return Some(Suggestion::InsertAfter(new_text));
        }
    }
    None
}

/// E11 §4: ALL matching actions for `anchor`, response order, deduped (the shipped `break`
/// capped attachment at one suggestion per diagnostic — multi-candidate is real on both new
/// engines: vale 5-for-one, ltex 2-for-`recieve`, both probe-verified).
#[allow(dead_code)] // E11 T2 scaffold — consumed by T4's on_fix_response; allow removed there
pub(crate) fn collect_fix_suggestions(
    actions: &[serde_json::Value], our_uri: &str, doc_text: &str,
    anchor: &std::ops::Range<usize>, accept_kind: impl Fn(&str) -> bool,
) -> Vec<Suggestion> {
    let mut out: Vec<Suggestion> = Vec::new();
    for a in actions {
        if let Some(s) = action_fix_suggestion(a, our_uri, doc_text, anchor, &accept_kind) {
            if !out.contains(&s) { out.push(s); }
        }
    }
    out
}
```

(`edits_to_suggestion` is private and immediately used by `action_fix_suggestion` — no allow
needed. `accept_kind` is `impl Fn` so T4 can pass `E::is_fix_kind`.)

- [ ] **Step 4: Green + gate + commit**

Run: `cargo test --workspace 2>&1 | tail -3 && cargo clippy --workspace --all-targets 2>&1 | tail -2`
Expected: green / clean (the shipped pipeline is untouched; new fns are allow-scaffolded).

```bash
git add wordcartel/src/lsp_rpc.rs wordcartel/src/lsp_client.rs wordcartel/src/harper_ls.rs \
    wordcartel/src/ltex_ls.rs wordcartel/src/vale_ls.rs
git commit -m "feat: engine-parameterized fix mapping — both edit shapes, attach-all (E11 T2)"
```

---

### Task 3: Delete the parking — publish emits immediately (TDD via rewrites)

**Files:**
- Modify: `wordcartel/src/lsp_client.rs`, `wordcartel/src/harper_ls.rs` (tests),
  `wordcartel/src/lsp_rpc.rs` (delete `quickfix_suggestion` + its tests)
- Test: the REWRITTEN pinned tests are this task's tests (behavior-change TDD: rewrite the
  expectation first, watch it fail against the old code, then change the code)

**Interfaces (produces):** `on_publish` emits `Msg::DiagnosticsDone` immediately on every
attributed publish (suggestions always empty); `DocState.last_raw: Option<(u64, Vec<Value>)>`
exists but is NEVER STORED here (spec round-4 Minor 5: T3 adds the field + clearing sites only;
ALL storage is T4's await-attribution — an interim ambient-version store would recreate the
round-3 Critical).

- [ ] **Step 1: Rewrite the expectation tests FIRST** (in `harper_ls.rs::tests`; run them to see
  them fail against the shipped parking behavior):

```rust
    #[test]
    fn nonempty_publish_emits_converted_immediately_no_codeaction_roundtrip() {
        // E11 §3.1: the parking is GONE — paint no longer waits on a fix round trip.
        let mut st = running(true);
        st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0), version: 5, path: None,
            text: "teh".into() }), 0);
        let out = st.on_inbound(Inbound::Server(json!({"jsonrpc":"2.0",
            "method":"textDocument/publishDiagnostics",
            "params":{"uri":"untitled:wcartel-0-1","diagnostics":[
                {"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":3}},
                 "message":"spelling","code":"SpellCheck"}]}})), 0);
        assert!(sends(&out).is_empty(), "no codeAction request is ever sent from a publish");
        let done = diag_dones(&out);
        assert_eq!(done.len(), 1);
        assert_eq!((done[0].0, done[0].1), (BufferId(0), 5));
        assert_eq!(done[0].2.len(), 1, "converted diagnostic emitted immediately");
        assert!(done[0].2[0].suggestions.is_empty(), "suggestions are on-demand (E11 §3)");
    }
```

  Rewrite `grammar_gate_drops_grammar_diagnostics_when_disabled` (the GATE survives; its
  assembly assertions do not):

```rust
    #[test]
    fn grammar_gate_drops_grammar_diagnostics_when_disabled() {
        let mut st = running(false); // grammar off
        st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0), version: 1, path: None,
            text: "teh cat".into() }), 0);
        let out = st.on_inbound(Inbound::Server(json!({"jsonrpc":"2.0",
            "method":"textDocument/publishDiagnostics",
            "params":{"uri":"untitled:wcartel-0-1","diagnostics":[
                {"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":3}},
                 "message":"spelling","code":"SpellCheck"},
                {"range":{"start":{"line":0,"character":4},"end":{"line":0,"character":7}},
                 "message":"style","code":"LongSentences"}]}})), 0);
        let done = diag_dones(&out);
        assert_eq!(done[0].2.len(), 1, "grammar-classified diagnostic dropped by the client gate");
        assert_eq!(done[0].2[0].kind, DiagnosticKind::Spelling);
    }
```

  Rewrite `flush_outstanding_covers_all_three_tracks_and_is_idempotent` → rename to
  `flush_outstanding_covers_awaiting_and_queued_and_is_idempotent` and drop the buffer-1
  assembling arrange (the third track returns in T4 as `pending_fix`; this task's version
  covers `awaiting_publish` + `queued` only — same assertions minus the `(BufferId(1), 2, …)`
  expectation and its arrange lines).

- [ ] **Step 2: DELETE the tests that pin the removed machinery** (each encodes a mechanism
  that no longer exists; their concerns are re-pinned in T4's slot tests):
  `nonempty_publish_then_codeaction_attaches_replace_with_and_drops_command_only`,
  `assembly_superseded_generation_is_discarded_not_emitted_with_new_ranges`,
  `stale_codeaction_response_does_not_consume_the_newer_assembly`,
  `codeaction_watchdog_emits_converted_suggestionless`,
  `assembly_result_then_eof_does_not_re_emit_empty_for_the_same_version`,
  `codeaction_watchdog_then_eof_no_duplicate_terminal`, and the `publish_teh` helper (its only
  consumers are the above). In `lsp_rpc.rs`: delete `quickfix_suggestion` and its `quickfix_*`
  tests (superseded by T2's mapping + tests). KEEP: `publish_watchdog_emits_empty_after_deadline`,
  `publish_watchdog_then_eof_no_duplicate_terminal`, `empty_publish_emits_terminal_immediately…`,
  `close_emits_terminal_before_removing_state`, `reload_recover_race_old_generation_publish_dropped`
  (all still-true pipeline behavior). In `lsp_client.rs::tests`: `after_first_publish…`/warm
  tests stay (publish semantics unchanged for the watchdog itself).

- [ ] **Step 3: Implement the deletion** in `lsp_client.rs`:
  1. Remove `struct Assembly`, the `assembling: HashMap<BufferId, Assembly>` field (and its
     `new()` init), `PendingKind::CodeAction { .. }`, `raw_envelope`, `codeaction_request`,
     `on_codeaction_response`, and the assembling arm of `on_deadline` (the `expired_asm`
     block). **AND the now-orphaned timeout constant surface (plan-gate round-2 finding 1):**
     the `LspEngine::CODEACTION_TIMEOUT_MS` trait const, its FOUR impls (`HarperEngine`,
     `LtexEngine`, `ValeEngine`, and the `#[cfg(test)] TestEngine` in `lsp_client.rs::tests`),
     and harper's module-level `const CODEACTION_TIMEOUT_MS` (`harper_ls.rs` — its only other
     consumers are the tests this task deletes). T4's `TestEngineAllRaws` copies TestEngine's
     POST-T3 impl, so it never carries the const either.
  2. `next_deadline()` drops the `assembling` chain (keeps `awaiting_publish`).
  3. `on_publish`: after `convert_diagnostics` and the await retirement, ALWAYS
     `return vec![Action::Emit(Msg::DiagnosticsDone { buffer_id, version: tagged,
     source: E::SOURCE, diagnostics: converted })];` — the empty/non-empty branch and the
     codeAction send are gone.
  4. `on_close`/`flush_outstanding`/`on_server_gone`: remove their `assembling` references
     (`on_close`'s `.or_else(|| self.assembling…)`, `flush_outstanding`'s assembling drain).
  5. `DocState` gains `last_raw: Option<(u64, Vec<Value>)>` (init `None` in the `on_change`
     reopen literal — the sole construction site; didChange mutates in place); `on_close`
     clears it implicitly by removing the DocState. NO other site touches it this task, so the
     crate-private never-read field would trip `dead_code` against the warning-free gate
     (plan-gate finding 1) — it carries
     `#[allow(dead_code)] // E11 T3 bridge — first read lands with T4's attribution; allow removed there`
     (removal is on T4's Step 3 checklist, item 7).
  6. Remove the two `#[allow(dead_code)]` scaffold attributes? NO — T2's mapping fns are still
     unconsumed (T4 consumes). They stay.

- [ ] **Step 4: Green + gate + commit** (expected: full workspace green — the e2e/overlay tests
  never asserted suggestion presence from the parked path; `diag_overlay` renders whatever
  `anchor.suggestions` holds, now empty until T5+)

```bash
git add wordcartel/src/lsp_client.rs wordcartel/src/harper_ls.rs wordcartel/src/lsp_rpc.rs
git commit -m "feat: unpark diagnostics — publish emits immediately, fixes go on-demand (E11 T3)

Deliberately rewrites behavior encoded in harper's pinned tests: the E10 pin protected a
behavior-preserving extraction; E11 changes the pipeline (spec §3.7 carries the inventory).
Deleted: Assembly/watchdog/batched-codeAction machinery + quickfix_suggestion."
```

---

### Task 4: The `pending_fix` slot — request/attribution/deadline/flush (TDD)

**Files:**
- Modify: `wordcartel/src/lsp_client.rs`, `wordcartel/src/app.rs` (Step 3.0: the `Msg` variant
  + Debug arm + inert reduce arm), `wordcartel/src/lsp_rpc.rs` (remove the two T2 scaffold
  allows), `wordcartel/src/ltex_ls.rs` (the outcome-B `FIX_CONTEXT_ALL_RAWS` flip, if T1
  ruled B)
- Test: `lsp_client.rs::tests` (TestEngine) + `harper_ls.rs::tests` (the flush three-track
  restoration)

**Interfaces (produces):** `Cmd::RequestFixes { token, buffer_id, version, range, code, message }`;
`PendingFix`; `PendingKind::FixRequest { token, buffer_id, generation, version, range }`;
`FIX_REQUEST_TIMEOUT_MS`; emission of `Msg::DiagFixesReady` (T5 adds the variant FIRST — see
the ordering note). Consumes T1's outcome + T2's `collect_fix_suggestions`/`E::is_fix_kind`
(remove T2's two `#[allow(dead_code)]` scaffolds in this task).

**ORDERING NOTE (greenness):** `Msg::DiagFixesReady` must exist before this task's `Action::Emit`
sites compile. To keep every commit green WITHOUT reaching into T5's seams, this task's Step 3.0
adds ONLY the `Msg` variant + its two compiler-forced arms (the manual `Debug` impl and an
inert `reduce_dispatch` arm `Msg::DiagFixesReady { .. } => {}` — replaced by T5's real arm).
Migration census for the variant: `app.rs` enum `Msg`; `app.rs` manual `impl Debug for Msg`
(exhaustive — add a `debug_struct` arm mirroring `DiagnosticsDone`'s); `app.rs::reduce_dispatch`
match (exhaustive, no catch-all — the inert arm). No other site matches `Msg` exhaustively
(verified: overlays/intercepts match specific variants with pass-through).

- [ ] **Step 1: Write the failing state-machine tests** (append to `lsp_client.rs::tests`;
  helpers `running()`/`change()`/`sends()`/`diag_dones()` exist from E10; add
  `fix_readies(acts) -> Vec<(u64, Vec<Suggestion>)>` extracting
  `Action::Emit(Msg::DiagFixesReady { token, suggestions, .. })`):

```rust
    fn fix_readies(acts: &[Action]) -> Vec<(u64, Vec<wordcartel_core::diagnostics::Suggestion>)> {
        acts.iter().filter_map(|a| match a {
            Action::Emit(Msg::DiagFixesReady { token, suggestions, .. }) =>
                Some((*token, suggestions.clone())),
            _ => None,
        }).collect()
    }

    fn publish_one(st: &mut ClientState<TestEngine>, uri: &str, at: u64) -> Vec<Action> {
        st.on_inbound(Inbound::Server(json!({"jsonrpc":"2.0",
            "method":"textDocument/publishDiagnostics",
            "params":{"uri":uri,"diagnostics":[
                {"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},
                 "message":"m","code":"C1"}]}})), at)
    }

    fn req(st: &mut ClientState<TestEngine>, token: u64, version: u64, at: u64) -> Vec<Action> {
        st.on_inbound(Inbound::Cmd(Cmd::RequestFixes { token, buffer_id: BufferId(0),
            version, range: 0..1, code: Some("C1".into()), message: "m".into() }), at)
    }

    #[test]
    fn fix_request_sends_when_running_open_and_raw_attributed() {
        let mut st = running();
        change(&mut st, 0, 1, 0);
        publish_one(&mut st, "untitled:wcartel-0-1", 5); // answers the await → last_raw tagged v1
        let out = req(&mut st, 7, 1, 10);
        let ca = sends(&out).into_iter().find(|v| v["method"] == "textDocument/codeAction")
            .expect("per-diagnostic codeAction sent");
        assert_eq!(ca["params"]["context"]["diagnostics"].as_array().unwrap().len(), 1,
            "the triple-matched RAW diagnostic is echoed verbatim (T1 outcome A shape)");
    }

    #[test]
    fn fix_response_attaches_all_and_clears_slot() {
        let mut st = running();
        change(&mut st, 0, 1, 0);
        publish_one(&mut st, "untitled:wcartel-0-1", 5);
        let out = req(&mut st, 7, 1, 10);
        let id = sends(&out).iter().find(|v| v["method"] == "textDocument/codeAction")
            .and_then(|v| v["id"].as_u64()).unwrap();
        let resp = st.on_inbound(Inbound::Server(json!({"jsonrpc":"2.0","id":id,"result":[
            {"kind":"quickfix","edit":{"changes":{"untitled:wcartel-0-1":[
                {"newText":"X","range":{"start":{"line":0,"character":0},
                                        "end":{"line":0,"character":1}}}]}}},
            {"kind":"quickfix","edit":{"changes":{"untitled:wcartel-0-1":[
                {"newText":"Y","range":{"start":{"line":0,"character":0},
                                        "end":{"line":0,"character":1}}}]}}}]})), 20);
        assert_eq!(fix_readies(&resp), vec![(7, vec![
            wordcartel_core::diagnostics::Suggestion::ReplaceWith("X".into()),
            wordcartel_core::diagnostics::Suggestion::ReplaceWith("Y".into())])],
            "attach-all through collect_fix_suggestions; token rides through");
        assert!(st.pending_fix.is_none());
    }

    #[test]
    fn fix_deadline_fires_even_while_initializing() {
        // Round-1 finding-2 regression: the slot + deadline are live in EVERY phase.
        let mut st = ClientState::<TestEngine>::new(cfg());
        st.on_spawned(0); // Initializing
        let out = req(&mut st, 9, 1, 0);
        assert!(out.is_empty(), "slot materialized, nothing sent, nothing queued");
        assert_eq!(st.next_deadline(), Some(FIX_REQUEST_TIMEOUT_MS),
            "the 10s leash is visible to the pump during warm");
        let expired = st.on_deadline(FIX_REQUEST_TIMEOUT_MS);
        assert_eq!(fix_readies(&expired), vec![(9, vec![])], "honest empty terminal at 10s");
    }

    #[test]
    fn stale_raw_after_didchange_never_sends_and_change_invalidates() {
        // Round-2 finding-2 + round-3: one-snapshot condition + change-invalidation.
        let mut st = running();
        change(&mut st, 0, 1, 0);
        publish_one(&mut st, "untitled:wcartel-0-1", 5); // last_raw tagged v1
        change(&mut st, 0, 2, 10); // didChange advances text/our_version to v2
        let out = req(&mut st, 3, 2, 15); // request against v2; raws are v1
        assert!(sends(&out).iter().all(|v| v["method"] != "textDocument/codeAction"),
            "no send against mismatched snapshots");
        // A further change PAST a pending slot resolves it empty immediately.
        let inv = st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0), version: 3,
            path: None, text: "zz".into() }), 20);
        assert_eq!(fix_readies(&inv), vec![(3u64, vec![])].into_iter()
            .map(|(_, s)| (3u64, s)).collect::<Vec<_>>(),
            "change-invalidation emits the token terminal at once");
    }

    #[test]
    fn replacement_terminates_displaced_request_and_deregisters_its_id() {
        // Round-2 finding-3: exactly-once holds for the DISPLACED request too.
        let mut st = running();
        change(&mut st, 0, 1, 0);
        publish_one(&mut st, "untitled:wcartel-0-1", 5);
        let a = req(&mut st, 1, 1, 10);
        let id_a = sends(&a).iter().find(|v| v["method"] == "textDocument/codeAction")
            .and_then(|v| v["id"].as_u64()).unwrap();
        let b = req(&mut st, 2, 1, 11);
        assert_eq!(fix_readies(&b), vec![(1, vec![])], "displaced token 1 terminated at replacement");
        let late = st.on_inbound(Inbound::Server(json!({"jsonrpc":"2.0","id":id_a,
            "result":[]})), 12);
        assert!(fix_readies(&late).is_empty(), "late response to the displaced id → unknown-id arm");
    }

    #[test]
    fn timeout_retirement_cycles_do_not_leak_and_late_publishes_drop() {
        // Round-3 Critical-1 + round-4 Important-2, pinned over N CYCLES with per-cycle
        // mapping/count assertions (plan-gate finding 5: one cycle without uri_owner
        // assertions cannot catch the leak).
        let mut st = running();
        let mut base = 0u64;
        for cycle in 1..=3u64 {
            let version = cycle;
            st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0), version,
                path: None, text: "x".into() }), base);
            let expired = st.on_deadline(base + TestEngine::FIRST_CHECK_TIMEOUT_MS.unwrap());
            assert!(!diag_dones(&expired).is_empty(), "cycle {cycle}: check terminal emitted");
            let closes = sends(&expired).iter()
                .filter(|v| v["method"] == "textDocument/didClose").count();
            assert_eq!(closes, 1, "cycle {cycle}: exactly one didClose per retirement");
            assert!(st.uri_owner.is_empty(),
                "cycle {cycle}: the retired mapping is REMOVED — uri_owner holds only live URIs");
            let late = publish_one(&mut st,
                &format!("untitled:wcartel-0-{cycle}"), base + 999);
            assert!(late.is_empty(), "cycle {cycle}: late publish to the retired uri drops whole");
            base += 1_000_000;
        }
        // The next check reopens under generation 4 (three retirements consumed 1..=3).
        let reopen = st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0), version: 9,
            path: None, text: "b".into() }), base);
        assert!(sends(&reopen).iter().any(|v| v["method"] == "textDocument/didOpen"
            && v["params"]["textDocument"]["uri"] == "untitled:wcartel-0-4"));
        assert_eq!(st.uri_owner.len(), 1, "exactly the one live URI after N cycles");
    }

    #[test]
    fn retirement_then_buffer_close_sends_exactly_one_didclose() {
        // Round-5 Important-2: the wire frame is gated on d.open.
        let mut st = running();
        change(&mut st, 0, 1, 0);
        let retired = st.on_deadline(TestEngine::FIRST_CHECK_TIMEOUT_MS.unwrap());
        let n1 = sends(&retired).iter().filter(|v| v["method"] == "textDocument/didClose").count();
        let closed = st.on_inbound(Inbound::Cmd(Cmd::Close { buffer_id: BufferId(0) }), 10);
        let n2 = sends(&closed).iter().filter(|v| v["method"] == "textDocument/didClose").count();
        assert_eq!(n1 + n2, 1, "exactly ONE didClose across retirement + close");
    }

    #[test]
    fn close_resolves_pending_fix_and_a_late_response_is_silent() {
        // Round-3 Important-2: the document-close leg of exactly-once.
        let mut st = running();
        change(&mut st, 0, 1, 0);
        publish_one(&mut st, "untitled:wcartel-0-1", 5);
        let out = req(&mut st, 4, 1, 10);
        let id = sends(&out).iter().find(|v| v["method"] == "textDocument/codeAction")
            .and_then(|v| v["id"].as_u64()).unwrap();
        let closed = st.on_inbound(Inbound::Cmd(Cmd::Close { buffer_id: BufferId(0) }), 20);
        assert!(fix_readies(&closed).contains(&(4, vec![])), "close emits the token terminal");
        let late = st.on_inbound(Inbound::Server(json!({"jsonrpc":"2.0","id":id,"result":[]})), 30);
        assert!(fix_readies(&late).is_empty(), "de-registered id → silence, no double terminal");
    }

    #[test]
    fn suspend_resume_holds_the_slot_and_sends_after_replay_and_fresh_publish() {
        // Spec §3.5's full ordering: suspend → change → unpark → slot persists → replay →
        // publish re-tags → send. (TestEngine::SUSPENDABLE = true.)
        let mut st = running();
        st.on_inbound(Inbound::Cmd(Cmd::Suspend), 0);
        st.on_inbound(Inbound::ServerEof, 1); // expected-EOF drain (E10)
        let resume = st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0), version: 1,
            path: None, text: "x".into() }), 2);
        assert!(resume.iter().any(|a| matches!(a, Action::Unpark)));
        let held = req(&mut st, 6, 1, 3);
        assert!(held.is_empty(), "slot held — no send, no emissions yet");
        assert!(st.queued.iter().all(|c| !matches!(c, Cmd::RequestFixes { .. })),
            "RequestFixes is NEVER in `queued` — the slot IS its queue (finding-2 class)");
        let spawn = st.on_spawned(4);
        let id = sends(&spawn)[0]["id"].as_u64().unwrap();
        let replay = st.on_inbound(Inbound::Server(json!({"jsonrpc":"2.0","id":id,
            "result":{"capabilities":{}}})), 5);
        assert!(sends(&replay).iter().any(|v| v["method"] == "textDocument/didOpen"));
        // The fresh publish (new generation uri -0-2) re-tags last_raw → the slot sends.
        let pubd = publish_one(&mut st, "untitled:wcartel-0-2", 6);
        assert!(sends(&pubd).iter().any(|v| v["method"] == "textDocument/codeAction"),
            "attempt site (c): send fires after the awaited publish re-tags last_raw");
    }

    #[test]
    fn unsolicited_publish_does_not_update_last_raw() {
        // §3.3 await-attribution, pinned with DISTINGUISHABLE arrays (plan-gate finding 5):
        // the solicited publish carries code C1; the unsolicited republish carries ONLY a
        // different diagnostic (code C9). If the unsolicited publish replaced last_raw, a
        // C1 request could no longer triple-match (empty terminal) and a C9 request COULD —
        // assert the exact opposite on both.
        let mut st = running();
        change(&mut st, 0, 1, 0);
        publish_one(&mut st, "untitled:wcartel-0-1", 5); // solicited: code C1 (answers await)
        let _ = st.on_inbound(Inbound::Server(json!({"jsonrpc":"2.0",
            "method":"textDocument/publishDiagnostics",
            "params":{"uri":"untitled:wcartel-0-1","diagnostics":[
                {"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},
                 "message":"other","code":"C9"}]}})), 6); // unsolicited: NO await live
        let c1 = req(&mut st, 8, 1, 10);
        assert!(sends(&c1).iter().any(|v| v["method"] == "textDocument/codeAction"),
            "C1 still triple-matches — the SOLICITED raws were retained");
        let c9 = st.on_inbound(Inbound::Cmd(Cmd::RequestFixes { token: 9,
            buffer_id: BufferId(0), version: 1, range: 0..1,
            code: Some("C9".into()), message: "other".into() }), 11);
        assert_eq!(fix_readies(&c9), vec![(9, vec![])],
            "C9 does NOT match (empty terminal) — the unsolicited raws were never stored");
    }

    #[test]
    fn no_triple_match_resolves_empty_immediately() {
        // §3.3: version-matched raws present but nothing matches the request identity.
        let mut st = running();
        change(&mut st, 0, 1, 0);
        publish_one(&mut st, "untitled:wcartel-0-1", 5); // raws hold C1 only
        let out = st.on_inbound(Inbound::Cmd(Cmd::RequestFixes { token: 4,
            buffer_id: BufferId(0), version: 1, range: 0..1,
            code: Some("NOPE".into()), message: "m".into() }), 10);
        assert_eq!(fix_readies(&out), vec![(4, vec![])], "no-match leg emits the empty terminal");
        assert!(sends(&out).iter().all(|v| v["method"] != "textDocument/codeAction"));
    }

    #[test]
    fn server_gone_flushes_a_pending_fix() {
        // §3.4 exactly-once, server-gone leg: the slot joins flush_outstanding's coverage.
        let mut st = running();
        st.on_inbound(Inbound::Cmd(Cmd::RequestFixes { token: 5, buffer_id: BufferId(0),
            version: 1, range: 0..1, code: None, message: "m".into() }), 0); // held (no raws)
        let gone = st.on_inbound(Inbound::ServerEof, 1);
        assert!(fix_readies(&gone).contains(&(5, vec![])),
            "on_server_gone's flush emits the held slot's token terminal");
        assert!(st.pending_fix.is_none());
    }

    #[test]
    fn flush_guard_drains_unread_request_fixes_from_the_channel() {
        // §3.2 FlushGuard extension — mirror the shipped flush_guard_drop channel pattern.
        let (msg_tx, msg_rx) = std::sync::mpsc::channel::<Msg>();
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<Inbound>();
        let state = ClientState::<TestEngine>::new(cfg());
        cmd_tx.send(Inbound::Cmd(Cmd::RequestFixes { token: 6, buffer_id: BufferId(0),
            version: 1, range: 0..1, code: None, message: "m".into() })).unwrap();
        drop(FlushGuard { state, cmd_rx, msg_tx });
        let got: Vec<u64> = std::iter::from_fn(|| msg_rx.try_recv().ok())
            .filter_map(|m| if let Msg::DiagFixesReady { token, suggestions, .. } = m {
                assert!(suggestions.is_empty()); Some(token) } else { None }).collect();
        assert_eq!(got, vec![6], "an UNREAD RequestFixes still gets its empty terminal on drop");
    }
```

  (`FlushGuard` here is the generic `lsp_client::FlushGuard<TestEngine>` — construct it by
  struct literal exactly as the shipped harper `flush_guard_*` tests do.)

```rust
```

  And in `harper_ls.rs::tests`, restore the three-track flush (rename back), full body:

```rust
    #[test]
    fn flush_outstanding_covers_all_three_tracks_and_is_idempotent() {
        let mut st = running(true);
        // Track 1 — awaiting (buffer 0).
        st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0), version: 1, path: None,
            text: "a".into() }), 0);
        // Track 2 — queued (buffer 2): drop back to Initializing so a change queues.
        st.phase = Phase::Initializing;
        st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(2), version: 3, path: None,
            text: "q".into() }), 0);
        // Track 3 — the E11 pending_fix slot (held: Initializing, no raws).
        st.on_inbound(Inbound::Cmd(Cmd::RequestFixes { token: 11, buffer_id: BufferId(0),
            version: 1, range: 0..1, code: None, message: "m".into() }), 0);
        let acts = st.flush_outstanding();
        let mut done: Vec<(BufferId, u64)> = acts.iter().filter_map(|a| match a {
            Action::Emit(Msg::DiagnosticsDone { buffer_id, version, .. }) =>
                Some((*buffer_id, *version)), _ => None }).collect();
        done.sort_by_key(|(b, _)| b.0);
        assert_eq!(done, vec![(BufferId(0), 1), (BufferId(2), 3)], "checks flushed");
        assert!(acts.iter().any(|a| matches!(a,
            Action::Emit(Msg::DiagFixesReady { token: 11, .. }))), "the slot's token terminal");
        assert!(st.flush_outstanding().is_empty(), "idempotent — a second flush emits nothing");
    }
```

- [ ] **Step 2: Run to verify the reds** — compile FAIL (`Cmd::RequestFixes`,
  `Msg::DiagFixesReady`, `pending_fix`, `FIX_REQUEST_TIMEOUT_MS` undefined).

- [ ] **Step 3.0: The `Msg` variant + compiler-forced arms** (`app.rs`):

```rust
    /// E11 §3.4: on-demand fix results for ONE overlay request. `token` is the delivery key
    /// (minted per request); the identity fields ride for debug asserts, never correlation.
    DiagFixesReady {
        token: u64,
        buffer_id: crate::editor::BufferId,
        version: u64,
        source: wordcartel_core::diagnostics::DiagSource,
        range: std::ops::Range<usize>,
        suggestions: Vec<wordcartel_core::diagnostics::Suggestion>,
    },
```

  Debug impl arm (beside `DiagnosticsDone`'s, same `debug_struct` style, fields
  `token`/`buffer_id`/`version`/`source` + `suggestions` len). `reduce_dispatch` arm (inert
  this task, replaced in T5): `Msg::DiagFixesReady { .. } => {}` with a
  `// E11 T4 placeholder — T5 installs the delivery arm` comment.

- [ ] **Step 3: Implement the machine** (`lsp_client.rs`), per spec §3.3 verbatim:
  1. `const FIX_REQUEST_TIMEOUT_MS: u64 = 10_000;` (module consts, doc-commented per spec).
  2. `Cmd` gains `RequestFixes { token: u64, buffer_id: BufferId, version: u64,
     range: std::ops::Range<usize>, code: Option<String>, message: String }`.
  3. `PendingFix` struct + `pending_fix: Option<PendingFix>` field (all `pub(crate)`, init
     `None`); `PendingKind` gains `FixRequest { token: u64, buffer_id: BufferId,
     generation: u64, version: u64, range: std::ops::Range<usize> }`.
  4. `on_inbound`: FIRST-CLASS routing before the queue arm (the `Cmd::Suspend` precedent):
     `Cmd::RequestFixes` in ANY phase → replace the slot (emitting the displaced token's empty
     terminal + removing its `sent_id` entry first), stamp
     `deadline = now + FIX_REQUEST_TIMEOUT_MS`, then `self.try_send_fix(now)` — NEVER queued.
  5. `try_send_fix(&mut self, now) -> Vec<Action>`: the §3.3 send condition — `Running` +
     `DocState.open` + `last_raw` tag == `our_version` == `PendingFix.version` + a
     triple-matching raw (converted-range == range AND code AND message equality). On match,
     materialize the wire request WITHOUT any byte→UTF-16 conversion (plan-gate finding 2: no
     inverse of `lsp_range_to_bytes` exists in the tree, and none is needed): **`params.range`
     is the triple-matched raw diagnostic's own verbatim wire `range` object**
     (`raw["range"].clone()` — the exact positions the server itself published, sidestepping
     multibyte/astral conversion entirely), `params.textDocument.uri` is the current
     `DocState.uri`, and `context.diagnostics` follows the Outcome seam (item 5a). Insert
     `PendingKind::FixRequest`, set `sent_id`. On no-triple-match WITH a version-matched
     `last_raw` present: resolve empty (the fix target no longer exists). Otherwise: hold.
  5a. **The Outcome-A/B seam, fully specified NOW (plan-gate finding 3 — T1 selects between
     two WRITTEN paths, it does not commission a design):** `LspEngine` gains a defaulted
     const:
     ```rust
    /// E11 §2.3/T1: what a per-diagnostic fix request carries in `context.diagnostics` —
    /// `false` (default): the triple-matched raw alone; `true`: EVERY retained raw from the
    /// same publish (the shape ltex demonstrably answers in batch; range stays the matched
    /// raw's own). Flipped per engine by T1's probe outcome.
    const FIX_CONTEXT_ALL_RAWS: bool = false;
     ```
     `try_send_fix` builds `context.diagnostics` as
     `if E::FIX_CONTEXT_ALL_RAWS { last_raw array verbatim } else { vec![matched raw] }`.
     Engine impls: harper/vale — omit (default `false`, vale's per-diagnostic behavior is
     probe-proven 7/7); ltex — **one line set from T1's recorded outcome** (`false` under
     Outcome A; `true` under Outcome B). Both paths are tested regardless of outcome: the
     Outcome-A test below (`context.len() == 1` against `TestEngine`, whose default-`false`
     const makes it the Outcome-A engine regardless of what T1 decides for ltex) plus a second
     `#[cfg(test)] struct TestEngineAllRaws` — a COPY of `TestEngine`'s entire `impl LspEngine`
     block (all consts + the five fns, duplicated verbatim) with the single change
     `const FIX_CONTEXT_ALL_RAWS: bool = true;` — with:
     ```rust
    #[test]
    fn all_raws_engine_echoes_the_full_retained_array_with_the_matched_range() {
        let mut st = ClientState::<TestEngineAllRaws>::new(cfg());
        // running() equivalent inline (the helper is TestEngine-typed): spawn + init response.
        let spawn = st.on_spawned(0);
        let id = sends(&spawn)[0]["id"].as_u64().unwrap();
        st.on_inbound(Inbound::Server(json!({"jsonrpc":"2.0","id":id,
            "result":{"capabilities":{}}})), 0);
        st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0), version: 1,
            path: None, text: "ab".into() }), 0);
        st.on_inbound(Inbound::Server(json!({"jsonrpc":"2.0",
            "method":"textDocument/publishDiagnostics",
            "params":{"uri":"untitled:wcartel-0-1","diagnostics":[
                {"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},
                 "message":"m","code":"C1"},
                {"range":{"start":{"line":0,"character":1},"end":{"line":0,"character":2}},
                 "message":"n","code":"C2"}]}})), 5);
        let out = st.on_inbound(Inbound::Cmd(Cmd::RequestFixes { token: 7,
            buffer_id: BufferId(0), version: 1, range: 0..1,
            code: Some("C1".into()), message: "m".into() }), 10);
        let ca = sends(&out).into_iter().find(|v| v["method"] == "textDocument/codeAction").unwrap();
        assert_eq!(ca["params"]["context"]["diagnostics"].as_array().unwrap().len(), 2,
            "ALL retained raws echoed under the all-raws shape");
        assert_eq!(ca["params"]["range"], json!({"start":{"line":0,"character":0},
            "end":{"line":0,"character":1}}), "range stays the MATCHED raw's verbatim range");
    }
     ```
  6. Attempt sites: end of `on_initialized` (after queue replay) and end of `on_publish`
     (after `last_raw` storage) both append `self.try_send_fix(now)`.
  7. `on_publish`: store `d.last_raw = Some((await_version, raw.clone()))` ONLY when the await
     removal returned `Some(a)` (await-attribution; tag = `a.our_version`).
  8. `on_change`: change-invalidation — before building the didChange/didOpen, if
     `pending_fix` matches this buffer with `version <` the new version, resolve it empty +
     de-register.
  9. `on_deadline`: the publish-watchdog arm ALSO performs retirement (remove the buffer's
     uri from `uri_owner`, push `Send(didClose{uri})`, clear `last_raw`, set `open = false`);
     a new `pending_fix`-deadline arm resolves an expired slot empty. `next_deadline()` chains
     `pending_fix.as_ref().map(|p| p.deadline)`.
  10. `on_close`: gate the didClose `Send` on `d.open`; resolve a matching `pending_fix`
      empty + de-register (before state removal — the terminal-first house pattern).
  11. `on_fix_response(token, buffer_id, generation, version, range, v)` (routed from
      `on_server_response`): stale generation or slot-superseded → `Vec::new()` (already
      terminated); else `collect_fix_suggestions(&v["result"] actions, uri, text, &range,
      E::is_fix_kind)` → emit `DiagFixesReady`; clear slot. REMOVE T2's two
      `#[allow(dead_code)]` scaffolds now that both fns are consumed.
  12. `flush_outstanding`: also drain `pending_fix` (empty token terminal);
      `FlushGuard::drop`'s channel drain: `Inbound::Cmd(Cmd::RequestFixes { token, buffer_id,
      version, range, .. })` → send an empty `DiagFixesReady` (mirror the `Change` arm's
      shape, with `source: E::SOURCE`).

- [ ] **Step 4: Green + gate + commit**

```bash
cargo test --workspace 2>&1 | tail -3 && cargo clippy --workspace --all-targets 2>&1 | tail -2
git add wordcartel/src/lsp_client.rs wordcartel/src/harper_ls.rs wordcartel/src/app.rs \
    wordcartel/src/lsp_rpc.rs wordcartel/src/ltex_ls.rs
# lsp_rpc.rs: the T2 scaffold allows removed here. ltex_ls.rs: staged for the outcome-B
# FIX_CONTEXT_ALL_RAWS flip (a no-op add under outcome A — harmless either way).
git commit -m "feat: on-demand pending_fix slot — attribution, retirement, exactly-once (E11 T4)"
```

---

### Task 5: Provider seam, token, reduce arm, quick_fix fetch (TDD)

**Files:**
- Modify: `wordcartel/src/diag_provider.rs`, `wordcartel/src/lsp_client.rs` (provider impl),
  `wordcartel/src/editor.rs`, `wordcartel/src/diag_overlay.rs` (two fields only),
  `wordcartel/src/search_ui.rs` (the shared `apply_diag_fixes_ready`), `wordcartel/src/app.rs`
  (the real reduce arm), `wordcartel/src/prompts.rs` (the modal-delivery arm — the file that
  stops a sole terminal being swallowed under a prompt), `wordcartel/src/registry.rs`
  (`quick_fix`)
- Test: `diag_provider.rs::tests`, `app.rs::tests` (or `search_ui.rs::tests` beside the diag
  tests — match the neighbors), `registry.rs::tests`

**Interfaces (produces):** `DiagnosticsProvider::request_fixes(token, buffer_id, version, range,
code, message) -> Accepted` (defaulted `No`); `ProviderSet::request_fixes(source, ..) -> Accepted`;
`ProviderCall::RequestFixes { .. }`; `Editor.next_fix_token: u64`;
`DiagOverlay.fix_token: Option<u64>` + `fix_state: FixState` (`enum FixState { Fetching, Done }`
in diag_overlay.rs); the REAL `Msg::DiagFixesReady` reduce arm.

- [ ] **Step 1: Failing tests.**

`diag_provider.rs::tests`:

```rust
    #[test]
    fn recording_provider_records_request_fixes_and_reports_settable_accepted() {
        let rec = RecordingProvider::new().with_source(DiagSource::LTeX);
        let calls = rec.calls_handle();
        let mut set = ProviderSet::default();
        set.install(Box::new(rec), true);
        let a = set.request_fixes(DiagSource::LTeX, 7, BufferId(0), 1, 2..5,
            Some("C".into()), "m".into());
        assert_eq!(a, Accepted::Yes, "recorder default accepts");
        assert!(calls.lock().unwrap().iter().any(|c| matches!(c,
            ProviderCall::RequestFixes { token: 7, .. })));
        assert_eq!(set.request_fixes(DiagSource::Vale, 8, BufferId(0), 1, 0..1, None, "".into()),
            Accepted::No, "unregistered source → No");
    }
```

Reduce-arm tests — **in `app.rs::tests`, on the REAL `reduce` harness** (plan-gate finding 6:
the `search_ui` neighbors call `diag_apply_selected` directly and never exercise
`reduce_dispatch`; the templates to mirror are the SHIPPED `*_via_reduce` tests —
`quick_fix_apply_in_review_arms_via_reduce` and `prompt_held_filterdone_in_review_arms_via_reduce`
— whose arrange builds `Registry::builtins()`, a `KeyTrie`, `InlineExecutor`, `TestClock`, an
mpsc channel, and `crate::test_support::test_fs()` and calls
`crate::app::reduce(msg, &mut e, &reg, &km, &ex, &clk, &tx, &fs)`). Complete literals:

```rust
    fn fetching_overlay_via(e: &mut Editor, token: u64) {
        let d = wordcartel_core::diagnostics::Diagnostic { range: 0..1,
            kind: wordcartel_core::diagnostics::DiagnosticKind::Grammar,
            source: wordcartel_core::diagnostics::DiagSource::LTeX,
            code: Some("C1".into()), href: None, message: "m".into(), suggestions: vec![] };
        e.open_diag(d);
        let ov = e.diag.as_mut().unwrap();
        ov.fix_token = Some(token);
        ov.fix_state = crate::diag_overlay::FixState::Fetching;
    }

    fn fixes_ready(e: &Editor, token: u64) -> Msg {
        Msg::DiagFixesReady { token, buffer_id: e.active().id,
            version: e.active().document.version,
            source: wordcartel_core::diagnostics::DiagSource::LTeX, range: 0..1,
            suggestions: vec![wordcartel_core::diagnostics::Suggestion::ReplaceWith("x".into())] }
    }

    #[test]
    fn fixes_ready_delivers_on_token_match_and_same_version_via_reduce() {
        let mut e = Editor::new_from_text("ab\n", None, (40, 10));
        fetching_overlay_via(&mut e, 7);
        let reg = Registry::builtins();
        let km = cua_keymap(); // the shipped app.rs::tests harness helper (verified)
        let (tx, _rx) = std::sync::mpsc::channel();
        let msg = fixes_ready(&e, 7);
        crate::app::reduce(msg, &mut e, &reg, &km, &crate::jobs::InlineExecutor::default(),
            &crate::test_support::TestClock::new(0), &tx, &crate::test_support::test_fs());
        let ov = e.diag.as_ref().expect("overlay stays open");
        assert_eq!(ov.anchor.suggestions.len(), 1, "suggestions delivered");
        assert_eq!(ov.fix_state, crate::diag_overlay::FixState::Done);
    }

    #[test]
    fn displaced_terminal_does_not_clear_a_reopened_overlays_fetching_state_via_reduce() {
        // Spec round-2 Critical-1: reopen mints the SAME buffer/version/range — only the
        // token discriminates. Token 1's (displaced) terminal must not touch token 2's overlay.
        let mut e = Editor::new_from_text("ab\n", None, (40, 10));
        fetching_overlay_via(&mut e, 1);
        e.diag = None;                    // close…
        fetching_overlay_via(&mut e, 2);  // …and reopen the same diagnostic, new token
        let reg = Registry::builtins();
        let km = cua_keymap(); // the shipped app.rs::tests harness helper (verified)
        let (tx, _rx) = std::sync::mpsc::channel();
        let stale = Msg::DiagFixesReady { token: 1, buffer_id: e.active().id,
            version: e.active().document.version,
            source: wordcartel_core::diagnostics::DiagSource::LTeX, range: 0..1,
            suggestions: vec![] };
        crate::app::reduce(stale, &mut e, &reg, &km, &crate::jobs::InlineExecutor::default(),
            &crate::test_support::TestClock::new(0), &tx, &crate::test_support::test_fs());
        let ov = e.diag.as_ref().expect("overlay untouched");
        assert_eq!(ov.fix_state, crate::diag_overlay::FixState::Fetching,
            "token-1 terminal dropped silently; token-2 fetch still live");
    }

    #[test]
    fn version_mismatched_terminal_is_consumed_and_closes_the_overlay_via_reduce() {
        // Spec round-5 Important-1: a BACKGROUND (non-key) mutation bumps the version while
        // Fetching; the change-invalidation terminal (token-matched, version-mismatched)
        // must CLOSE the overlay with the shipped sticky status — consumed, never dropped.
        let mut e = Editor::new_from_text("ab\n", None, (40, 10));
        fetching_overlay_via(&mut e, 7);
        let opened = e.diag.as_ref().unwrap().opened_version;
        // Background mutation through the edit funnel (no key input; the modal only eats keys).
        let (cs, edit) = crate::commands::build_range_replace(0, 0, "z",
            e.active().document.buffer.len());
        let txn = wordcartel_core::history::Transaction::new(cs);
        let _ = e.apply(txn, edit, wordcartel_core::history::EditKind::Other,
            &crate::test_support::TestClock::new(0));
        assert!(e.active().document.version > opened, "background edit bumped the version");
        let reg = Registry::builtins();
        let km = cua_keymap(); // the shipped app.rs::tests harness helper (verified)
        let (tx, _rx) = std::sync::mpsc::channel();
        let terminal = Msg::DiagFixesReady { token: 7, buffer_id: e.active().id,
            version: opened, // the terminal names the REQUEST's version
            source: wordcartel_core::diagnostics::DiagSource::LTeX, range: 0..1,
            suggestions: vec![] };
        crate::app::reduce(terminal, &mut e, &reg, &km, &crate::jobs::InlineExecutor::default(),
            &crate::test_support::TestClock::new(0), &tx, &crate::test_support::test_fs());
        assert!(e.diag.is_none(), "consumed AND closed — no eternal Fetching");
        let st = e.status().expect("status set");
        assert_eq!(e.status_text(), "document changed; re-open");
        assert_eq!(st.lifetime(), crate::status::StatusLifetime::Sticky);
    }

    #[test]
    fn prompt_intercept_delivers_fixes_ready_under_a_modal() {
        // Plan-gate finding 4: prompts::intercept consumes ALL messages while a prompt is
        // open (`_ => {}`), with explicit forwarding arms for background results — the
        // shipped `intercept_delivers_diag_provider_event_under_a_modal` precedent. Without
        // a DiagFixesReady arm, a prompt raised while a fetch is live would eat the sole
        // terminal (eternal Fetching, the spec rounds-4/5 defect class).
        let mut e = Editor::new_from_text("ab\n", None, (40, 10));
        fetching_overlay_via(&mut e, 7);
        e.open_prompt(crate::prompt::Prompt::close_confirm("f.md", e.active().id));
        // NOTE open_prompt closes overlays (XOR) — re-arm the diag state after, as the race
        // being modeled is a prompt raised by a BACKGROUND route (which does not close_all):
        fetching_overlay_via(&mut e, 7);
        e.prompt = Some(crate::prompt::Prompt::close_confirm("f.md", e.active().id));
        let reg = Registry::builtins();
        let km = cua_keymap(); // the shipped app.rs::tests harness helper (verified)
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::app::reduce(fixes_ready(&e, 7), &mut e, &reg, &km,
            &crate::jobs::InlineExecutor::default(), &crate::test_support::TestClock::new(0),
            &tx, &crate::test_support::test_fs());
        assert_eq!(e.diag.as_ref().unwrap().fix_state, crate::diag_overlay::FixState::Done,
            "the prompt-intercept arm delivered the terminal; nothing was swallowed");
    }
```

**Keymap arrange:** the literals above use `cua_keymap()` directly — the shipped
`app.rs::tests` harness helper the `*_via_reduce` neighbors already use (verified at source);
copying the bodies compiles as-is.

`registry.rs::tests`: `quick_fix_fires_a_fix_request_and_sets_fetching_from_accepted` — install
a `RecordingProvider` (LTeX, enabled) + a valid diagnostic in the store, set
`active_analysis_source`, dispatch `quick_fix`; assert the recorder saw
`ProviderCall::RequestFixes` with the minted token and `editor.diag.unwrap().fix_state ==
Fetching`; repeat with `with_accepted(Accepted::No)` → `FixState::Done`, `fix_token == None`.

- [ ] **Step 2: reds** (compile failures on the new seam items).

- [ ] **Step 3: Implement.**
  - `diag_provider.rs`: trait method (spec §3.2 doc comment verbatim, default body
    `{ let _ = (token, buffer_id, version, range, code, message); Accepted::No }`);
    `ProviderSet::request_fixes(&mut self, source, token, buffer_id, version, range, code,
    message) -> Accepted` (get_mut → delegate; `None` → `Accepted::No`); `ProviderCall` gains
    `RequestFixes { token: u64, buffer_id: BufferId, version: u64,
    range: std::ops::Range<usize>, code: Option<String>, message: String }`;
    `RecordingProvider` records + returns its settable `accepted`.
  - `lsp_client.rs` `impl DiagnosticsProvider for LspProvider<E>`: mirror `notify_change` —
    send `Cmd::RequestFixes { .. }`; `Ok → Accepted::Yes`, `Err → set_availability(Unavailable)
    + Accepted::No`.
  - `editor.rs`: `pub next_fix_token: u64` (init 1) beside `diag_hint_shown`.
  - `diag_overlay.rs`: `pub fix_token: Option<u64>`, `pub fix_state: FixState`,
    `pub enum FixState { Fetching, Done }`; `DiagOverlay::new` inits `None`/`Done`.
    (Row model unchanged this task — T6; `rows()` does not exist yet, so paint/apply behavior
    is untouched and green.)
  - `registry.rs` `quick_fix` handler: after `c.editor.open_diag(d.clone())`, mint
    `let token = c.editor.next_fix_token; c.editor.next_fix_token += 1;`, call
    `c.editor.diag_providers.request_fixes(c.editor.active_analysis_source, token, bid, ver,
    d.range.clone(), d.code.clone(), d.message.clone())`, set
    `fix_token`/`fix_state` from the result per spec §3.2.
  - The delivery logic is a SHARED fn with TWO call sites (plan-gate finding 4: the modal
    prompt intercept consumes every message with explicit forwarding arms — the sweep result:
    `prompts::intercept` is the SOLE consume-all intercept; all nine other overlay intercepts
    `Handled::Pass` non-key messages, verified). In `search_ui.rs` (beside
    `diag_apply_selected`):

```rust
/// E11 §3.4: deliver a fix terminal — token-keyed CONSUMPTION, version-gated DISPLAY. Any
/// terminal for a token nobody holds is silence (displaced/expired/closed requests). TWO
/// call sites: `app::reduce_dispatch`'s arm and `prompts::intercept`'s modal-delivery arm
/// (the DiagProviderEvent "second delivery site" precedent) — one body, no drift.
pub(crate) fn apply_diag_fixes_ready(editor: &mut Editor, token: u64, version: u64,
    suggestions: Vec<wordcartel_core::diagnostics::Suggestion>) {
    if editor.diag.as_ref().map(|ov| ov.fix_token) != Some(Some(token)) { return; }
    let same_version = editor.diag.as_ref()
        .map(|ov| ov.opened_version == version && editor.active().document.version == version)
        .unwrap_or(false);
    if same_version {
        if let Some(ov) = editor.diag.as_mut() {
            ov.anchor.suggestions = suggestions;
            ov.fix_state = crate::diag_overlay::FixState::Done;
            // T6 replaces this clamp with apply_fix_delivery (the §5.2 selection policy).
            ov.selected = ov.selected.min(ov.row_count().saturating_sub(1));
        }
    } else {
        editor.diag = None;
        editor.set_status_full(crate::status::StatusKind::Warning,
            "document changed; re-open", crate::status::StatusLifetime::Sticky,
            crate::status::StatusSource::Host, None);
    }
}
```

  - `app.rs`: replace the inert arm with
    `Msg::DiagFixesReady { token, version, suggestions, .. } =>
    crate::search_ui::apply_diag_fixes_ready(editor, token, version, suggestions),`
  - `prompts.rs::intercept`: add the arm beside the `Msg::DiagProviderEvent` forwarding arm
    (BEFORE the `_ => {}`), with the same second-delivery-site comment style:

```rust
        // E11 §3.4: a fix terminal must reach its overlay even under an open modal — the
        // token's exactly-once terminal is the only one that will ever come (second delivery
        // site beside reduce_dispatch's arm; the `_ => {}` would otherwise swallow it).
        Msg::DiagFixesReady { token, version, suggestions, .. } =>
            crate::search_ui::apply_diag_fixes_ready(editor, token, version, suggestions),
```

- [ ] **Step 4: Green + gate + commit**

```bash
git add wordcartel/src/diag_provider.rs wordcartel/src/lsp_client.rs wordcartel/src/editor.rs \
    wordcartel/src/diag_overlay.rs wordcartel/src/app.rs wordcartel/src/registry.rs \
    wordcartel/src/search_ui.rs wordcartel/src/prompts.rs
git commit -m "feat: fix-fetch seam — token mint, Accepted result, token-keyed delivery (E11 T5)"
```

---

### Task 6: The `DiagRow` model + selection policy + kind-aware rows (TDD)

**Files:**
- Modify: `wordcartel/src/diag_overlay.rs`, `wordcartel/src/search_ui.rs`
  (`diag_apply_selected`), `wordcartel/src/render_overlays.rs` (`paint_diag` labels),
  `wordcartel/src/app.rs` (the T5 clamp → policy call)
- Test: `diag_overlay.rs::tests` (+ the search_ui delivery tests updated)

**Migration census for the row model:** `DiagOverlay::{row_count, is_ignore, is_add_dict,
chosen_suggestion}` — callers: `search_ui::diag_apply_selected` (is_ignore/is_add_dict/
chosen_suggestion → re-keyed on `rows()[selected]`), `render_overlays::paint_diag` (the
`i < n_sugg` label ladder → `rows()` labels), the `row_count()` production consumers — `render_overlays::paint_diag` (its
`keep_overlay_visible` call + windowing), `mouse.rs::mouse_diag` (hover/wheel/click paths),
`chrome_geom::diag_row_at` — all index-based and UNCHANGED, since `row_count()` now delegates
to `rows().len()` (`app::keep_overlay_visible` itself is a helper DEFINITION, not a row-model
consumer — plan-gate Minor 2), `diag_overlay`'s own
`diag_window_follows_selection` test (its `tall_diag` expectation `28 + 2 = 30` still holds for
a Spelling anchor with `FixState::Done` — Spelling keeps both standing rows; update the fixture
to set `fix_state: Done` explicitly if the T5 default differs).

- [ ] **Step 1: Failing tests** (`diag_overlay.rs::tests`):

```rust
    fn diag(kind: DiagnosticKind, href: Option<&str>, n_sugg: usize) -> DiagOverlay {
        let suggestions = (0..n_sugg).map(|i| Suggestion::ReplaceWith(format!("s{i}"))).collect();
        let d = Diagnostic { range: 0..1, kind,
            source: wordcartel_core::diagnostics::DiagSource::LTeX,
            code: Some("C".into()), href: href.map(str::to_string),
            message: "m".into(), suggestions };
        DiagOverlay::new(d, crate::editor::BufferId(1), 0)
    }

    #[test]
    fn rows_are_kind_aware_and_href_conditional() {
        let mut sp = diag(DiagnosticKind::Spelling, None, 1);
        sp.fix_state = FixState::Done;
        assert_eq!(sp.rows(), vec![DiagRow::Suggestion(0), DiagRow::IgnoreOnce,
            DiagRow::AddToDictionary], "Spelling keeps the two standing rows; no dismiss");
        let mut gr = diag(DiagnosticKind::Grammar, Some("https://x"), 0);
        gr.fix_state = FixState::Done;
        assert_eq!(gr.rows(), vec![DiagRow::NoFixes, DiagRow::LearnMore, DiagRow::DismissSession],
            "Grammar: dismiss instead of the spelling rows; LearnMore iff href");
    }

    #[test]
    fn fetching_row_shows_while_fetching_and_empty_done_shows_nofixes() {
        let mut g = diag(DiagnosticKind::Grammar, None, 0);
        g.fix_state = FixState::Fetching;
        assert_eq!(g.rows()[0], DiagRow::FetchingFixes);
        g.fix_state = FixState::Done;
        assert_eq!(g.rows()[0], DiagRow::NoFixes);
    }

    #[test]
    fn delivery_preserves_user_selection_by_row_identity() {
        // Round-3 Important-4: a writer parked on IgnoreOnce stays there when rows appear.
        let mut sp = diag(DiagnosticKind::Spelling, None, 0);
        sp.fix_state = FixState::Fetching; // rows: [FetchingFixes, IgnoreOnce, AddToDictionary]
        sp.selected = 1; // IgnoreOnce
        let before = sp.rows()[sp.selected].clone();
        sp.apply_fix_delivery(vec![Suggestion::ReplaceWith("a".into()),
            Suggestion::ReplaceWith("b".into())]);
        assert_eq!(sp.rows()[sp.selected], before, "selection followed the ROW, not the index");
    }

    #[test]
    fn delivery_on_vanished_fetching_row_resets_deterministically() {
        let mut sp = diag(DiagnosticKind::Spelling, None, 0);
        sp.fix_state = FixState::Fetching;
        sp.selected = 0; // FetchingFixes
        sp.apply_fix_delivery(vec![Suggestion::ReplaceWith("a".into())]);
        assert_eq!(sp.rows()[sp.selected], DiagRow::Suggestion(0), "deliberate reset, not clamp");
        let mut g = diag(DiagnosticKind::Grammar, None, 0);
        g.fix_state = FixState::Fetching;
        g.selected = 0;
        g.apply_fix_delivery(vec![]);
        assert_eq!(g.rows()[g.selected], DiagRow::NoFixes, "empty delivery lands on NoFixes");
    }
```

  And in `search_ui.rs::tests`, the complete no-op literal:

```rust
    #[test]
    fn enter_on_fetching_and_nofixes_rows_is_a_noop() {
        for fetch_state in [crate::diag_overlay::FixState::Fetching,
                            crate::diag_overlay::FixState::Done] {
            let mut e = Editor::new_from_text("ab\n", None, (40, 10));
            let d = wordcartel_core::diagnostics::Diagnostic { range: 0..1,
                kind: wordcartel_core::diagnostics::DiagnosticKind::Grammar,
                source: wordcartel_core::diagnostics::DiagSource::LTeX,
                code: Some("R".into()), href: None, message: "m".into(), suggestions: vec![] };
            e.open_diag(d);
            e.diag.as_mut().unwrap().fix_state = fetch_state;
            e.diag.as_mut().unwrap().selected = 0; // FetchingFixes or NoFixes, per state
            let v = e.active().document.version;
            crate::search_ui::diag_apply_selected(&mut e, &crate::test_support::TestClock::new(0));
            assert_eq!(e.active().document.version, v, "no edit from a no-op row");
            assert!(e.diag.is_some(), "overlay stays open — the row is inert, not a dismissal");
        }
    }
```

- [ ] **Step 2: reds** (compile: `DiagRow`, `rows()`, `apply_fix_delivery` undefined).

- [ ] **Step 3: Implement** (`diag_overlay.rs`):

```rust
/// E11 §5.1: the computed row list — one source of truth for paint, mouse, and apply.
/// Conditional rows made index arithmetic impossible; identity selection (§5.2) needs values.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DiagRow {
    Suggestion(usize),
    FetchingFixes,
    NoFixes,
    LearnMore,
    IgnoreOnce,
    AddToDictionary,
    DismissSession,
}

impl DiagOverlay {
    /// Pure fn of anchor + fetch state (spec §5.1).
    pub fn rows(&self) -> Vec<DiagRow> {
        let mut out: Vec<DiagRow> = (0..self.anchor.suggestions.len())
            .map(DiagRow::Suggestion).collect();
        if out.is_empty() {
            out.push(match self.fix_state {
                FixState::Fetching => DiagRow::FetchingFixes,
                FixState::Done => DiagRow::NoFixes,
            });
        }
        if self.anchor.href.is_some() { out.push(DiagRow::LearnMore); }
        match self.anchor.kind {
            wordcartel_core::diagnostics::DiagnosticKind::Spelling => {
                out.push(DiagRow::IgnoreOnce);
                out.push(DiagRow::AddToDictionary);
            }
            wordcartel_core::diagnostics::DiagnosticKind::Grammar => {
                out.push(DiagRow::DismissSession);
            }
        }
        out
    }

    pub fn row_count(&self) -> usize { self.rows().len() }

    /// E11 §5.2: deliver fetched suggestions with the selection POLICY — identity for
    /// surviving rows, deliberate reset when the fetching row vanished.
    pub fn apply_fix_delivery(&mut self, suggestions: Vec<Suggestion>) {
        let prev = self.rows().get(self.selected).cloned();
        self.anchor.suggestions = suggestions;
        self.fix_state = FixState::Done;
        let rows = self.rows();
        self.selected = match prev {
            Some(DiagRow::FetchingFixes) | None => 0, // deliberate reset (Suggestion(0)/NoFixes)
            Some(row) => rows.iter().position(|r| *r == row).unwrap_or(0),
        };
    }

    /// The row Enter activates — `None` for the non-activatable states.
    pub fn selected_row(&self) -> Option<DiagRow> { self.rows().get(self.selected).cloned() }
}
```

  Delete `is_ignore`/`is_add_dict` (their SOLE caller is `diag_apply_selected` — re-keyed
  now); keep `chosen_suggestion` re-implemented via `selected_row()` matching
  `DiagRow::Suggestion(i)`. `up`/`down` unchanged (operate on `row_count()`).
  `search_ui::diag_apply_selected`: replace the `is_ignore/is_add_dict/suggestion` extraction
  with a `match ov.selected_row()` — `Suggestion(i)` → the existing apply branch (unchanged
  body), `IgnoreOnce`/`AddToDictionary` → the existing branches, `LearnMore` → T8 placeholder
  `{}` comment (T8 fills it), `DismissSession` → T7 placeholder `{}`, `FetchingFixes`/`NoFixes`
  /`None` → return without closing (no-op rows). `render_overlays::paint_diag`: the label
  ladder becomes a `match` over `rows()[i]` (labels: `suggestion_label(..)`,
  `"fetching fixes…"`, `"(no fixes available)"`, `"Learn more (copy link)"`, `"Ignore once"`,
  `"Add to dictionary"`, `"Dismiss for this session"`). `app.rs`'s T5 arm: replace the clamp
  line with `ov.apply_fix_delivery(suggestions);` (delete the `anchor.suggestions =` and
  `fix_state =` lines — `apply_fix_delivery` owns both).

- [ ] **Step 4: Green + gate + commit** (`tall_diag`'s expectation update per the census note).

```bash
git add wordcartel/src/diag_overlay.rs wordcartel/src/search_ui.rs \
    wordcartel/src/render_overlays.rs wordcartel/src/app.rs
git commit -m "feat: DiagRow model — kind-aware rows, fetch states, identity selection (E11 T6)"
```

---

### Task 7: The session dismiss — pair key + equality filter (TDD)

**Files:**
- Modify: `wordcartel/src/editor.rs`, `wordcartel/src/diagnostics_run.rs`,
  `wordcartel/src/search_ui.rs` (the `DismissSession` arm)
- Test: `diagnostics_run.rs::tests`

**Interfaces (produces):** `diagnostics_run::DismissKey { pub sentence: String, pub line: String }`
(`Clone, PartialEq, Eq, Hash, Debug`); `Editor.session_dismissals:
std::collections::HashSet<(DiagSource, String, DismissKey)>`;
`diagnostics_run::dismissal_units_at(buf: &TextBuffer, pos: usize) -> DismissKey` (the shared
parse-free derivation — rope blank-line window + `wordcartel_core::textobj::sentence_bounds`
for `.sentence`; `lines::line_text(buf, buf.byte_to_line(pos))` for `.line`).

- [ ] **Step 1: Failing tests** (`diagnostics_run.rs::tests`; build diagnostics via the
  existing fixture style — `Diagnostic { range, kind: Grammar, source: DiagSource::LTeX,
  code: Some("R"), .. }` — and editors via `Editor::new_from_text`):

```rust
    #[test]
    fn dismissal_units_pair_sentence_and_line() {
        let e = crate::editor::Editor::new_from_text(
            "Para one here. Para two here.\n\nOther block.\n", None, (80, 24));
        let k = dismissal_units_at(&e.active().document.buffer, 16); // inside "Para two"
        assert_eq!(k.sentence, "Para two here.");
        assert_eq!(k.line, "Para one here. Para two here.");
    }

    // ── T7 harness (all bodies complete — plan-gate round-2 finding 4) ──────────────────────

    fn gdiag(range: std::ops::Range<usize>, code: &str) -> Diagnostic {
        Diagnostic { range, kind: DiagnosticKind::Grammar, source: DiagSource::LTeX,
            code: Some(code.into()), href: None, message: "m".into(), suggestions: vec![] }
    }
    fn dismiss_at(e: &mut crate::editor::Editor, pos: usize, code: &str) {
        let key = dismissal_units_at(&e.active().document.buffer, pos);
        e.session_dismissals.insert((DiagSource::LTeX, code.into(), key));
    }
    fn seed_slot(e: &mut crate::editor::Editor, diags: Vec<Diagnostic>) {
        let v = e.active().document.version;
        let slot = e.active_mut().diagnostics.slot_mut(DiagSource::LTeX);
        slot.diagnostics = diags;
        slot.computed_version = v;
    }
    fn slot_ranges(e: &crate::editor::Editor) -> Vec<std::ops::Range<usize>> {
        e.active().diagnostics.slot(DiagSource::LTeX)
            .map(|s| s.diagnostics.iter().map(|d| d.range.clone()).collect())
            .unwrap_or_default()
    }

    #[test]
    fn dismissal_filters_with_an_empty_spelling_union() {
        // The guard regression (round-1 finding 7): NO dictionary words, NO session ignores —
        // a dismissal alone must still filter, in BOTH call paths.
        let mut e = crate::editor::Editor::new_from_text("Alpha beta gamma. Delta eps.\n",
            None, (80, 24));
        e.diag_providers.install(Box::new(crate::diag_provider::RecordingProvider::new()
            .with_source(DiagSource::LTeX)), true);
        assert!(e.dictionary.is_empty() && e.session_ignores.is_empty());
        dismiss_at(&mut e, 18, "R"); // inside "Delta eps."
        seed_slot(&mut e, vec![gdiag(18..23, "R")]);
        retain_unignored(&mut e);
        assert!(slot_ranges(&e).is_empty(), "retain_unignored path filters on dismissals alone");
        let (id, v) = (e.active().id, e.active().document.version);
        apply_diagnostics_done(&mut e, id, v, DiagSource::LTeX, vec![gdiag(18..23, "R")]);
        assert!(slot_ranges(&e).is_empty(), "republish path filters on dismissals alone");
    }

    #[test]
    fn dismiss_filters_by_pair_equality_and_reapplies_on_republish() {
        let mut e = crate::editor::Editor::new_from_text(
            "Alpha beta gamma. Delta epsilon zeta.\n", None, (80, 24));
        e.diag_providers.install(Box::new(crate::diag_provider::RecordingProvider::new()
            .with_source(DiagSource::LTeX)), true);
        dismiss_at(&mut e, 18, "R"); // "Delta epsilon zeta." starts at byte 18
        seed_slot(&mut e, vec![gdiag(18..23, "R"), gdiag(0..5, "R")]); // + one in sentence 1
        retain_unignored(&mut e);
        assert_eq!(slot_ranges(&e), vec![0..5], "only the dismissed sentence's flag dropped");
        let (id, v) = (e.active().id, e.active().document.version);
        apply_diagnostics_done(&mut e, id, v, DiagSource::LTeX,
            vec![gdiag(18..23, "R"), gdiag(0..5, "R")]);
        assert_eq!(slot_ranges(&e), vec![0..5], "the dismissal re-applies on every republish");
    }

    #[test]
    fn identical_wording_in_a_different_sentence_survives() {
        // D9 discriminator: same flagged wording, different enclosing sentence.
        let mut e = crate::editor::Editor::new_from_text(
            "Go now please. You should go now please today.\n", None, (80, 24));
        e.diag_providers.install(Box::new(crate::diag_provider::RecordingProvider::new()
            .with_source(DiagSource::LTeX)), true);
        dismiss_at(&mut e, 0, "R"); // sentence 1: "Go now please."
        seed_slot(&mut e, vec![gdiag(26..28, "R")]); // "no(w)" inside sentence 2
        retain_unignored(&mut e);
        assert_eq!(slot_ranges(&e), vec![26..28], "different sentence-unit ⇒ survives");
    }

    #[test]
    fn heading_dismissal_stays_scoped_to_that_line() {
        // D10 discriminator + round-3 counterexample: "# Title" dismissed; body prose
        // containing "Title" keeps its flag (line-units differ).
        let mut e = crate::editor::Editor::new_from_text(
            "# Title\n\nThe Title is tentative.\n", None, (80, 24));
        e.diag_providers.install(Box::new(crate::diag_provider::RecordingProvider::new()
            .with_source(DiagSource::LTeX)), true);
        dismiss_at(&mut e, 2, "R"); // inside the heading line
        seed_slot(&mut e, vec![gdiag(13..18, "R")]); // "Title" inside the body sentence
        retain_unignored(&mut e);
        assert_eq!(slot_ranges(&e), vec![13..18], "the body flag survives the heading dismissal");
    }

    #[test]
    fn dismissed_sentence_does_not_suppress_a_longer_containing_sentence() {
        // Round-3: EQUALITY, not containment. The segmenter keeps "Dr. Smith arrived." as
        // ONE sentence (the shipped textobj doctest), which literally CONTAINS the dismissed
        // "Smith arrived." — containment would suppress it; equality must not.
        let mut e = crate::editor::Editor::new_from_text(
            "Smith arrived. Yes.\n\nDr. Smith arrived. Indeed.\n", None, (80, 24));
        e.diag_providers.install(Box::new(crate::diag_provider::RecordingProvider::new()
            .with_source(DiagSource::LTeX)), true);
        dismiss_at(&mut e, 0, "R"); // key sentence: "Smith arrived."
        seed_slot(&mut e, vec![gdiag(25..30, "R")]); // "Smith" inside "Dr. Smith arrived."
        retain_unignored(&mut e);
        assert_eq!(slot_ranges(&e), vec![25..30], "superstring sentence ⇒ NOT equal ⇒ survives");
    }

    #[test]
    fn identical_pair_across_roles_is_suppressed_documented_behavior() {
        // Round-5 Minor-3, on a GENUINE cross-role fixture (plan-gate round-3 finding 1):
        // Markdown lazy continuation makes the first `Title` blockquote CONTENT
        // (role-non-prose — an unmarked line under `> Quote.` belongs to the quote), yet its
        // pair derives sentence unit "Title" (`Quote.` terminates the preceding sentence in
        // the blank-line window "> Quote.\nTitle") and line unit "Title" — byte-identical to
        // the isolated one-line PARAGRAPH `Title` below. The pair rule is ROLE-BLIND: the
        // dismissal suppresses both. Documented identical-text collision class, not
        // separation (spec §5.3's context-sensitive-Markdown caveat, made concrete).
        let mut e = crate::editor::Editor::new_from_text(
            "> Quote.\nTitle\n\nTitle\n", None, (80, 24));
        e.diag_providers.install(Box::new(crate::diag_provider::RecordingProvider::new()
            .with_source(DiagSource::LTeX)), true);
        dismiss_at(&mut e, 9, "R"); // the lazy-continuation "Title" (bytes 9..14, non-prose)
        seed_slot(&mut e, vec![gdiag(16..21, "R")]); // the paragraph "Title" (bytes 16..21)
        retain_unignored(&mut e);
        assert!(slot_ranges(&e).is_empty(),
            "byte-equal line AND sentence units across ROLES ⇒ suppressed, role never consulted");
    }

    #[test]
    fn rewrap_of_the_containing_line_drops_the_dismissal_documented_behavior() {
        // The named limit: rewrapping changes the line-unit; the pair no longer matches.
        let mut a = crate::editor::Editor::new_from_text(
            "Alpha beta gamma delta.\n", None, (80, 24));
        dismiss_at(&mut a, 0, "R");
        let mut b = crate::editor::Editor::new_from_text(
            "Alpha beta\ngamma delta.\n", None, (80, 24)); // same sentence, rewrapped
        b.diag_providers.install(Box::new(crate::diag_provider::RecordingProvider::new()
            .with_source(DiagSource::LTeX)), true);
        b.session_dismissals = a.session_dismissals.clone();
        seed_slot(&mut b, vec![gdiag(0..5, "R")]);
        retain_unignored(&mut b);
        assert_eq!(slot_ranges(&b), vec![0..5], "line-unit changed ⇒ the flag honestly returns");
    }

    #[test]
    fn filter_runs_on_non_active_buffer_apply_without_touching_any_tree() {
        // Lazy-reparse invariant: the apply lands on a NON-active buffer; the filter must
        // use rope+textobj against THAT buffer's text (never the lens/classifier).
        let mut e = crate::editor::Editor::new_from_text(
            "Alpha beta gamma. Delta epsilon zeta.\n", None, (80, 24));
        e.diag_providers.install(Box::new(crate::diag_provider::RecordingProvider::new()
            .with_source(DiagSource::LTeX)), true);
        dismiss_at(&mut e, 18, "R");
        let (target, v) = (e.active().id, e.active().document.version);
        e.install_scratch();
        crate::workspace::goto_scratch(&mut e);
        assert_ne!(e.active().id, target, "the target buffer is NOT active");
        apply_diagnostics_done(&mut e, target, v, DiagSource::LTeX, vec![gdiag(18..23, "R")]);
        let stored = e.by_id_mut(target).unwrap().diagnostics.slot(DiagSource::LTeX)
            .map(|s| s.diagnostics.len()).unwrap_or(0);
        assert_eq!(stored, 0, "dismissal filtered on the non-active buffer's own text");
    }
```

  (Byte offsets in the fixtures are computed from the literal texts — the implementer verifies
  each with a `&text[a..b]` scratch assertion if in doubt; empty-key refusal is a `search_ui`
  handler test, T7 Step 1b below.)

- [ ] **Step 1b: the empty-key refusal handler test** (`search_ui.rs::tests`):

```rust
    #[test]
    fn dismiss_on_an_empty_line_is_refused_at_store_time() {
        let mut e = Editor::new_from_text("a\n\nb\n", None, (40, 10));
        let d = wordcartel_core::diagnostics::Diagnostic { range: 2..2,
            kind: wordcartel_core::diagnostics::DiagnosticKind::Grammar,
            source: wordcartel_core::diagnostics::DiagSource::LTeX,
            code: Some("R".into()), href: None, message: "m".into(), suggestions: vec![] };
        e.open_diag(d);
        // Select the DismissSession row (Grammar, no href, Done ⇒ rows = [NoFixes, Dismiss]).
        e.diag.as_mut().unwrap().fix_state = crate::diag_overlay::FixState::Done;
        e.diag.as_mut().unwrap().selected = 1;
        crate::search_ui::diag_apply_selected(&mut e, &crate::test_support::TestClock::new(0));
        assert!(e.session_dismissals.is_empty(), "empty line-unit ⇒ refused (belt)");
        assert_eq!(e.status_text(), "cannot dismiss here");
    }
```

- [ ] **Step 2: reds.**

- [ ] **Step 3: Implement.**
  - `dismissal_units_at` (valid-Rust shape per plan-gate finding 8):

```rust
/// E11 §5.3: BOTH parse-free units at `pos` — the blank-line-window sentence + the source
/// line. Rope + `textobj` only (no block tree — safe on any buffer; the lazy-reparse law).
pub(crate) fn dismissal_units_at(buf: &wordcartel_core::buffer::TextBuffer, pos: usize)
    -> DismissKey {
    let pos = pos.min(buf.len());
    let line = buf.byte_to_line(pos);
    // Expand to the nearest blank-line/document boundaries (source-level paragraph).
    let mut first = line;
    while first > 0 && !crate::lines::line_text(buf, first - 1).is_empty() { first -= 1; }
    let total = crate::lines::total_logical_lines(buf); // the shell's line-count helper (verified)
    let mut last = line;
    while last + 1 < total && !crate::lines::line_text(buf, last + 1).is_empty() { last += 1; }
    let win_start = buf.line_to_byte(first);
    let win_end = if last + 1 < total { buf.line_to_byte(last + 1) } else { buf.len() };
    let window = buf.slice(win_start..win_end).to_string();
    let rel = pos.saturating_sub(win_start).min(window.len());
    let (from, to) = wordcartel_core::textobj::sentence_bounds(&window, rel);
    DismissKey {
        sentence: window.get(from..to).unwrap_or("").to_string(),
        line: crate::lines::line_text(buf, line),
    }
}
```

  (Every symbol above is verified at source: `TextBuffer::byte_to_line`/`line_to_byte`
  (`wordcartel-core/src/buffer.rs`), `lines::line_text` and `lines::total_logical_lines`
  (`wordcartel/src/lines.rs`), `textobj::sentence_bounds`. Boundary contract: clamp, checked
  slice, empty-on-out-of-range.)
  - `Editor.session_dismissals` field + init (beside `session_ignores`).
  - Filter: extend the retain pass — `retain_over_union` gains a `dismissals` parameter (or a
    sibling fn folded in the SAME pass, plan-preference: extend the signature). Migration
    census for the signature: callers are exactly `retain_unignored` and
    `apply_diagnostics_done` (both in `diagnostics_run.rs` — verified sole callers) — **and
    BOTH callers' emptiness GUARDS must change (plan-gate finding 7), not just the
    signature:** `retain_unignored`'s early return becomes
    `if union.is_empty() && editor.session_dismissals.is_empty() { return; }`, and
    `apply_diagnostics_done`'s `if !union.is_empty() { … }` filter call becomes
    `if !union.is_empty() || !dismissals.is_empty() { … }` (with the dismissals snapshot taken
    beside `ignore_union_lower`'s). A regression test pins the miss:
    `dismissal_filters_with_an_empty_spelling_union` (no dictionary/session-ignore words at
    all; a dismissal alone must still drop its diagnostic in both call paths). Rule per spec
    §5.3: (source, code-or-empty) match AND `dismissal_units_at(buf, d.range.start) == key`
    (line short-circuit first).
  - `search_ui.rs` `DismissSession` arm: derive the key at the anchor
    (`dismissal_units_at(&editor.active().document.buffer, a)`), REFUSE empty `.line`
    (status "cannot dismiss here"), else insert
    `(anchor.source, anchor.code.clone().unwrap_or_default(), key)`, close, `retain_unignored`.

- [ ] **Step 4: Green + gate + commit**

```bash
git add wordcartel/src/editor.rs wordcartel/src/diagnostics_run.rs wordcartel/src/search_ui.rs
git commit -m "feat: session dismiss — sentence+line pair key, equality filter (E11 T7)"
```

---

### Task 8: The Learn-more row (TDD)

**Files:**
- Modify: `wordcartel/src/search_ui.rs`
- Test: `search_ui.rs::tests`

- [ ] **Step 1: Failing test** (`search_ui.rs::tests`, complete):

```rust
    #[test]
    fn learn_more_copies_href_acks_and_keeps_the_overlay_open() {
        let mut e = Editor::new_from_text("ab\n", None, (40, 10));
        let d = wordcartel_core::diagnostics::Diagnostic { range: 0..1,
            kind: wordcartel_core::diagnostics::DiagnosticKind::Grammar,
            source: wordcartel_core::diagnostics::DiagSource::LTeX,
            code: Some("PASSIVE_VOICE".into()),
            href: Some("https://community.languagetool.org/rule/show/PASSIVE_VOICE?lang=en-US".into()),
            message: "m".into(), suggestions: vec![] };
        e.open_diag(d);
        e.diag.as_mut().unwrap().fix_state = crate::diag_overlay::FixState::Done;
        // rows for Grammar + href + Done + no suggestions: [NoFixes, LearnMore, DismissSession]
        e.diag.as_mut().unwrap().selected = 1;
        crate::search_ui::diag_apply_selected(&mut e, &crate::test_support::TestClock::new(0));
        assert_eq!(e.clipboard_sync_request.as_deref(),
            Some("https://community.languagetool.org/rule/show/PASSIVE_VOICE?lang=en-US"),
            "the href reached the shipped copy-out intent");
        assert_eq!(e.status_text(), "link copied", "the MANDATORY ack (D5)");
        assert!(e.diag.is_some(), "copy is not a dismissal — the overlay stays open");
    }
```
- [ ] **Step 2: red.**
- [ ] **Step 3: Implement** — the `LearnMore` arm in `diag_apply_selected`:

```rust
        DiagRow::LearnMore => {
            // E11 §5.4 (D5): copy is the action (OSC 52 ⇒ works over SSH/tmux); the ack is
            // MANDATORY — clipboard copy is invisible, an unacknowledged row reads as dead.
            if let Some(href) = editor.diag.as_ref().and_then(|ov| ov.anchor.href.clone()) {
                editor.clipboard_sync_request = Some(href);
                editor.set_status(crate::status::StatusKind::Info, "link copied");
            }
            // Overlay stays open (spec §5.4).
        }
```

- [ ] **Step 4: Green + gate + commit** (`git add wordcartel/src/search_ui.rs`;
  `"feat: Learn-more row — copy href + status ack (E11 T8)"`).

---

### Task 9: The bottom-anchored detail box (TDD)

**Files:**
- Modify: `wordcartel/src/chrome_geom.rs`, `wordcartel/src/render_overlays.rs`,
  `wordcartel/src/render.rs` (`paint_status` — ONE guarded call, beside `paint_prompt_detail`'s)
- Test: `chrome_geom.rs::tests` (geometry) + `render_overlays.rs`/e2e-style rendered-screen
  tests (mirror the existing `paint_prompt_detail` FAIL-VERIFY tests' arrange in `render.rs`)

- [ ] **Step 1: Failing geometry tests** (`chrome_geom.rs::tests`, beside the
  `prompt_detail_rect_*` set):

```rust
    #[test]
    fn diag_detail_rect_caps_below_the_overlay_and_declines_when_no_room() {
        let area = Rect::new(0, 0, 80, 24);
        let overlay = palette_overlay_rect(area, 5);
        let r = diag_detail_rect(area, 23, overlay, 3).expect("room on a 24-row screen");
        assert!(r.y > overlay.y + overlay.height - 1, "top edge strictly below the overlay");
        assert!(r.y + r.height <= 23, "sits above the status row");
        // A short screen where the overlay eats the space → None, never overlap.
        let small = Rect::new(0, 0, 80, 8);
        let ov_small = palette_overlay_rect(small, 5);
        assert!(diag_detail_rect(small, 7, ov_small, 3).is_none()
            || diag_detail_rect(small, 7, ov_small, 3).unwrap().y
                > ov_small.y + ov_small.height - 1);
    }

    #[test]
    fn diag_detail_rect_disjointness_property_sweep() {
        // Spec §6: rect ∩ overlay == ∅ or None — sweep sizes × row counts × line counts.
        for h in 5..40u16 { for rows in 1..20usize { for lines in 1..12usize {
            let area = Rect::new(0, 0, 60, h);
            let ov = palette_overlay_rect(area, rows);
            if let Some(r) = diag_detail_rect(area, h.saturating_sub(1), ov, lines) {
                let disjoint = r.y >= ov.y + ov.height || r.y + r.height <= ov.y;
                assert!(disjoint, "h={h} rows={rows} lines={lines}: {r:?} vs {ov:?}");
            }
        }}}
    }
```

- [ ] **Step 2: reds.**
- [ ] **Step 3: Implement** — `chrome_geom::diag_detail_rect(area, status_row, overlay: Rect,
  lines: usize) -> Option<Rect>`: start from `prompt_detail_rect`'s body (same width
  ladder/centering, bottom-anchored above `status_row`, `None` under 3 free rows) PLUS the cap:
  the candidate rect's `y` is raised to `max(y, overlay.y + overlay.height)` with the height
  reduced accordingly; `None` when no content row remains. `render_overlays::paint_diag_detail
  (frame, diag: &DiagOverlay, area, status_row, cs)`: wrapped `anchor.message` (plain word
  wrap to the box width — prose, NOT the prompt's path-elision), an attribution line that
  OMITS the code entirely when `None` (the spec governs — plan-gate finding 9):
  `match anchor.code.as_deref() { Some(c) => format!("{} · {}", anchor.source.label(), c),
  None => anchor.source.label().to_string() }`, the
  `href` as a truncated courtesy line when present, the prompt precedent's
  `…and N more` summary row when wrapped lines exceed the box. `render.rs::paint_status`:
  beside the `paint_prompt_detail` call, add
  `if let Some(diag) = editor.diag.as_ref() { crate::render_overlays::paint_diag_detail(frame, diag, area, status_row, cs); }`
  — the shipped "painted FOR that one overlay" pattern; no `OVERLAYS`/`RenderSite` change.
  Rendered-screen test (the C5 lesson — in `render.rs::tests`, on the VERIFIED harness
  helpers `render_to_buffer(editor, w, h) -> ratatui::buffer::Buffer` and
  `screen_text(&Buffer) -> String`; complete):

```rust
    #[test]
    fn diag_detail_box_renders_message_on_tall_screens_and_declines_on_short() {
        let mk = || {
            let mut e = Editor::new_from_text("ab\n", None, (80, 30));
            let d = wordcartel_core::diagnostics::Diagnostic { range: 0..1,
                kind: wordcartel_core::diagnostics::DiagnosticKind::Grammar,
                source: wordcartel_core::diagnostics::DiagSource::LTeX,
                code: Some("PASSIVE_VOICE".into()), href: None,
                message: "The passive voice was used here by this sentence.".into(),
                suggestions: vec![] };
            e.open_diag(d);
            e.diag.as_mut().unwrap().fix_state = crate::diag_overlay::FixState::Done;
            e
        };
        let tall = screen_text(&render_to_buffer(&mut mk(), 80, 30));
        assert!(tall.contains("passive voice was used"), "the wrapped message renders: {tall}");
        assert!(tall.contains("LTeX \u{b7} PASSIVE_VOICE"), "attribution line renders");
        let short = screen_text(&render_to_buffer(&mut mk(), 80, 6));
        assert!(!short.contains("passive voice was used"),
            "the box DECLINED on a short screen — nothing load-bearing lived in it");
        assert!(short.contains("Dismiss for this session"),
            "the overlay's own rows still render without the box");
    }
```

  (`\u{b7}` is the `·` separator; if the attribution assertion's exact spacing drifts from
  the §implementation string, fix the ASSERTION to the implemented `format!` — the invariant
  under test is presence-on-tall/absence-on-short, not spacing.)
- [ ] **Step 4: Green + gate + commit**

```bash
git add wordcartel/src/chrome_geom.rs wordcartel/src/render_overlays.rs wordcartel/src/render.rs
git commit -m "feat: bottom-anchored diag detail box (E11 T9)"
```

---

### Task 10: Advisory live probe (mandatory-run, advisory-pass)

**Files:**
- Create: `scratchpad/e11/probe/t10-live-results.md` (scratch — UNTRACKED; quoted verbatim in
  the pre-merge report, never committed)
- Modify (Step 3, the ship bookkeeping — all TRACKED): `backlog.toml` (E11 status/hook),
  `docs/ux-backlog.md` (prose section OUT), `docs/backlog-archive.md` (prose section IN),
  `BACKLOG.md` (regenerated by `scripts/backlog bless` — the generated dashboard is tracked
  and must ride the same commit, never hand-edited)
- No `wordcartel/src` changes expected — a falsified empirical claim is a FINDING for the
  controller first.

- [ ] **Step 1:** `cargo build`, then drive the real app (tui-interact) against real
  harper/ltex-ls-plus/vale-ls with a fixture tripping spelling + grammar + style:
  1. Open quick-fix on a vale spelling diagnostic → fetch state visible → MULTI-candidate list
     (probe promised 5); apply one; undo restores.
  2. Open quick-fix on an ltex `recieve`-class diagnostic → candidates per T1's outcome.
  3. Underlines appear ON PUBLISH (no 5s parking stall — compare against pre-E11 behavior).
  4. "Dismiss for this session" on a grammar diagnostic → underline drops now AND after the
     next recheck; an identical sentence elsewhere keeps its flag.
  5. "Learn more" on an ltex diagnostic → paste the clipboard somewhere → the rule URL;
     status showed "link copied".
  6. Detail box: full message + `LTeX · <rule>` at the bottom on a tall terminal; ABSENT on a
     tiny terminal with the overlay still usable.
  7. ltex idle-suspend (idle_shutdown_min = 1) → re-enter Review → open a fix during the warm
     → honest "fetching fixes…" then results, or "no fixes available" at 10 s; no hang.
  8. `scripts/smoke/run.sh` — quote the one-line summary verbatim.
- [ ] **Step 2:** Record per-line PASS/FAIL/SKIP verdicts + the summary line
  (`probe: N/8 …`). Advisory: red lines never block the merge; they are surfaced explicitly.
- [ ] **Step 3: Backlog ship bookkeeping (plan-gate Minor 3 — assigned here so it cannot be
  missed at merge time).** Per CLAUDE.md's item-shipping rules: edit E11's `[[item]]` block in
  `backlog.toml` (status → `shipped`, `shipped_commit`/`shipped_date`, hook text updated for
  the D8 relay cut per spec §8.1), MOVE the E11 prose section from `docs/ux-backlog.md` to
  `docs/backlog-archive.md` (repoint `doc =`), run `scripts/backlog bless`, and confirm the
  backlog drift GATE (`wordcartel/tests/backlog.rs`) is green.
- [ ] **Step 4: Stage + commit the bookkeeping (T10's OWN commit — round-4 gate finding:
  edited-but-unstaged tracked files).** This commit carries the ship bookkeeping; the merge
  commit itself carries no backlog edits (the E10 precedent: a dedicated
  `backlog: mark <item> shipped` commit). The probe-results scratch file is untracked and
  stays OUT.

```bash
git add backlog.toml docs/ux-backlog.md docs/backlog-archive.md BACKLOG.md
git commit -m "backlog: mark E11 shipped"
```

  (`shipped_commit` names the merge commit — so this bookkeeping commit is made AT the merge,
  immediately after the `--no-ff` merge lands, exactly like E10's `67fcd87` followed
  `9c2d9e4`.)

---

## Self-review (performed at authoring)

- **Spec coverage:** §2.3/T1 → Task 1; §4 → T2; §3.1/§3.7 → T3; §3.3/§3.4 machine → T4;
  §3.2/§3.4 reduce + §3.6 → T5; §5.1/§5.2 → T6; §5.3 → T7; §5.4 → T8; §6 → T9; §10's
  T10 → Task 10. §7 (contract) and §8/§9 (cuts/boundaries) are Global Constraints + task
  scoping. Every spec-named regression test appears in a task step by name.
- **Greenness:** T2 scaffolds carry allows removed in T4; T3's rewrites land WITH the deletion
  in one commit; T4's `Msg` variant lands with compiler-forced arms (inert reduce arm) before
  the machine emits it; T5's clamp is a stated placeholder T6 replaces; T6's row-model census
  lists every caller.
- **Migration censuses:** `Msg::DiagFixesReady` (enum + Debug impl + reduce_dispatch — the
  match is exhaustive, no catch-all, verified); the row-model callers (T6); the
  `retain_over_union` signature (two callers, verified sole); `quickfix_suggestion` deletion
  (sole production caller `on_codeaction_response` dies in the same task).
- **Honest flags (current as of plan-gate round 2):** T1's outcome B/C paths are fully
  specified (the `FIX_CONTEXT_ALL_RAWS` const seam + the `TestEngineAllRaws` test; outcome C
  is a STOP). Every spec-named regression now has a COMPLETE executable literal — the four
  delivery tests (T5, on the real `reduce` harness with the verified `cua_keymap()` helper),
  the T4 machine matrix, the nine T7 dismissal tests + the empty-key handler test, T6's
  Enter-noop, T8's Learn-more, and T9's rendered-screen test (on the verified
  `render_to_buffer`/`screen_text` helpers). Two deliberate documented-behavior pins: the
  role-blind identical-pair test on the GENUINE cross-role lazy-continuation fixture
  (plan-gate round 3 disproved the earlier unconstructibility claim — the retraction is
  recorded in the spec's amended T7 sentence) and the rewrap limit. Remaining judgment notes are marked inline where an assertion's exact string may
  track the implementation (T9's separator spacing) — invariants, not behaviors, are the
  test subjects there.
