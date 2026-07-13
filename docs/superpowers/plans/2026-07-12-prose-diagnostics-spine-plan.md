# Prose-Diagnostics Spine Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Generalize wordcartel's single-provider diagnostics plumbing (harper-ls) to multi-provider with a switchable-lens UX, proven by harper + a `#[cfg(test)]` mock second provider.

**Architecture:** Widen the pure `wordcartel_core::diagnostics::Diagnostic` with an exhaustive `DiagSource` tag + `code`/`href` (one atomic foundation task migrating all construction sites and both message shapes so the tree stays compiling); then layer the behavioral generalizations as independently-green tasks — a `ProviderSet` registry, a source-partitioned `DiagStore`, source-routed marshaling, a per-source dispatch fan-out, the switchable analysis lens + its commands, and the config wiring — each building on the last.

**Tech Stack:** Rust workspace — pure `wordcartel-core` (`#![forbid(unsafe_code)]`) + shell `wordcartel` (ratatui 0.30, crossterm). Functional-core / imperative-shell.

**Spec:** `docs/superpowers/specs/2026-07-12-prose-diagnostics-spine-design.md` (Codex-clean, committed). Section references below (§N) are to that spec.

## Global Constraints

Every task's requirements implicitly include this section. Every implementer AND reviewer reads it.

**Locked product decisions (§ intro):**
- Each engine is its OWN namespaced diagnostic view — NEVER merged, NEVER simultaneously rendered inline.
- View interaction = **switchable lens**: inline underlines + `diag_next`/`diag_prev`/`quick_fix` show exactly ONE source at a time; a cycle command + per-engine set-per-state command switch it.
- Default engine = default lens = **harper** (`DiagSource::Harper`).
- Spine scope = harper-only refit as provider #1; the single→multi path is PROVEN by a `#[cfg(test)]` mock second provider (`RecordingProvider` with `DiagSource::Plugin("mock")`). Do NOT add a real second engine (ltex/vale).

**House conventions that GATE (from CLAUDE.md):**
- `wordcartel-core` stays pure: no IO, no Lua, `#![forbid(unsafe_code)]`. New core items are plain data + pure fns.
- **Exhaustive matches on `DiagSource` — never a catch-all `_`** that would silently absorb a future `LTeX`/`Vale`/`Plugin` arm. (The `SemanticElement` exhaustive-literal discipline.)
- Hot path stays `O(visible)+O(edited)`, never `O(document)`; idle is free — background work is edge-triggered (armed slots), never wall-clock polled.
- All `DiagnosticsProvider` methods are non-blocking (`cmd_tx.send`); the blocking `recv` lives in each provider's worker thread.
- **Dispatchers delegate — no hub growth.** New behavior enters through a registration seam / domain module, not by growing `reduce`/`on_tick`/`render`. New commands are rows in the existing registration block; reduce/timers arms stay one-line delegations.
- **`clippy::too_many_lines` (threshold 100) and `wordcartel/tests/module_budgets.rs` hub budgets are GATEs.** `app.rs` budget 1000, `render.rs` 900, `timers.rs` 400. Keep new/split fns under 100 lines; where a fn would exceed it, split (named per task) or carry an item-local `#[allow(clippy::too_many_lines)]` with a one-line reason for a genuinely-flat dispatch.
- No `.unwrap()` on fallible/external paths; `.expect("…invariant…")` after establishing an invariant.
- **Hand-formatted dense style — do NOT run `cargo fmt`.** Match neighbors by hand; don't reflow untouched code. Em-dash `—` in prose comments, never `--`. No emoji in code.
- Private struct fields by default; accessors / validated constructors. `Option<T>` over sentinels. Typed error enums surfaced to the STATUS LINE (never the console).

**Merge GATEs (verified after the final task; each task ends green):**
- `cargo test` green across all suites (`wordcartel-core` lib + oracle, `wordcartel` lib).
- `cargo build` and `cargo test --no-run` warning-free for touched crates.
- `cargo clippy --workspace --all-targets` clean (`[workspace.lints.clippy] all = "deny"`).
- Command-surface invariant tests green (palette-completeness, every-persisted-setting-has-a-command, hint re-resolution) — see §12 conformance notes per task.
- PTY smoke suite `scripts/smoke/run.sh` — mandatory-run, advisory-pass (quote its one-line summary in the pre-merge report; a red result is an advisory finding, never a merge block).

**Commit trailers (every commit ends with, verbatim):**
```
Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
Claude-Session: <current session URL>
```

**Do NOT** commit/push beyond each task's own commit; the branch merges only when the coordinator says so. Never edit `BACKLOG.md`. For compile/usage/signature questions on code you are editing, trust `cargo` + `grep`, not an editor "unused"/"undefined" hint.

---

## Compiles-green sequencing (the critical constraint, resolved)

Adding `source`/`code`/`href` to `Diagnostic` breaks **31 construction literals** (§13.1 corrected: app 10, save 5, render 4, mouse 3, registry 3, harper_ls 1, search_ui 2, diag_overlay 1, e2e 1, diagnostics_run 1) **+ 2 helper-fn bodies** (`spelling`/`grammar`) **plus** both `Msg` shapes (`DiagnosticsDone`, `DiagProviderEvent`) **plus** the two apply-fn signatures **and their 8 callers** (2 production delivery paths `app.rs::reduce_dispatch` + `prompts.rs::intercept`, plus 6 `apply_diagnostics_done` test callers) — the tree will NOT compile until every site is fixed.

**Resolution: Task 1 is a single ATOMIC foundation** that lands the core-type widening, both message-shape updates, the two apply-fn signature changes, and ALL 31 construction-site migrations in one commit — ending on a compiling, green tree. Splitting the field-add from the migrations would leave 30+ broken sites straddling a task boundary (no intermediate green tree possible), and per-field temporary `Default` scaffolding would be pure throwaway churn (the fields have no sensible runtime default at the harper construction site — it must set real `source: DiagSource::Harper`). Every later task (2–8) is a behavioral generalization that compiles green on top of Task 1's widened vocabulary. Task 1 introduces `DiagSource` and threads a literal `DiagSource::Harper` through the existing single-provider paths **without yet** changing provider/store/dispatch structure — so it is mechanical and self-contained.

---

## Task list (titles + one-line + dependencies)

1. **Core-type widening + full construction-site migration (atomic foundation)** — add `DiagSource` + `Diagnostic.{source,code,href}`, update both `Msg` shapes + both apply-fn signatures + both delivery paths + all 31 literals, harper sets real source/code/href. Deps: none.
2. **`ProviderSet` registry + trait `source()`/`install_hint()`; delete `NullProvider`** — replace `Editor.diag_provider: Box<dyn>` with `diag_providers: ProviderSet`; move `INSTALL_HINT` to harper_ls. Deps: 1.
3. **Source-partitioned `DiagStore` (`SourceSlot` + `BTreeMap`)** — per-source slots, `arm_enabled`, `any_due`/`due_sources`/`due_deadline`; migrate all slot-access sites. Deps: 1, 2 (`arm_enabled` uses `ProviderSet::enabled_sources`).
4. **Source-routed marshaling** — `apply_diagnostics_done(+source)` routes to the slot; `apply_provider_event(+source)`; per-source hint latch (`diag_hint_shown: BTreeSet<DiagSource>`). Deps: 2, 3.
5. **Dispatch fan-out (`dispatch_diagnostics(editor, now)` + `dispatch_one`)** + timers row (`due_deadline`/`any_due`). Deps: 2, 3, 4.
6. **The switchable lens: `active_analysis_source` + `set_analysis_source` + `active_lens_diags` + lens-routed render/nav/status.** Deps: 2, 3.
7. **Lens/enable commands + `cycle_analysis_source` + `set_engine_enabled`** (§8.5, §12 conformance). Deps: 2, 3, 6.
8. **Config wiring + `install_core_providers`** — consume `linters`, `[diagnostics.harper]` table, warns-before-status ordering. Deps: 2, 3, 5, 7.
9. **Two-provider mock proof (§14.1's 9 cases) + e2e lens journey.** Deps: 2–8.

Tasks 1→2→3 are the spine; 4 and 6 both depend on 2+3 and are independently reviewable; 5 needs 4; 7 needs 6; 8 needs 5+7; 9 is the whole-effort proof. Execute in numeric order (safe topological order).

---

## Task 1: Core-type widening + full construction-site migration (atomic foundation)

**Files:**
- Modify: `wordcartel-core/src/diagnostics.rs` (add `DiagSource`; widen `Diagnostic`)
- Modify: `wordcartel/src/app.rs` (both `Msg` variant defs + both `Debug`-impl arms + both reduce arms — see the Step-4 Msg-site inventory; 10 test literals; the 2 `apply_diagnostics_done` test callers; the `reduce_delivers_diag_provider_event_to_status` test construct)
- Modify: `wordcartel/src/prompts.rs` (both `intercept` arms; the modal-lifecycle test `DiagProviderEvent` construct; the `INSTALL_HINT` test import stays on `diag_provider` for now — Task 2 moves the const)
- Modify: `wordcartel/src/diag_provider.rs` (`apply_provider_event` signature +`source`)
- Modify: `wordcartel/src/diagnostics_run.rs` (`apply_diagnostics_done` signature +`source`; 1 test literal + 2 helper fns `spelling`/`grammar`)
- Modify: `wordcartel/src/harper_ls.rs` (11 `DiagnosticsDone` production emits +`source: DiagSource::Harper`; 3 `DiagProviderEvent` emits → struct; the `dones` full-destructure extractor +`source`; the flush_guard `if let` full destructure +`source`; the `restarted?`/`degraded` `DiagProviderEvent` helper patterns → struct; the five `..` `DiagnosticsDone` assertion matches are compile-safe, left as-is; `convert_diagnostics` sets real `source`/`code`/`href`)
- Modify: `wordcartel/src/save.rs` (5 test literals + 3 `apply_diagnostics_done` test callers), `render.rs` (4), `mouse.rs` (3), `registry.rs` (3), `search_ui.rs` (2), `diag_overlay.rs` (1), `e2e.rs` (1 test literal + the `diagnostics_probe` `DiagnosticsDone` construct +`source`)
- Modify: `wordcartel/tests/harper_ls_integration.rs` (pattern-match +`source`)
- Test: inline `#[cfg(test)]` in `wordcartel-core/src/diagnostics.rs`

**Interfaces:**
- Produces:
  - `pub enum DiagSource { Harper, LTeX, Vale, Plugin(&'static str) }` — `#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]`; `fn label(self) -> &'static str`; `fn config_name(self) -> &'static str`.
  - `Diagnostic { range, kind, source: DiagSource, code: Option<String>, href: Option<String>, message, suggestions }` (all `pub`).
  - `Msg::DiagnosticsDone { buffer_id, version, source: DiagSource, diagnostics }`; `Msg::DiagProviderEvent { source: DiagSource, event: ProviderEvent }`.
  - `apply_diagnostics_done(editor, buffer_id, version, source, diagnostics)`; `apply_provider_event(editor, source, event, clock)` — signatures only (bodies still single-store this task).

- [ ] **Step 1: Write the failing test** (in `wordcartel-core/src/diagnostics.rs` `#[cfg(test)] mod tests`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diag_source_labels_and_config_names_are_exhaustive() {
        assert_eq!(DiagSource::Harper.label(), "Harper");
        assert_eq!(DiagSource::LTeX.label(), "LTeX");
        assert_eq!(DiagSource::Vale.label(), "vale");
        assert_eq!(DiagSource::Plugin("mock").label(), "mock");
        assert_eq!(DiagSource::Harper.config_name(), "harper");
        assert_eq!(DiagSource::LTeX.config_name(), "ltex");
        assert_eq!(DiagSource::Vale.config_name(), "vale");
        assert_eq!(DiagSource::Plugin("x").config_name(), "x");
    }

    #[test]
    fn diagnostic_carries_source_code_href() {
        let d = Diagnostic {
            range: 0..3, kind: DiagnosticKind::Spelling, source: DiagSource::Harper,
            code: Some("SpellCheck".into()), href: None, message: "m".into(), suggestions: vec![],
        };
        assert_eq!(d.source, DiagSource::Harper);
        assert_eq!(d.code.as_deref(), Some("SpellCheck"));
        assert_eq!(d.href, None);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p wordcartel-core diagnostics::tests 2>&1 | tail -20`
Expected: FAIL — `DiagSource` not found / missing fields on `Diagnostic`.

- [ ] **Step 3: Widen the core type** (`wordcartel-core/src/diagnostics.rs`)

Add above `Diagnostic` (exact code — §3.1/§3.2):

```rust
/// The engine that produced a diagnostic — the namespace tag behind the per-engine "separate
/// views, never merged" contract. An exhaustive enum, not a free-form string: match sites are
/// forced to place every engine (the `SemanticElement` exhaustive-literal discipline), and an
/// invalid source is unrepresentable. `Plugin` carries a static name for non-core engines
/// (future plugin-declared providers; the test mock uses `Plugin("mock")`).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum DiagSource {
    /// harper-ls — the bundled core provider (Effort A).
    Harper,
    /// ltex-ls-plus — reserved vocabulary; provider ships in the ltex/vale effort.
    LTeX,
    /// vale / vale-ls — reserved vocabulary; provider ships in the ltex/vale effort.
    Vale,
    /// A non-core engine, named statically (plugin-declared engines; test mocks).
    Plugin(&'static str),
}

impl DiagSource {
    /// Human-facing label — the status bar (`REVIEW · Harper`), the lens-cycle status, and the
    /// menu state-in-label all use this. `&'static str` on purpose: feeds `MenuMark::Value`.
    pub fn label(self) -> &'static str {
        match self {
            DiagSource::Harper => "Harper",
            DiagSource::LTeX => "LTeX",
            DiagSource::Vale => "vale",
            DiagSource::Plugin(name) => name,
        }
    }
    /// Config-surface name — the `[diagnostics] linters` entry and the `[diagnostics.<engine>]`
    /// table key for this engine.
    pub fn config_name(self) -> &'static str {
        match self {
            DiagSource::Harper => "harper",
            DiagSource::LTeX => "ltex",
            DiagSource::Vale => "vale",
            DiagSource::Plugin(name) => name,
        }
    }
}
```

Widen `Diagnostic` (insert `source`/`code`/`href` between `kind` and `message`, per §3.2 field order) and update the struct doc-comment to the N-provider story. Keep `#[derive(Clone, PartialEq, Eq, Debug)]`.

- [ ] **Step 4: Migrate the two message shapes + both delivery paths + apply signatures**

`app.rs` — `Msg::DiagnosticsDone` gains `source: wordcartel_core::diagnostics::DiagSource` (between `version` and `diagnostics`); `Msg::DiagProviderEvent(crate::diag_provider::ProviderEvent)` becomes `DiagProviderEvent { source: wordcartel_core::diagnostics::DiagSource, event: crate::diag_provider::ProviderEvent }`. Update the `Debug` impl: `DiagnosticsDone` adds `.field("source", source)`; `DiagProviderEvent` becomes `f.debug_struct("DiagProviderEvent").field("source", source).field("event", event).finish()`.

`app.rs::reduce_dispatch` arms:
```rust
Msg::DiagnosticsDone { buffer_id, version, source, diagnostics } => {
    crate::diagnostics_run::apply_diagnostics_done(editor, buffer_id, version, source, diagnostics);
}
Msg::DiagProviderEvent { source, event } =>
    crate::diag_provider::apply_provider_event(editor, source, event, clock),
```

`prompts.rs::intercept` arms — identical migration (the SECOND delivery path):
```rust
Msg::DiagnosticsDone { buffer_id, version, source, diagnostics } => {
    crate::diagnostics_run::apply_diagnostics_done(editor, buffer_id, version, source, diagnostics);
}
Msg::DiagProviderEvent { source, event } =>
    crate::diag_provider::apply_provider_event(editor, source, event, clock),
```

`diagnostics_run.rs` — `apply_diagnostics_done` gains `source: DiagSource` param (4th, before `diagnostics`); body **unchanged this task** (still writes the single `b.diagnostics.diagnostics` — Task 4 routes by source). Add `use wordcartel_core::diagnostics::DiagSource;` to the existing `use` line.

`diag_provider.rs` — `apply_provider_event(editor, source: DiagSource, ev: ProviderEvent, clock)` gains the `source` param; body **unchanged this task** (Task 4 uses it). Import `DiagSource`.

**COMPLETE `Msg`-site inventory (own EVERY construct/match/destructure — the two variant shapes change).** Governing rule:
- **`Msg::DiagnosticsDone` GAINS a field (`source`)** → sites that destructure with a trailing `..` still COMPILE unchanged; ONLY full constructs and full destructures (no `..`) must add `source`.
- **`Msg::DiagProviderEvent` changes tuple `(ProviderEvent)` → struct `{ source, event }`** → EVERY site breaks (the tuple pattern/construct is now invalid); ALL must migrate.

Grep to confirm nothing missed: `grep -rn "DiagnosticsDone\|DiagProviderEvent" wordcartel/src wordcartel/tests --include="*.rs" | grep -v "//"`.

**`DiagnosticsDone` — MUST add `source` (full construct/destructure; production emits get `DiagSource::Harper`, extractors/patterns bind it):**
| Site | Kind | Action |
|---|---|---|
| `app.rs` `Msg::DiagnosticsDone {` variant def | definition | add `source: …DiagSource` field (Step 4 above) |
| `app.rs` Debug-impl arm (`{ buffer_id, version, diagnostics }`) | full destructure | add `source` to the pattern AND `.field("source", source)` |
| `app.rs` reduce arm | full destructure | add `source` (Step 4 above) |
| `prompts.rs` intercept arm | full destructure | add `source` (Step 4 above) |
| `harper_ls.rs` ×11 production emits (269/373/378/409/425/458/466/500/504/509/667) | full construct | add `source: DiagSource::Harper` (Step 5 below) |
| `harper_ls.rs` `dones` test-helper extractor (`{ buffer_id, version, diagnostics }`) | full destructure | add `source` to the pattern (Step 5 below) |
| `harper_ls.rs` flush_guard test `if let Msg::DiagnosticsDone { buffer_id, version, diagnostics }` | full destructure (NO `..`) | **add `source`** |
| `e2e.rs` `diagnostics_probe` construct (`{ buffer_id: bid, version: ver, diagnostics: diags }`) | full construct | **add `source: DiagSource::Harper`** |
| `tests/harper_ls_integration.rs` `Ok(Msg::DiagnosticsDone { buffer_id, version, diagnostics })` | full destructure | add `source` (Step 6 below) |

**`DiagnosticsDone` — COMPILE-SAFE (use `..` — DO NOT touch unless the test asserts on `source`):** `harper_ls.rs` five `..` sites (the `{ version: 5, .. }`, three `{ buffer_id, version, .. }` assertion matches, and the `filter_map` `{ buffer_id, version, .. }`). The `..` absorbs the new field; leave them.

**`DiagProviderEvent` — EVERY site migrates (tuple → struct):**
| Site | Kind | Action |
|---|---|---|
| `app.rs` `DiagProviderEvent(…)` variant def | definition | tuple → `{ source: …DiagSource, event: …ProviderEvent }` (Step 4 above) |
| `app.rs` Debug-impl arm (`(ev)` → `debug_tuple`) | pattern + body | `{ source, event }` → `debug_struct("DiagProviderEvent").field("source", source).field("event", event).finish()` (Step 4 above) |
| `app.rs` reduce arm | pattern | `{ source, event }` (Step 4 above) |
| `prompts.rs` intercept arm | pattern | `{ source, event }` (Step 4 above) |
| `harper_ls.rs` ×3 production emits (484 Restarted / 488 Degraded CRASHED_HINT / 724 Degraded INSTALL_HINT) | construct | `{ source: DiagSource::Harper, event: ProviderEvent::… }` (Step 5 below) |
| `harper_ls.rs` `restarted?` test helper `matches!(…, …DiagProviderEvent(ProviderEvent::Restarted))` | pattern | `…DiagProviderEvent { event: ProviderEvent::Restarted, .. }` (Step 5 below) |
| `harper_ls.rs` `degraded` test helper `DiagProviderEvent(ProviderEvent::Degraded(h))` | pattern | `…DiagProviderEvent { event: ProviderEvent::Degraded(h), .. }` (Step 5 below) |
| `app.rs` `reduce_delivers_diag_provider_event_to_status` test construct | construct | **`{ source: DiagSource::Harper, event: ProviderEvent::Degraded(INSTALL_HINT.into()) }`** |
| `prompts.rs` modal-lifecycle test construct | construct | **`{ source: DiagSource::Harper, event: ProviderEvent::Degraded(INSTALL_HINT.into()) }`** |

(`tests/harper_ls_integration.rs` matches `DiagProviderEvent` only via a wildcard `Ok(_) => continue` — compile-safe, no change.) The two `app.rs`/`prompts.rs` test `INSTALL_HINT` imports move to `crate::harper_ls` in **Task 2** (CRITICAL-1 fold); in THIS task they still import from `diag_provider` — migrate only the `Msg` shape here.

- [ ] **Step 5: Set real source/code/href in the harper producer + tag all emit sites**

`harper_ls.rs::convert_diagnostics` — set the new fields from real wire data (§4.4). Hoist the `code` extraction (mirror `classify_lsp`'s `d.get("code")`) so it is read once:
```rust
let code = match d.get("code") {
    Some(Value::String(s)) => Some(s.clone()),
    Some(other) => Some(other.to_string()),
    None => None,
};
let href = d.get("codeDescription").and_then(|c| c.get("href"))
    .and_then(|h| h.as_str()).map(str::to_string);
out.push(Diagnostic { range, kind, source: DiagSource::Harper, code, href, message,
    suggestions: Vec::new() });
```
All **11** `Msg::DiagnosticsDone { … }` construction sites in `harper_ls.rs` gain `source: DiagSource::Harper` (the close-terminal, publish-empty/assembly, codeAction stale/assembled, deadline watchdogs ×2, on_server_gone flush ×2 + queued, `FlushGuard::drop` drain — grep `DiagnosticsDone {` in the file to enumerate). The **3** `Msg::DiagProviderEvent(...)` sites become `Msg::DiagProviderEvent { source: DiagSource::Harper, event: ... }`. Add `use wordcartel_core::diagnostics::DiagSource;`. Update the `#[cfg(test)]` helper extractors (`dones`/`degraded` that pattern-match these `Msg`) to the new shapes.

- [ ] **Step 6: Migrate all remaining construction literals + the integration test**

Add `source: DiagSource::Harper, code: None, href: None` (import `DiagSource` where needed) to every `Diagnostic { … }` literal. **Do NOT trust these per-file counts as a checklist — GREP each file and fix every hit** (the counts are a sanity total, matching the corrected spec §13.1): `app.rs` (**10**), `save.rs` (5), `render.rs` (4), `mouse.rs` (3), `registry.rs` (3), `search_ui.rs` (**2**), `diag_overlay.rs` (1, in `tall_diag`), `e2e.rs` (1, in `make_diags`), and `diagnostics_run.rs` (1 test literal + the 2 helper fns `spelling`/`grammar` — set `source: DiagSource::Harper` in each helper body) — **31 struct literals + 2 helper-fn bodies** total. `tests/harper_ls_integration.rs` — its `Ok(Msg::DiagnosticsDone { buffer_id, version, diagnostics })` match adds `source` (bind and assert `DiagSource::Harper`).

**Also migrate the `apply_diagnostics_done` TEST callers** (distinct from the `Diagnostic{}` literals — these are apply-fn calls whose signature changed in Step 4, so they break compile without the new `source` arg). Add `DiagSource::Harper` as the 4th arg (before the `diagnostics` vec) at all six sites: `diagnostics_run.rs` (`apply_filters_ignored_spelling_over_the_union_keeps_grammar`), `app.rs` (the two calls in the version-gate test — current-version + stale-version), `save.rs` (the three reload/recovery-stale-diagnostics tests). (Task 4 later installs an enabled provider at these same six sites — see Task 4 Step 4.)

Grep to confirm none missed. Struct literals: `grep -rn "Diagnostic {" wordcartel-core/src wordcartel/src wordcartel/tests --include="*.rs" | grep -v "DiagnosticKind\|DiagnosticsConfig\|DiagnosticsDone\|DiagnosticsProvider\|-> Diagnostic\|struct Diagnostic"` — the `-> Diagnostic` exclusion drops the two helper-fn return-type false-positives; every remaining hit must carry the new fields. Apply-fn callers: `grep -rn "apply_diagnostics_done(" wordcartel/src` — every call passes a `source`.

- [ ] **Step 7: Run the core test + full build to verify green**

Run: `cargo test -p wordcartel-core diagnostics::tests 2>&1 | tail -20` → PASS.
Run: `cargo build 2>&1 | tail -30` → clean (no errors, no warnings).
Run: `cargo test --no-run 2>&1 | tail -20` → clean.
Run: `cargo clippy --workspace --all-targets 2>&1 | tail -20` → clean.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit  # message: "feat(diag): widen Diagnostic with DiagSource/code/href; migrate all sites"
```

**Done =** core test green; whole tree compiles + all existing suites green; clippy clean; committed. Behavior unchanged (single provider still installed; `source` is always `Harper`).

**§12 conformance:** N/A — Task 1 touches no command, option, palette, menu, or hint.

---

## Task 2: `ProviderSet` registry + trait `source()`/`install_hint()`; delete `NullProvider`

**Files:**
- Modify: `wordcartel/src/diag_provider.rs` (trait; `ProviderSet`; delete `NullProvider`; `RecordingProvider` gains `source`)
- Modify: `wordcartel/src/harper_ls.rs` (`source()`/`install_hint()`; `INSTALL_HINT` const lands here)
- Modify: `wordcartel/src/editor.rs` (`diag_provider` field → `diag_providers: ProviderSet`; constructor)
- Modify: `wordcartel/src/app.rs` (install → `diag_providers.install(...)`; loop-exit `shutdown_all`)
- Modify: `wordcartel/src/save.rs` + `workspace.rs` (`notify_close` → `notify_close_all`)
- Modify: `wordcartel/src/search_ui.rs` (`reload_dictionary` → `reload_dictionary_enabled`)
- Modify: `wordcartel/src/render_status.rs`, `diagnostics_run.rs::dispatch_diagnostics`, `plugin/host.rs` doc-comments — call-site updates (see Step 5)
- Test: inline `#[cfg(test)]` in `diag_provider.rs`

**Interfaces:**
- Consumes: `DiagSource` (Task 1).
- Produces:
  - trait method `fn source(&self) -> DiagSource;` (replaces `name`) and `fn install_hint(&self) -> &'static str;`.
  - `pub struct ProviderSet` with the §4.2 API: `install`, `sources`, `enabled_sources`, `is_enabled`, `set_enabled`, `availability(source) -> Option<Availability>`, `install_hint(source) -> Option<&'static str>`, `ensure_running(source)`, `notify_change(source, buffer_id, version, path, text) -> Accepted`, `configure(source, cfg)`, `notify_close_all(buffer_id)`, `reload_dictionary_enabled()`, `shutdown_all()`.
  - `Editor.diag_providers: ProviderSet`.
  - `pub const INSTALL_HINT: &str` now in `harper_ls.rs`.

- [ ] **Step 1: Write the failing tests** (`diag_provider.rs` `#[cfg(test)]`)

```rust
#[test]
fn provider_set_registers_and_reports_enabled() {
    let mut set = ProviderSet::default();
    set.install(Box::new(RecordingProvider::new().with_source(DiagSource::Harper)), true);
    set.install(Box::new(RecordingProvider::new().with_source(DiagSource::Plugin("mock"))), false);
    assert_eq!(set.sources().collect::<Vec<_>>(), vec![DiagSource::Harper, DiagSource::Plugin("mock")]);
    assert_eq!(set.enabled_sources().collect::<Vec<_>>(), vec![DiagSource::Harper]);
    assert!(set.is_enabled(DiagSource::Harper));
    assert!(!set.is_enabled(DiagSource::Plugin("mock")));
    assert!(set.set_enabled(DiagSource::Plugin("mock"), true));
    assert!(set.is_enabled(DiagSource::Plugin("mock")));
    assert!(!set.set_enabled(DiagSource::Vale, true), "unknown source → false");
}

#[test]
fn provider_set_source_keyed_delegation() {
    let mut set = ProviderSet::default();
    let rec = RecordingProvider::new().with_source(DiagSource::Harper);
    let calls = rec.calls_handle();
    set.install(Box::new(rec), true);
    set.ensure_running(DiagSource::Harper);
    assert_eq!(set.availability(DiagSource::Harper), Some(Availability::Ready));
    assert_eq!(set.availability(DiagSource::Vale), None, "unknown source → None");
    let a = set.notify_change(DiagSource::Harper, BufferId(1), 3, None, "t".into());
    assert_eq!(a, Accepted::Yes);
    assert_eq!(set.notify_change(DiagSource::Vale, BufferId(1), 3, None, "t".into()), Accepted::No,
        "unknown source never latches");
    assert!(calls.lock().unwrap().iter().any(|c| matches!(c, ProviderCall::EnsureRunning)));
}
```

- [ ] **Step 2: Run to verify fail**

Run: `cargo test -p wordcartel diag_provider 2>&1 | tail -20` → FAIL (`ProviderSet`/`with_source` undefined).

- [ ] **Step 3: Move `INSTALL_HINT`, change the trait, add `ProviderSet`, delete `NullProvider`**

**`INSTALL_HINT` const move — repoint EVERY reference in THIS task (CRITICAL: the const is used across 5 files; deleting it from `diag_provider.rs` without repointing all of them breaks compile).** In `harper_ls.rs` add `pub const INSTALL_HINT: &str = "grammar checker unavailable — install harper-ls (Arch: pacman -S harper)";` (verbatim text moved from `diag_provider.rs`); update `harper_ls.rs`'s own `INSTALL_HINT` use at its import line and its degrade-send site to the local const (drop it from the `use crate::diag_provider::{…}` list). Then repoint every other reference to `crate::harper_ls::INSTALL_HINT` **in this task** — verified sites (grep `INSTALL_HINT` to confirm none missed):
- **Production** `diagnostics_run.rs::show_install_hint` — `editor.status = crate::diag_provider::INSTALL_HINT.into()` → `crate::harper_ls::INSTALL_HINT.into()`. **Resolution (a):** this interim repoint keeps `show_install_hint` compiling until Task 4 refactors its body to `editor.diag_providers.install_hint(source)` (the §7.3 refactor stays in Task 4 — NOT moved here). No re-export is added.
- `diagnostics_run.rs` test import (`use crate::diag_provider::{…, INSTALL_HINT}`) + its two asserts → import from `crate::harper_ls`.
- `diag_provider.rs`'s own two `INSTALL_HINT` test uses are deleted together with `NullProvider`'s tests? No — they live in `apply_provider_event`'s Degraded test, which survives; repoint them to `crate::harper_ls::INSTALL_HINT`.
- `prompts.rs` test (`use crate::diag_provider::{ProviderEvent, INSTALL_HINT}` + two uses) → import `INSTALL_HINT` from `crate::harper_ls` (the `ProviderEvent`/`Msg` shape for that test already migrated in Task 1).
- `app.rs` test (`use crate::diag_provider::{ProviderEvent, INSTALL_HINT}` + two uses) → import `INSTALL_HINT` from `crate::harper_ls`.

In `diag_provider.rs`: delete the `INSTALL_HINT` const and `NullProvider` (struct + impl + its two unit tests). Change the trait — replace `fn name(&self) -> &'static str;` with `fn source(&self) -> DiagSource;` and add `fn install_hint(&self) -> &'static str;`. Add:

```rust
/// The registered diagnostic engines, identified by `DiagSource`. Insertion order is the lens
/// cycle order (core catalog order — harper first). Hermetic default: empty (no thread, no
/// process, no emissions — the role `NullProvider` used to play).
#[derive(Debug, Default)]
pub struct ProviderSet { entries: Vec<ProviderEntry> }

#[derive(Debug)]
struct ProviderEntry { enabled: bool, provider: Box<dyn DiagnosticsProvider> }

impl ProviderSet {
    /// Register an engine. Duplicate sources are a wiring bug (cold startup path).
    pub fn install(&mut self, provider: Box<dyn DiagnosticsProvider>, enabled: bool) {
        let src = provider.source();
        assert!(!self.entries.iter().any(|e| e.provider.source() == src),
            "duplicate diagnostics provider source: {src:?}");
        self.entries.push(ProviderEntry { enabled, provider });
    }
    pub fn sources(&self) -> impl Iterator<Item = DiagSource> + '_ {
        self.entries.iter().map(|e| e.provider.source())
    }
    pub fn enabled_sources(&self) -> impl Iterator<Item = DiagSource> + '_ {
        self.entries.iter().filter(|e| e.enabled).map(|e| e.provider.source())
    }
    pub fn is_enabled(&self, source: DiagSource) -> bool {
        self.entries.iter().any(|e| e.provider.source() == source && e.enabled)
    }
    pub fn set_enabled(&mut self, source: DiagSource, on: bool) -> bool {
        match self.entries.iter_mut().find(|e| e.provider.source() == source) {
            Some(e) => { e.enabled = on; true }
            None => false,
        }
    }
    fn get_mut(&mut self, source: DiagSource) -> Option<&mut Box<dyn DiagnosticsProvider>> {
        self.entries.iter_mut().find(|e| e.provider.source() == source).map(|e| &mut e.provider)
    }
    fn get(&self, source: DiagSource) -> Option<&Box<dyn DiagnosticsProvider>> {
        self.entries.iter().find(|e| e.provider.source() == source).map(|e| &e.provider)
    }
    pub fn availability(&self, source: DiagSource) -> Option<Availability> {
        self.get(source).map(|p| p.availability())
    }
    pub fn install_hint(&self, source: DiagSource) -> Option<&'static str> {
        self.get(source).map(|p| p.install_hint())
    }
    pub fn ensure_running(&mut self, source: DiagSource) {
        if let Some(p) = self.get_mut(source) { p.ensure_running(); }
    }
    pub fn notify_change(&mut self, source: DiagSource, buffer_id: BufferId, version: u64,
        path: Option<std::path::PathBuf>, text: String) -> Accepted {
        match self.get_mut(source) {
            Some(p) => p.notify_change(buffer_id, version, path, text),
            None => Accepted::No,
        }
    }
    pub fn configure(&mut self, source: DiagSource, cfg: ProviderConfig) {
        if let Some(p) = self.get_mut(source) { p.configure(cfg); }
    }
    pub fn notify_close_all(&mut self, buffer_id: BufferId) {
        for e in self.entries.iter_mut() { e.provider.notify_close(buffer_id); }
    }
    pub fn reload_dictionary_enabled(&mut self) {
        for e in self.entries.iter_mut().filter(|e| e.enabled) { e.provider.reload_dictionary(); }
    }
    pub fn shutdown_all(&mut self) {
        for e in self.entries.iter_mut() { e.provider.shutdown(); }
    }
}
```
Add `use wordcartel_core::diagnostics::DiagSource;`. Note: `clippy::borrowed_box` may fire on the private `get`/`get_mut` returning `&Box<dyn …>` — if so, return `&dyn DiagnosticsProvider` / use an item-local `#[allow(clippy::borrowed_box)]` with a one-line reason, OR (preferred) have `get`/`get_mut` return the `&ProviderEntry` and call through it. Verify with `cargo clippy`.

`RecordingProvider`: add `source: DiagSource` field (default `DiagSource::Plugin("recording")` in `new()`), a `with_source(mut self, source) -> Self` builder, `fn source(&self) -> DiagSource { self.source }`, and `fn install_hint(&self) -> &'static str { "test provider unavailable" }`. Update its `impl DiagnosticsProvider` (remove `name`).

`HarperLs`: `fn source(&self) -> DiagSource { DiagSource::Harper }`; `fn install_hint(&self) -> &'static str { INSTALL_HINT }`; remove `fn name`. Its `#[cfg(test)]` `assert_eq!(p.name(), "Harper")` becomes `assert_eq!(p.source(), DiagSource::Harper)`.

- [ ] **Step 4: Repoint the `Editor` field + construction + install + teardown**

`editor.rs`: `pub diag_provider: Box<dyn crate::diag_provider::DiagnosticsProvider>` → `pub diag_providers: crate::diag_provider::ProviderSet`; in `new_from_text`, `diag_provider: Box::new(crate::diag_provider::NullProvider)` → `diag_providers: crate::diag_provider::ProviderSet::default()`.

`app.rs::run`: the harper install block becomes (unchanged config derivation, wrapped in `install`):
```rust
editor.diag_providers.install(Box::new(crate::harper_ls::HarperLs::new(
    msg_tx.clone(),
    crate::diag_provider::ProviderConfig {
        grammar: cfg.diagnostics.grammar,
        dictionary: cfg.diagnostics.dictionary.clone(),
        max_file_length: crate::limits::HARPER_MAX_FILE_LENGTH,
    })), true);
```
(Task 8 replaces this whole block with `install_core_providers`; this interim keeps the tree green.) Loop-exit `editor.borrow_mut().diag_provider.shutdown()` → `editor.borrow_mut().diag_providers.shutdown_all()`.

- [ ] **Step 5: Update the remaining single-provider call sites (interim, source = Harper)**

Each `editor.diag_provider.X` becomes a `diag_providers` call keyed on `DiagSource::Harper` (Tasks 5/6 generalize these):
- `diagnostics_run.rs::dispatch_diagnostics`: `editor.diag_provider.ensure_running()` → `editor.diag_providers.ensure_running(DiagSource::Harper)`; the two `availability()` reads → `editor.diag_providers.availability(DiagSource::Harper)` (now `Option` — match `Some(Availability::Unavailable)`/`Some(Availability::Starting)`, treat `None` as unavailable); `notify_change(...)` → `editor.diag_providers.notify_change(DiagSource::Harper, ...)`.
- `save.rs` (2×) + `workspace.rs` (1×): `editor.diag_provider.notify_close(id)` → `editor.diag_providers.notify_close_all(id)`.
- `search_ui.rs`: `editor.diag_provider.reload_dictionary()` → `editor.diag_providers.reload_dictionary_enabled()`.
- `render_status.rs::status_left_text`: `editor.diag_provider.availability() == Ready` → `editor.diag_providers.availability(DiagSource::Harper) == Some(Availability::Ready)`; `editor.diag_provider.name()` → `DiagSource::Harper.label()`. (Task 6 makes this follow the lens.) Update the `render_status.rs` tests that build `e.diag_provider = Box::new(RecordingProvider…)` → `e.diag_providers.install(Box::new(RecordingProvider::new().with_source(DiagSource::Harper).with_availability(...)), true)`.
- `diagnostics_run.rs` + `app.rs` tests that assign `e.diag_provider = Box::new(RecordingProvider::new())` → `e.diag_providers.install(Box::new(RecordingProvider::new().with_source(DiagSource::Harper)…), true)`; their `calls_handle()` pattern is unchanged.
- `plugin/host.rs`: the two doc-comments citing `NullProvider` → "the empty `ProviderSet` (hermetic default)".

- [ ] **Step 6: Run + green**

Run: `cargo test -p wordcartel diag_provider 2>&1 | tail -20` → PASS.
Run: `cargo build 2>&1 | tail -30` && `cargo clippy --workspace --all-targets 2>&1 | tail -20` → clean.
Run: `cargo test 2>&1 | tail -20` → all suites green.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit  # "refactor(diag): ProviderSet registry; source()/install_hint(); drop NullProvider"
```

**Done =** new + migrated tests green; whole tree green; clippy clean; committed.

**§12 conformance:** N/A — no command/option/palette/menu/hint change (the status label still shows harper; Task 6 makes it lens-aware). Module budgets: `ProviderSet` methods are all small; `app.rs` unchanged size. No hub grew.

---

## Task 3: Source-partitioned `DiagStore` (`SourceSlot` + `BTreeMap`)

**Files:**
- Modify: `wordcartel/src/diagnostics_run.rs` (`DiagStore` → map of `SourceSlot`; `arm_enabled`, `any_due`/`due_sources`/`due_deadline`; `retain_unignored` over all slots; migrate `dispatch_diagnostics`/`apply_diagnostics_done` slot access to `DiagSource::Harper` interim)
- Modify: `wordcartel/src/editor.rs` (`set_render_mode` arm call; `on_reenter` arm at ~line 991 uses `arm_enabled`)
- Modify: every slot-field access site: `save.rs`, `render.rs`, `registry.rs`, `app.rs`, `timers.rs`, `search_ui.rs`, `e2e.rs`, `diag_provider.rs` tests
- Test: inline `#[cfg(test)]` in `diagnostics_run.rs`

**Interfaces:**
- Consumes: `DiagSource` (Task 1); `ProviderSet::enabled_sources` (Task 2 — used by `arm_enabled`).
- Produces:
  - `pub struct SourceSlot { pub diagnostics: Vec<Diagnostic>, pub computed_version: u64, pub recheck_due_at: Option<u64>, pub in_flight_version: Option<u64> }` with `fn valid_for(&self, version) -> bool`, `fn arm(&mut self, now, debounce_ms)`.
  - `pub struct DiagStore { slots: BTreeMap<DiagSource, SourceSlot> }` with `new`, `slot(source) -> Option<&SourceSlot>`, `slot_mut(source) -> &mut SourceSlot`, `clear_source(source)`, `any_due(now) -> bool`, `due_sources(now) -> impl Iterator`, `due_deadline() -> Option<u64>`.
  - `pub fn arm_enabled(editor, now, debounce_ms)`.

- [ ] **Step 1: Write the failing tests** (`diagnostics_run.rs` `#[cfg(test)]`)

```rust
#[test]
fn source_slots_are_independent() {
    let mut s = DiagStore::new();
    s.slot_mut(DiagSource::Harper).arm(1000, 400);
    assert_eq!(s.slot(DiagSource::Harper).unwrap().recheck_due_at, Some(1400));
    assert!(s.slot(DiagSource::Plugin("mock")).is_none(), "untouched source has no slot");
    s.slot_mut(DiagSource::Plugin("mock")).arm(1000, 100);
    assert_eq!(s.due_deadline(), Some(1100), "earliest armed deadline across slots");
    assert!(s.any_due(1100) && !s.any_due(1099));
    assert_eq!(s.due_sources(1400).collect::<Vec<_>>(),
        vec![DiagSource::Harper, DiagSource::Plugin("mock")]); // BTreeMap order
}

#[test]
fn due_deadline_excludes_in_flight_slot() {
    let mut s = DiagStore::new();
    s.slot_mut(DiagSource::Harper).arm(1000, 400);
    s.slot_mut(DiagSource::Harper).in_flight_version = Some(7);
    assert_eq!(s.due_deadline(), None, "an in-flight slot never re-drives the deadline");
    assert!(!s.any_due(2000));
}

#[test]
fn arm_enabled_arms_only_enabled_sources() {
    use crate::editor::{Editor, RenderMode};
    let mut e = Editor::new_from_text("x\n", None, (40, 10));
    e.diag_providers.install(Box::new(
        crate::diag_provider::RecordingProvider::new().with_source(DiagSource::Harper)), true);
    e.diag_providers.install(Box::new(
        crate::diag_provider::RecordingProvider::new().with_source(DiagSource::Plugin("mock"))), false);
    e.diag_cfg.enabled = true;
    e.active_mut().view.mode = RenderMode::Review;
    arm_enabled(&mut e, 500, 400);
    assert_eq!(e.active().diagnostics.slot(DiagSource::Harper).unwrap().recheck_due_at, Some(900));
    assert!(e.active().diagnostics.slot(DiagSource::Plugin("mock")).is_none(), "disabled: no slot");
}
```

- [ ] **Step 2: Run to verify fail**

Run: `cargo test -p wordcartel diagnostics_run 2>&1 | tail -20` → FAIL.

- [ ] **Step 3: Restructure `DiagStore`** (§5)

```rust
#[derive(Debug, Default, Clone)]
pub struct DiagStore { slots: std::collections::BTreeMap<DiagSource, SourceSlot> }

#[derive(Debug, Default, Clone)]
pub struct SourceSlot {
    pub diagnostics: Vec<Diagnostic>,
    pub computed_version: u64,
    pub recheck_due_at: Option<u64>,
    pub in_flight_version: Option<u64>,
}

impl SourceSlot {
    /// Markers paintable only when computed against the current version AND non-empty.
    pub fn valid_for(&self, version: u64) -> bool {
        !self.diagnostics.is_empty() && self.computed_version == version
    }
    /// Arm this source's re-check `debounce_ms` from `now`.
    pub fn arm(&mut self, now: u64, debounce_ms: u64) {
        self.recheck_due_at = Some(now.saturating_add(debounce_ms));
    }
}

impl DiagStore {
    pub fn new() -> Self { DiagStore::default() }
    pub fn slot(&self, source: DiagSource) -> Option<&SourceSlot> { self.slots.get(&source) }
    pub fn slot_mut(&mut self, source: DiagSource) -> &mut SourceSlot {
        self.slots.entry(source).or_default()
    }
    pub fn clear_source(&mut self, source: DiagSource) { self.slots.remove(&source); }
    /// Earliest armed deadline among slots with NO check in flight (per-source A3 gate).
    pub fn due_deadline(&self) -> Option<u64> {
        self.slots.values()
            .filter(|s| s.in_flight_version.is_none())
            .filter_map(|s| s.recheck_due_at).min()
    }
    pub fn any_due(&self, now: u64) -> bool { self.due_sources(now).next().is_some() }
    pub fn due_sources(&self, now: u64) -> impl Iterator<Item = DiagSource> + '_ {
        self.slots.iter()
            .filter(move |(_, s)| s.in_flight_version.is_none()
                && matches!(s.recheck_due_at, Some(t) if now >= t))
            .map(|(src, _)| *src)
    }
}
```
Add `arm_enabled`:
```rust
/// Arm every ENABLED engine's slot on the active buffer — the multi-provider generalization of
/// the old single `store.arm`. Callers: `arm_if_edited`, `set_render_mode`, `recheck_diagnostics`.
pub fn arm_enabled(editor: &mut Editor, now: u64, debounce_ms: u64) {
    let sources: Vec<DiagSource> = editor.diag_providers.enabled_sources().collect();
    let store = &mut editor.active_mut().diagnostics;
    for s in sources { store.slot_mut(s).arm(now, debounce_ms); }
}
```
Remove the old `diag_due` free fn (its per-slot role moves to `due_sources`). `retain_unignored`: iterate `editor.active_mut().diagnostics.slots.values_mut()` and `retain_over_union` each `slot.diagnostics` — but `slots` is private, so add a small pub helper `pub fn retain_all_slots(store: &mut DiagStore, text, union)` on `DiagStore` OR make `retain_unignored` call a new `DiagStore::retain_each(&mut self, f: impl FnMut(&mut Vec<Diagnostic>))`. Prefer: `impl DiagStore { pub fn slots_mut(&mut self) -> impl Iterator<Item = &mut SourceSlot> { self.slots.values_mut() } }` and refilter each in `retain_unignored`.

- [ ] **Step 4: Migrate all slot-field access sites to `slot(...)`/`slot_mut(...)`**

Interim: every current `X.diagnostics.{diagnostics,computed_version,recheck_due_at,in_flight_version}` and `X.diagnostics.arm(...)`/`.valid_for(...)` becomes `X.diagnostics.slot(DiagSource::Harper)` / `.slot_mut(DiagSource::Harper)`. Sites (grep `\.diagnostics\.\(diagnostics\|valid_for\|arm\|in_flight\|computed\|recheck\)`):
- `diagnostics_run.rs`: `dispatch_diagnostics` (`recheck_due_at`, `in_flight_version`), `apply_diagnostics_done` (`.diagnostics`, `.computed_version`, `.in_flight_version`), `arm_if_edited` (`.arm`), and its own tests.
- `editor.rs`: `set_render_mode`'s `self.active_mut().diagnostics.arm(now_ms, 0)` → `crate::diagnostics_run::arm_enabled(self, now_ms, 0)`; the on-reenter arm at ~L991 likewise → `arm_enabled`.
- `timers.rs`: `diag_deadline` → uses `e.active().diagnostics.due_deadline()` gated on `should_run_diagnostics` (drop the `in_flight_version.is_none()` clause — `due_deadline` already filters per-slot); `on_tick`'s diagnostics dispatch → `should_run_diagnostics(editor) && editor.active().diagnostics.any_due(now)` then `dispatch_diagnostics(editor, now)` (Task 5 finalizes the `dispatch_diagnostics` signature — this task keeps calling the current no-`now` form via a temporary shim OR order Task 5 to land the signature; see note). **Sequencing note:** to keep this task green without a throwaway shim, this task calls `dispatch_diagnostics(editor)` unchanged (still single-Harper body) and only swaps the deadline/`any_due` reads; Task 5 changes the signature + fan-out together. The `diag_due` removal is compensated by `any_due` at the timer call site.
- `render.rs`: gather `diag_active`/`diag_all` → `editor.active().diagnostics.slot(DiagSource::Harper).map_or(false, |s| s.valid_for(...))` interim (Task 6 swaps to the lens).
- `registry.rs`: `quick_fix`/`diag_next`/`diag_prev` `.diagnostics.valid_for` + `.diagnostics.diagnostics` → `.slot(DiagSource::Harper)` interim; `recheck_diagnostics` `.arm` → `arm_enabled`.
- `save.rs`/`app.rs`/`search_ui.rs`/`e2e.rs`/`diag_provider.rs`/`render_status.rs` tests: `.diagnostics.diagnostics = …` / reads → `.diagnostics.slot_mut(DiagSource::Harper).diagnostics = …` / `.slot(DiagSource::Harper)`. `save.rs`'s wholesale `new_buf.diagnostics = DiagStore::new()` is unchanged (now clears all slots).

- [ ] **Step 5: Run + green**

Run: `cargo test -p wordcartel diagnostics_run 2>&1 | tail -20` → PASS.
Run: `cargo build && cargo test 2>&1 | tail -20` → green. `cargo clippy --workspace --all-targets` → clean.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit  # "feat(diag): source-partition DiagStore into per-source SourceSlots"
```

**Done =** new + migrated tests green; whole tree green; clippy clean; committed. Behavior unchanged (only the Harper slot is ever used).

**§12 conformance:** N/A. Module budgets: `timers.rs` (budget 400) — the `diag_deadline`/`on_tick` edits are net-neutral in size; verify with `cargo test -p wordcartel --test module_budgets`.

---

## Task 4: Source-routed marshaling (apply + provider events + per-source hint latch)

**Files:**
- Modify: `wordcartel/src/diagnostics_run.rs` (`apply_diagnostics_done` body routes by source; `show_install_hint(editor, source)`)
- Modify: `wordcartel/src/diag_provider.rs` (`apply_provider_event` body uses source)
- Modify: `wordcartel/src/editor.rs` (`diag_hint_shown: bool` → `BTreeSet<DiagSource>`; `set_render_mode` clears it; constructor)
- Modify: `wordcartel/src/test_support.rs` (add the shared `install_enabled_harper` test helper)
- Modify: `wordcartel/src/diagnostics_run.rs`, `app.rs`, `save.rs` test modules (install an enabled Harper provider before the 6 pre-existing apply-on-default-editor tests — CRITICAL: the new `!is_enabled` guard drops results on a bare default editor)
- Test: inline `#[cfg(test)]` in `diagnostics_run.rs` + `diag_provider.rs`

**Interfaces:**
- Consumes: `ProviderSet` (Task 2), `SourceSlot`/`slot_mut` (Task 3).
- Produces: `apply_diagnostics_done` routing to `slot_mut(source)`; per-source hint set; `apply_provider_event` re-arming only the event's source; `test_support::install_enabled_harper(&mut Editor)`.

**Why the 6-site test migration is REQUIRED (green-sequencing).** After Task 2, `Editor::new_from_text` builds an EMPTY `ProviderSet` (`NullProvider` deleted → no engine enabled by default). This task's `apply_diagnostics_done` gains the §6.2 guard: `if !editor.diag_providers.is_enabled(source) { clear_source; return; }`. Six pre-existing tests apply a Harper result on a bare default editor and assert it lands — under the new guard those become "disabled source → dropped" and the asserts fail, so the task would not be green. Fix: install an enabled Harper provider first (Step 4a below). **PRODUCTION is unaffected** — `run()` calls `install_core_providers` (which enables harper, Task 8; and the interim Task-2 install enables it too) BEFORE the run loop, so no production `apply_diagnostics_done` ever precedes enablement; this is a test-only construction gap, not a regression.

- [ ] **Step 1: Write the failing tests**

```rust
// diagnostics_run.rs
#[test]
fn apply_routes_to_the_named_source_slot_only() {
    let mut e = review_editor("teh cat\n");
    e.diag_providers.install(Box::new(
        crate::diag_provider::RecordingProvider::new().with_source(DiagSource::Plugin("mock"))), true);
    let id = e.active().id; let v = e.active().document.version;
    apply_diagnostics_done(&mut e, id, v, DiagSource::Harper, vec![spelling(0..3)]);
    apply_diagnostics_done(&mut e, id, v, DiagSource::Plugin("mock"), vec![grammar(4..7)]);
    assert_eq!(e.active().diagnostics.slot(DiagSource::Harper).unwrap().diagnostics.len(), 1);
    assert_eq!(e.active().diagnostics.slot(DiagSource::Plugin("mock")).unwrap().diagnostics.len(), 1);
    assert_eq!(e.active().diagnostics.slot(DiagSource::Harper).unwrap().diagnostics[0].kind,
        DiagnosticKind::Spelling);
}

#[test]
fn apply_stale_version_clears_latch_without_storing() {
    let mut e = review_editor("teh\n");
    let id = e.active().id; let v = e.active().document.version;
    e.active_mut().diagnostics.slot_mut(DiagSource::Harper).in_flight_version = Some(v);
    e.active_mut().document.version = v + 1; // edited after dispatch
    apply_diagnostics_done(&mut e, id, v, DiagSource::Harper, vec![spelling(0..3)]);
    let slot = e.active().diagnostics.slot(DiagSource::Harper).unwrap();
    assert!(slot.diagnostics.is_empty(), "stale result not stored");
    assert_eq!(slot.in_flight_version, None, "latch cleared (in_flight == msg.version)");
}

#[test]
fn apply_for_disabled_source_drops_and_clears_slot() {
    let mut e = review_editor("teh\n");
    e.active_mut().diagnostics.slot_mut(DiagSource::Plugin("mock")).in_flight_version = Some(1);
    // mock is NOT installed/enabled → result dropped, slot removed.
    let id = e.active().id; let v = e.active().document.version;
    apply_diagnostics_done(&mut e, id, v, DiagSource::Plugin("mock"), vec![spelling(0..3)]);
    assert!(e.active().diagnostics.slot(DiagSource::Plugin("mock")).is_none());
}
```
```rust
// diag_provider.rs
#[test]
fn restarted_arms_only_its_source_slot() {
    let mut e = Editor::new_from_text("x\n", None, (40, 10));
    e.diag_providers.install(Box::new(RecordingProvider::new().with_source(DiagSource::Harper)), true);
    e.diag_providers.install(Box::new(RecordingProvider::new().with_source(DiagSource::Plugin("mock"))), true);
    e.diag_cfg.enabled = true;
    e.active_mut().view.mode = crate::editor::RenderMode::Review;
    apply_provider_event(&mut e, DiagSource::Plugin("mock"), ProviderEvent::Restarted, &TestClock::new(1000));
    assert_eq!(e.active().diagnostics.slot(DiagSource::Plugin("mock")).unwrap().recheck_due_at,
        Some(1000 + e.diag_cfg.debounce_ms));
    assert!(e.active().diagnostics.slot(DiagSource::Harper).is_none(), "other source not armed");
}
```

- [ ] **Step 2: Run to verify fail** → FAIL.

- [ ] **Step 3: Route `apply_diagnostics_done` by source** (§6.2, exact body)

```rust
pub fn apply_diagnostics_done(editor: &mut Editor, buffer_id: BufferId, version: u64,
    source: DiagSource, diagnostics: Vec<Diagnostic>) {
    if !editor.diag_providers.is_enabled(source) {
        if let Some(b) = editor.by_id_mut(buffer_id) { b.diagnostics.clear_source(source); }
        return;
    }
    let union = ignore_union_lower(editor);
    if let Some(b) = editor.by_id_mut(buffer_id) {
        if b.document.version == version {
            let mut diagnostics = diagnostics;
            debug_assert!(diagnostics.iter().all(|d| d.source == source),
                "DiagnosticsDone payload sources match the message tag");
            if !union.is_empty() {
                let text = b.document.buffer.to_string();
                retain_over_union(&mut diagnostics, &text, &union);
            }
            let slot = b.diagnostics.slot_mut(source);
            slot.diagnostics = diagnostics;
            slot.computed_version = version;
        }
        if b.diagnostics.slot(source).map(|s| s.in_flight_version) == Some(Some(version)) {
            b.diagnostics.slot_mut(source).in_flight_version = None;
        }
    }
}
```

- [ ] **Step 4: Per-source hint latch + `apply_provider_event` source use**

`editor.rs`: field `pub diag_hint_shown: bool` → `pub diag_hint_shown: std::collections::BTreeSet<wordcartel_core::diagnostics::DiagSource>`; constructor `diag_hint_shown: false` → `::new()`; `set_render_mode`'s `self.diag_hint_shown = false` → `self.diag_hint_shown.clear()`.

`diagnostics_run.rs::show_install_hint` (§7.3):
```rust
fn show_install_hint(editor: &mut Editor, source: DiagSource) {
    if editor.diag_hint_shown.insert(source) {
        if let Some(hint) = editor.diag_providers.install_hint(source) {
            editor.status = hint.into();
        }
    }
}
```
(Its callers in `dispatch_diagnostics` are updated in Task 5; for THIS task, `dispatch_diagnostics` still calls the old `show_install_hint(editor)` — so give this task's `show_install_hint` the `source` param AND update the two existing call sites in `dispatch_diagnostics` to pass `DiagSource::Harper` interim. That keeps green.)

`diag_provider.rs::apply_provider_event` (§6.3):
```rust
pub fn apply_provider_event(editor: &mut Editor, source: DiagSource, ev: ProviderEvent,
    clock: &dyn Clock) {
    match ev {
        ProviderEvent::Restarted => {
            editor.status = format!("{} restarted", source.label());
            if crate::diagnostics_run::should_run_diagnostics(editor)
                && editor.diag_providers.is_enabled(source)
            {
                let now = clock.now_ms();
                let debounce = editor.diag_cfg.debounce_ms;
                editor.active_mut().diagnostics.slot_mut(source).arm(now, debounce);
            }
        }
        ProviderEvent::Degraded(hint) => { editor.status = hint; }
    }
}
```
Update `diag_provider.rs`'s existing `restarted_*`/`degraded_*` tests to pass a `source` arg and (for the Degraded test) construct with the new `Msg`/fn shape.

- [ ] **Step 4a: Add the shared test helper + migrate the 6 apply-on-default-editor tests** (CRITICAL 2)

In `wordcartel/src/test_support.rs` (already `#[cfg(test)] pub(crate)`), add:
```rust
/// Install an ENABLED Harper `RecordingProvider` into a bare test editor so
/// `apply_diagnostics_done`'s `is_enabled(Harper)` guard accepts Harper results.
/// (`Editor::new_from_text` builds an empty `ProviderSet`; production seeds harper via
/// `install_core_providers` before the loop — this mirrors that for unit tests.)
pub(crate) fn install_enabled_harper(e: &mut crate::editor::Editor) {
    e.diag_providers.install(
        Box::new(crate::diag_provider::RecordingProvider::new()
            .with_source(wordcartel_core::diagnostics::DiagSource::Harper)),
        true);
}
```
Then call `crate::test_support::install_enabled_harper(&mut e);` immediately after the editor is built (before the `apply_diagnostics_done` call) at all SIX sites:
1. `diagnostics_run.rs` — `apply_filters_ignored_spelling_over_the_union_keeps_grammar` (the `Editor::new_from_text("teh cat\n", …)` editor).
2. `app.rs` — the version-gate test that applies current-version then stale-version (the two `apply_diagnostics_done` calls share one editor → one helper call).
3–5. `save.rs` — the three reload/recovery-stale-diagnostics tests (`reload_from_disk` clean-slate, its "fresh result IS accepted" sanity, and `load_recovered`'s late-result-discarded test). Note `save.rs`'s wholesale `new_buf.diagnostics = DiagStore::new()` reset does NOT touch `diag_providers`, so the helper installed on the pre-reload editor persists across the reload — install once per test editor.

(These same editors' result-READS were already repointed to `.slot(DiagSource::Harper)` in Task 3 Step 4; this step only adds the enable-before-apply.)

**Do NOT fold `install_enabled_harper` into the `review_editor(...)` helper.** Many dispatch/proof tests (Task 5, Task 9, and the migrated HEAD dispatch tests) call `review_editor(...)` and THEN explicitly `e.diag_providers.install(Box::new(RecordingProvider…Harper…), true)` — an auto-install in `review_editor` would make that a SECOND Harper install and trip `ProviderSet::install`'s duplicate-source assert (panic). Add the helper ONLY to the 6 named apply-on-default-editor tests, which install no provider of their own. Tests that install their own enabled Harper are already correct and need no change.

- [ ] **Step 5: Run + green** → all suites green; clippy clean.

- [ ] **Step 6: Commit**

```bash
git commit -am "feat(diag): source-route apply + provider events; per-source hint latch"
```

**Done =** new + migrated tests green; tree green; clippy clean; committed.

**§12 conformance:** N/A.

---

## Task 5: Dispatch fan-out (`dispatch_diagnostics(editor, now)` + `dispatch_one`)

**Files:**
- Modify: `wordcartel/src/diagnostics_run.rs` (`dispatch_diagnostics` fan-out + `dispatch_one`)
- Modify: `wordcartel/src/timers.rs` (`on_tick` calls `dispatch_diagnostics(editor, now)`)
- Test: inline `#[cfg(test)]` in `diagnostics_run.rs`

**Interfaces:**
- Consumes: `ProviderSet` source-keyed delegation (Task 2), `due_sources`/`slot_mut` (Task 3), `show_install_hint(editor, source)` (Task 4).
- Produces: `pub fn dispatch_diagnostics(editor: &mut Editor, now: u64)`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn dispatch_fans_out_to_all_due_enabled_sources() {
    let mut e = review_editor("teh\n");
    let h = crate::diag_provider::RecordingProvider::new().with_source(DiagSource::Harper);
    let m = crate::diag_provider::RecordingProvider::new().with_source(DiagSource::Plugin("mock"));
    let (hc, mc) = (h.calls_handle(), m.calls_handle());
    e.diag_providers.install(Box::new(h), true);
    e.diag_providers.install(Box::new(m), true);
    let v = e.active().document.version;
    e.active_mut().diagnostics.slot_mut(DiagSource::Harper).arm(0, 0);
    e.active_mut().diagnostics.slot_mut(DiagSource::Plugin("mock")).arm(0, 0);
    dispatch_diagnostics(&mut e, 10);
    assert_eq!(e.active().diagnostics.slot(DiagSource::Harper).unwrap().in_flight_version, Some(v));
    assert_eq!(e.active().diagnostics.slot(DiagSource::Plugin("mock")).unwrap().in_flight_version, Some(v));
    assert!(hc.lock().unwrap().iter().any(|c| matches!(c, crate::diag_provider::ProviderCall::NotifyChange { version, .. } if *version == v)));
    assert!(mc.lock().unwrap().iter().any(|c| matches!(c, crate::diag_provider::ProviderCall::NotifyChange { .. })));
}

#[test]
fn dispatch_skips_not_due_source_and_latches_independently() {
    let mut e = review_editor("teh\n");
    e.diag_providers.install(Box::new(
        crate::diag_provider::RecordingProvider::new().with_source(DiagSource::Harper)), true);
    e.diag_providers.install(Box::new(
        crate::diag_provider::RecordingProvider::new()
            .with_source(DiagSource::Plugin("mock")).with_accepted(crate::diag_provider::Accepted::No)), true);
    e.active_mut().diagnostics.slot_mut(DiagSource::Harper).arm(0, 0);
    e.active_mut().diagnostics.slot_mut(DiagSource::Plugin("mock")).arm(0, 0);
    dispatch_diagnostics(&mut e, 10);
    assert!(e.active().diagnostics.slot(DiagSource::Harper).unwrap().in_flight_version.is_some());
    assert!(e.active().diagnostics.slot(DiagSource::Plugin("mock")).unwrap().in_flight_version.is_none(),
        "Accepted::No mock does not latch; harper unaffected");
}

#[test]
fn dispatch_over_cap_consumes_deadlines_and_never_latches() {
    let big = "x".repeat((crate::limits::DIAG_MAX_SEND_BYTES as usize) + 1);
    let mut e = review_editor(&big);
    e.diag_providers.install(Box::new(
        crate::diag_provider::RecordingProvider::new().with_source(DiagSource::Harper)), true);
    e.active_mut().diagnostics.slot_mut(DiagSource::Harper).arm(0, 0);
    dispatch_diagnostics(&mut e, 10);
    assert_eq!(e.status, "document too large for grammar checking");
    assert_eq!(e.active().diagnostics.slot(DiagSource::Harper).unwrap().in_flight_version, None);
    assert_eq!(e.active().diagnostics.slot(DiagSource::Harper).unwrap().recheck_due_at, None);
}
```

- [ ] **Step 2: Run to verify fail** → FAIL (`dispatch_diagnostics` arity).

- [ ] **Step 3: Implement the fan-out + `dispatch_one`** (§7.1, exact code)

```rust
pub fn dispatch_diagnostics(editor: &mut Editor, now: u64) {
    let due: Vec<DiagSource> = editor.active().diagnostics.due_sources(now)
        .filter(|s| editor.diag_providers.is_enabled(*s)).collect();
    if due.is_empty() { return; }
    let b = editor.active();
    let (buffer_id, version) = (b.id, b.document.version);
    let path = b.document.path.clone();
    let text = b.document.buffer.snapshot().to_string();
    if text.len() as u64 > crate::limits::DIAG_MAX_SEND_BYTES {
        for s in &due { editor.active_mut().diagnostics.slot_mut(*s).recheck_due_at = None; }
        editor.status = "document too large for grammar checking".into();
        return;
    }
    for source in due { dispatch_one(editor, source, buffer_id, version, &path, &text); }
}

fn dispatch_one(editor: &mut Editor, source: DiagSource, buffer_id: BufferId, version: u64,
    path: &Option<std::path::PathBuf>, text: &str) {
    use crate::diag_provider::{Availability, Accepted};
    editor.active_mut().diagnostics.slot_mut(source).recheck_due_at = None; // consumed
    editor.diag_providers.ensure_running(source);
    match editor.diag_providers.availability(source) {
        Some(Availability::Unavailable) | None => { show_install_hint(editor, source); return; }
        Some(Availability::Starting) => {
            editor.status = format!("starting {}…", source.label());
        }
        Some(Availability::Idle) | Some(Availability::Ready) => {}
    }
    match editor.diag_providers.notify_change(source, buffer_id, version, path.clone(),
        text.to_string()) {
        Accepted::Yes => {
            editor.active_mut().diagnostics.slot_mut(source).in_flight_version = Some(version);
        }
        Accepted::No => show_install_hint(editor, source),
    }
}
```
Both fns are well under 100 lines (verify). Remove the interim `DiagSource::Harper` literals left in the old body.

- [ ] **Step 4: Migrate ALL 7 `dispatch_diagnostics` callers to the new arity** (IMPORTANT — the arity change breaks 7 sites, not just the timer)

The signature became `dispatch_diagnostics(editor, now)`. Migrate every caller (grep `dispatch_diagnostics(` to confirm — 1 production + 6 tests):
1. **Production** `timers.rs::on_tick` — replace the diagnostics dispatch block with
```rust
if crate::diagnostics_run::should_run_diagnostics(editor)
    && editor.active().diagnostics.any_due(now)
{ crate::diagnostics_run::dispatch_diagnostics(editor, now); }
```
(drop the now-unused `let version = …` line if nothing else uses it; `now` is already bound in `on_tick`). Confirm `diag_deadline` already uses `due_deadline()` (Task 3).
2–7. **The 6 HEAD dispatch unit tests in `diagnostics_run.rs`** (`dispatch_latches_in_flight_only_on_accepted_yes`, `dispatch_no_latch_and_hint_on_accepted_no`, `dispatch_over_cap_sets_status_and_never_touches_provider`, `dispatch_unavailable_shows_hint_once`, `dispatch_starting_shows_no_silent_wait_status_and_latches`, and the sixth `dispatch_diagnostics(&mut e)` caller) — each `dispatch_diagnostics(&mut e)` → `dispatch_diagnostics(&mut e, <now>)` (use the same clock value the test arms with — e.g. `10` when armed at `arm(0, …)`, so the slot is due). These tests already had their provider install (Task 2) and slot access (Task 3) migrated; this is the arity fix. Verify each still asserts correctly under per-source dispatch (they use a single enabled Harper → one due source → identical behavior).

- [ ] **Step 5: Run + green** → all green; clippy clean; `cargo test -p wordcartel --test module_budgets` (timers ≤400) green.

- [ ] **Step 6: Commit**

```bash
git commit -am "feat(diag): per-source dispatch fan-out (dispatch_one); timer any_due/now"
```

**Done =** new + migrated tests green; tree green; clippy clean; committed.

**§12 conformance:** N/A. Module budgets: `dispatch_one` split keeps both fns <100 (too_many_lines); `timers.rs` net-neutral.

---

## Task 6: The switchable lens — state, setter, lens-routed render/nav/status

**Files:**
- Modify: `wordcartel/src/editor.rs` (`active_analysis_source` field + constructor + `set_analysis_source`)
- Modify: `wordcartel/src/diagnostics_run.rs` (`active_lens_diags`)
- Modify: `wordcartel/src/render.rs` (gather → lens), `registry.rs` (`quick_fix`/`diag_next`/`diag_prev` → lens), `render_status.rs` (label → lens)
- Test: inline `#[cfg(test)]` in `diagnostics_run.rs` + `editor.rs`

**Interfaces:**
- Consumes: `ProviderSet` (Task 2), `slot`/`valid_for` (Task 3).
- Produces:
  - `Editor.active_analysis_source: DiagSource`; `Editor::set_analysis_source(&mut self, source)`.
  - `pub fn active_lens_diags(editor: &Editor) -> Option<&[Diagnostic]>`.

- [ ] **Step 1: Write the failing tests**

```rust
// diagnostics_run.rs
#[test]
fn active_lens_diags_follows_the_lens() {
    let mut e = review_editor("teh cat\n");
    e.diag_providers.install(Box::new(
        crate::diag_provider::RecordingProvider::new().with_source(DiagSource::Harper)), true);
    e.diag_providers.install(Box::new(
        crate::diag_provider::RecordingProvider::new().with_source(DiagSource::Plugin("mock"))), true);
    let v = e.active().document.version;
    let hs = e.active_mut().diagnostics.slot_mut(DiagSource::Harper);
    hs.diagnostics = vec![spelling(0..3)]; hs.computed_version = v;
    let ms = e.active_mut().diagnostics.slot_mut(DiagSource::Plugin("mock"));
    ms.diagnostics = vec![grammar(4..7)]; ms.computed_version = v;
    assert_eq!(active_lens_diags(&e).unwrap().len(), 1);
    assert_eq!(active_lens_diags(&e).unwrap()[0].kind, DiagnosticKind::Spelling); // default lens = Harper
    e.set_analysis_source(DiagSource::Plugin("mock"));
    assert_eq!(active_lens_diags(&e).unwrap()[0].kind, DiagnosticKind::Grammar);
}
```
```rust
// editor.rs
#[test]
fn set_analysis_source_refuses_disabled_engine() {
    let mut e = Editor::new_from_text("x\n", None, (40, 10));
    e.diag_providers.install(Box::new(
        crate::diag_provider::RecordingProvider::new().with_source(DiagSource::Harper)), true);
    e.set_analysis_source(wordcartel_core::diagnostics::DiagSource::Vale);
    assert_eq!(e.active_analysis_source, wordcartel_core::diagnostics::DiagSource::Harper);
    assert!(e.status.contains("not enabled"));
}
```

- [ ] **Step 2: Run to verify fail** → FAIL.

- [ ] **Step 3: Add state + setter** (`editor.rs`, §8.1)

Field (near the diagnostics fields): `pub active_analysis_source: wordcartel_core::diagnostics::DiagSource,`; constructor default `wordcartel_core::diagnostics::DiagSource::Harper`. Setter:
```rust
pub fn set_analysis_source(&mut self, source: wordcartel_core::diagnostics::DiagSource) {
    if !self.diag_providers.is_enabled(source) {
        self.status = format!("{} is not enabled", source.label());
        return;
    }
    self.active_analysis_source = source;
    self.status = format!("analysis: {}", source.label());
}
```

- [ ] **Step 4: `active_lens_diags` + route the four consumers** (§8.2)

`diagnostics_run.rs`:
```rust
pub fn active_lens_diags(editor: &Editor) -> Option<&[Diagnostic]> {
    if !should_show_diagnostics(editor) { return None; }
    let b = editor.active();
    b.diagnostics.slot(editor.active_analysis_source)
        .filter(|s| s.valid_for(b.document.version))
        .map(|s| s.diagnostics.as_slice())
}
```
`render.rs` gather: replace the `diag_active`/`diag_all` block with
```rust
let diag_all: &[wordcartel_core::diagnostics::Diagnostic] =
    crate::diagnostics_run::active_lens_diags(editor).unwrap_or(&[]);
let diag_active = !diag_all.is_empty();
```
(the `partition_point` windowing, face-by-`kind`, and `use_placed` stay).
`registry.rs` `quick_fix`/`diag_next`/`diag_prev`: replace each `should_show + valid_for + read` prelude with `let Some(diags) = crate::diagnostics_run::active_lens_diags(c.editor) else { … }` — `quick_fix` keeps its `status = "no diagnostic here"` on the else; `diag_next`/`diag_prev` keep the silent `return CommandResult::Handled`. The caret-find / unfold / selection / `open_diag` tails are unchanged (they now read from `diags`, the lens slice). The `diag_apply_selected` add-dict path's `reload_dictionary_enabled()` (Task 2) is unchanged.
`render_status.rs` Review arm (§8.3): follow the lens —
```rust
crate::editor::RenderMode::Review => {
    let lens = editor.active_analysis_source;
    if editor.diag_providers.availability(lens) == Some(crate::diag_provider::Availability::Ready)
    { format!("REVIEW · {}", lens.label()).into() } else { "REVIEW".into() }
}
```

- [ ] **Step 5: Run + green** → all green; clippy clean; module_budgets (render ≤900) green.

- [ ] **Step 6: Commit**

```bash
git commit -am "feat(diag): switchable analysis lens (active_lens_diags routes render/nav/status)"
```

**Done =** new + migrated tests green; tree green; clippy clean; committed.

**§12 conformance:** `set_analysis_source` is the single lens setter (law 6) — Task 7 registers the commands that call it. This task adds no command yet; the lens is inert until Task 7. No palette/menu/hint change here.

---

## Task 7: Lens/enable commands + `cycle_analysis_source` + `set_engine_enabled`

**Files:**
- Modify: `wordcartel/src/diagnostics_run.rs` (`cycle_analysis_source`, `set_engine_enabled`)
- Modify: `wordcartel/src/registry.rs` (3 registrations in the Diagnostics block)
- Test: inline `#[cfg(test)]` in `diagnostics_run.rs` + `registry.rs`

**Interfaces:**
- Consumes: `set_analysis_source` (Task 6), `ProviderSet::{enabled_sources,set_enabled,is_enabled}` (Task 2), `slot_mut`/`clear_source` (Task 3).
- Produces: `pub fn cycle_analysis_source(editor)`; `pub fn set_engine_enabled(editor, source, on, clock)`; commands `analysis_engine_harper`, `analysis_next`, `toggle_engine_harper`.

- [ ] **Step 1: Write the failing tests**

```rust
// diagnostics_run.rs
#[test]
fn cycle_wraps_enabled_sources_only() {
    let mut e = Editor::new_from_text("x\n", None, (40, 10));
    e.diag_providers.install(Box::new(crate::diag_provider::RecordingProvider::new().with_source(DiagSource::Harper)), true);
    e.diag_providers.install(Box::new(crate::diag_provider::RecordingProvider::new().with_source(DiagSource::Plugin("mock"))), true);
    assert_eq!(e.active_analysis_source, DiagSource::Harper);
    cycle_analysis_source(&mut e); assert_eq!(e.active_analysis_source, DiagSource::Plugin("mock"));
    cycle_analysis_source(&mut e); assert_eq!(e.active_analysis_source, DiagSource::Harper);
}

#[test]
fn disable_clears_slots_and_relocates_lens() {
    use crate::test_support::TestClock;
    let mut e = review_editor("teh\n");
    e.diag_providers.install(Box::new(crate::diag_provider::RecordingProvider::new().with_source(DiagSource::Harper)), true);
    e.diag_providers.install(Box::new(crate::diag_provider::RecordingProvider::new().with_source(DiagSource::Plugin("mock"))), true);
    e.set_analysis_source(DiagSource::Plugin("mock"));
    e.active_mut().diagnostics.slot_mut(DiagSource::Plugin("mock")).diagnostics = vec![spelling(0..3)];
    set_engine_enabled(&mut e, DiagSource::Plugin("mock"), false, &TestClock::new(0));
    assert!(e.active().diagnostics.slot(DiagSource::Plugin("mock")).is_none(), "slot cleared");
    assert_eq!(e.active_analysis_source, DiagSource::Harper, "lens relocated off disabled engine");
    assert!(!e.diag_providers.is_enabled(DiagSource::Plugin("mock")));
}
```
```rust
// registry.rs (dispatch_id pattern used by existing tests)
#[test]
fn analysis_commands_registered_and_dispatch() {
    let reg = Registry::builtins();
    assert!(reg.meta(CommandId("analysis_engine_harper")).is_some());
    assert!(reg.meta(CommandId("analysis_next")).is_some());
    assert!(reg.meta(CommandId("toggle_engine_harper")).is_some());
    assert_eq!(reg.meta(CommandId("analysis_engine_harper")).unwrap().menu, None, "set-primitive is palette-only");
    assert!(reg.meta(CommandId("analysis_next")).unwrap().menu.is_some(), "cycle carried in the menu");
}
```

- [ ] **Step 2: Run to verify fail** → FAIL.

- [ ] **Step 3: Implement the domain fns** (`diagnostics_run.rs`, §8.1/§8.4)

```rust
pub fn cycle_analysis_source(editor: &mut Editor) {
    let enabled: Vec<DiagSource> = editor.diag_providers.enabled_sources().collect();
    if enabled.len() < 2 { editor.status = "no other analysis engine".into(); return; }
    let cur = editor.active_analysis_source;
    let idx = enabled.iter().position(|s| *s == cur).unwrap_or(0);
    editor.set_analysis_source(enabled[(idx + 1) % enabled.len()]);
}

pub fn set_engine_enabled(editor: &mut Editor, source: DiagSource, on: bool,
    clock: &dyn wordcartel_core::history::Clock) {
    if !editor.diag_providers.set_enabled(source, on) {
        editor.status = format!("unknown analysis engine: {}", source.label());
        return;
    }
    if on {
        if should_run_diagnostics(editor) {
            let now = clock.now_ms();
            editor.active_mut().diagnostics.slot_mut(source).arm(now, 0);
        }
        editor.status = format!("{} enabled", source.label());
    } else {
        for b in editor.buffers.iter_mut() { b.diagnostics.clear_source(source); }
        if editor.active_analysis_source == source {
            match editor.diag_providers.enabled_sources().next() {
                Some(next) => editor.set_analysis_source(next),
                None => editor.status = format!("{} disabled — no analysis engine enabled",
                    source.label()),
            }
        } else { editor.status = format!("{} disabled", source.label()); }
    }
}
```
(Disable does NOT `shutdown()` the provider — warm for re-enable; teardown stays the loop-exit `shutdown_all`.)

- [ ] **Step 4: Register the 3 commands** (`registry.rs`, in the Diagnostics block near `quick_fix`, §8.5)

```rust
// Analysis lens — set-per-state primitive (palette-only) + stateful cycle representative
// (contract rule 8 — the keymap_next / cycle_render_mode precedent). One primitive per
// AVAILABLE core engine; the ltex/vale effort adds its siblings here.
r.register("analysis_engine_harper", "Analysis Engine: Harper", None, |c| {
    c.editor.set_analysis_source(wordcartel_core::diagnostics::DiagSource::Harper);
    CommandResult::Handled
});
r.register_stateful("analysis_next", "Analysis Engine", Some(MenuCategory::View),
    |e| MenuMark::Value(e.active_analysis_source.label()),
    |c| { crate::diagnostics_run::cycle_analysis_source(c.editor); CommandResult::Handled });
// Per-engine enablement — a 2-state toggle (contract rule 8), palette-only.
r.register("toggle_engine_harper", "Toggle Harper Engine", None, |c| {
    let on = !c.editor.diag_providers.is_enabled(wordcartel_core::diagnostics::DiagSource::Harper);
    crate::diagnostics_run::set_engine_enabled(c.editor,
        wordcartel_core::diagnostics::DiagSource::Harper, on, c.clock);
    CommandResult::Handled
});
```
`MenuMark::Value` takes the `&'static str` from `label` directly (verified `MenuMark::Value(&'static str)`).

- [ ] **Step 5: Run + green**

Run: `cargo test -p wordcartel diagnostics_run registry 2>&1 | tail -20` → PASS.
Run the command-surface invariant tests: `cargo test -p wordcartel palette 2>&1 | tail -20` (palette-completeness now covers the 3 new commands automatically) → green. `cargo test 2>&1 | tail -20` → all green; clippy clean.

- [ ] **Step 6: Commit**

```bash
git commit -am "feat(diag): analysis-engine lens + enable commands (cycle + set-per-state + toggle)"
```

**Done =** new tests green; palette-completeness + hint-resolution invariant tests green; tree green; clippy clean; committed.

**§12 conformance (state explicitly in the task report):** law 1 (mutations only via registered commands); law 2 (lens → `analysis_engine_harper` + `analysis_next`; enablement → `toggle_engine_harper`; the `diagnostics.enabled` + `diagnostics.grammar` gaps remain deliberately waived per §12); law 3 (palette-exhaustive — the 3 register normally); law 4 (only `analysis_next` in the menu, names a command → in palette); law 6 (lens → `set_analysis_source`; enablement → `set_engine_enabled`); rule 8 (set-per-state + cycle for the lens; toggle for enablement); law 7 (hints resolve via registry — the existing tests stay green). No dynamic menu section added.

---

## Task 8: Config wiring + `install_core_providers`

**Files:**
- Modify: `wordcartel/src/config.rs` (`RawHarperEngine`; `linters` consumption doc; grammar override)
- Modify: `wordcartel/src/diagnostics_run.rs` (`install_core_providers`)
- Modify: `wordcartel/src/app.rs::run` (replace the interim install block with `install_core_providers`, placed BEFORE the `warns.first()` status point)
- Test: inline `#[cfg(test)]` in `config.rs` + `diagnostics_run.rs`

**Interfaces:**
- Consumes: `ProviderSet::install`, `set_analysis_source`, `HarperLs::new`, `ProviderConfig`.
- Produces: `pub fn install_core_providers(editor, cfg, msg_tx, warns)`.

- [ ] **Step 1: Write the failing tests**

```rust
// diagnostics_run.rs — install semantics (no real thread; harper is lazy so install spawns nothing)
#[test]
fn install_core_providers_enables_per_linters_and_warns_unknown() {
    use crate::config::Config;
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut e = Editor::new_from_text("x\n", None, (40, 10));
    let mut cfg = Config::default();
    cfg.diagnostics.linters = Some(vec!["harper".into(), "bogus".into()]);
    let mut warns = Vec::new();
    install_core_providers(&mut e, &cfg, &tx, &mut warns);
    assert!(e.diag_providers.is_enabled(DiagSource::Harper));
    assert_eq!(e.active_analysis_source, DiagSource::Harper, "default lens = first enabled");
    assert!(warns.iter().any(|w| w.contains("bogus")), "unknown linter warned");
}

#[test]
fn install_core_providers_none_linters_enables_harper() {
    use crate::config::Config;
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut e = Editor::new_from_text("x\n", None, (40, 10));
    let cfg = Config::default(); // linters = None
    let mut warns = Vec::new();
    install_core_providers(&mut e, &cfg, &tx, &mut warns);
    assert!(e.diag_providers.is_enabled(DiagSource::Harper));
    assert!(warns.is_empty());
}
```
```rust
// config.rs #[cfg(test)] mod tests — mirrors the existing `load_clip` helper (config.rs, C3 tests):
// write the TOML to a temp file, then `load(&[path])` (the real config entry point).
fn load_diag(name: &str, body: &str) -> (Config, Vec<String>) {
    let p = std::env::temp_dir().join(format!("wcartel-cfg-{}-{name}.toml", std::process::id()));
    std::fs::write(&p, body).unwrap();
    let out = load(std::slice::from_ref(&p));
    let _ = std::fs::remove_file(&p);
    out
}

#[test]
fn harper_engine_table_overrides_grammar() {
    let (cfg, _warns) = load_diag("harper-grammar",
        "[diagnostics]\ngrammar = true\n[diagnostics.harper]\ngrammar = false\n");
    assert!(!cfg.diagnostics.grammar, "[diagnostics.harper].grammar overrides top-level");
}

#[test]
fn linters_list_round_trips() {
    let (cfg, _warns) = load_diag("linters", "[diagnostics]\nlinters = [\"harper\"]\n");
    assert_eq!(cfg.diagnostics.linters, Some(vec!["harper".to_string()]));
}
```
(`load` is `super::load` — already in scope via `use super::*;` in the test module, exactly as `load_clip` calls it.)

- [ ] **Step 2: Run to verify fail** → FAIL.

- [ ] **Step 3: Config — `RawHarperEngine` + grammar override + linters doc** (`config.rs`, §9.1/§9.2)

Add `#[derive(Debug, Default, Deserialize)] #[serde(default)] struct RawHarperEngine { grammar: Option<bool> }` and a `harper: RawHarperEngine` field on `RawDiagnostics`. In the diagnostics fold, AFTER the existing `if let Some(v) = raw.diagnostics.grammar { cfg.diagnostics.grammar = v; }`, add `if let Some(v) = raw.diagnostics.harper.grammar { cfg.diagnostics.grammar = v; }` (per-engine spelling overrides top-level). Replace the stale "validated against the core catalog later (Task 4 assembly) — warn there" comment with a pointer to `diagnostics_run::install_core_providers` (where the linters names are now validated). `linters` layering is unchanged (already parsed).

- [ ] **Step 4: `install_core_providers`** (`diagnostics_run.rs`, §9.3)

```rust
/// Build the core provider catalog (harper today), fold `linters` into per-engine enablement
/// (warning on unknown names), install into `editor.diag_providers`, and seed the default lens
/// (first enabled source in cycle order). Providers spawn nothing here — lazy, as before.
pub fn install_core_providers(editor: &mut Editor, cfg: &crate::config::Config,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>, warns: &mut Vec<String>) {
    // The core catalog in cycle order. Effort b appends ltex/vale here.
    let catalog: &[DiagSource] = &[DiagSource::Harper];
    // Which engines are enabled: None → all core; Some(list) → exactly the named (config_name).
    let enabled_of = |src: DiagSource| -> bool {
        match &cfg.diagnostics.linters {
            None => true,
            Some(list) => list.iter().any(|n| n == src.config_name()),
        }
    };
    if let Some(list) = &cfg.diagnostics.linters {
        for name in list {
            if !catalog.iter().any(|s| s.config_name() == name) {
                warns.push(format!("config: diagnostics.linters — unknown engine \"{name}\" (known: harper)"));
            }
        }
    }
    for &src in catalog {
        let provider: Box<dyn crate::diag_provider::DiagnosticsProvider> = match src {
            DiagSource::Harper => Box::new(crate::harper_ls::HarperLs::new(
                msg_tx.clone(),
                crate::diag_provider::ProviderConfig {
                    grammar: cfg.diagnostics.grammar,
                    dictionary: cfg.diagnostics.dictionary.clone(),
                    max_file_length: crate::limits::HARPER_MAX_FILE_LENGTH,
                })),
            // Exhaustive — future core engines add arms; LTeX/Vale/Plugin are not in the catalog yet.
            DiagSource::LTeX | DiagSource::Vale | DiagSource::Plugin(_) => continue,
        };
        editor.diag_providers.install(provider, enabled_of(src));
    }
    // Seed the lens to the first enabled source (Harper fallback when none enabled — inert).
    if let Some(first) = editor.diag_providers.enabled_sources().next() {
        editor.active_analysis_source = first;
    }
}
```
Note: seeding writes the field directly (not `set_analysis_source`, which would status-message and refuse a not-yet-populated set) — this is construction, matching the clipboard-provider seeding precedent.

- [ ] **Step 5: Wire into `run()` before the status point** (`app.rs`, §9.3 finding)

Remove the interim `editor.diag_providers.install(Box::new(HarperLs::new(...)), true)` block (Task 2). Call `crate::diagnostics_run::install_core_providers(&mut editor, &cfg, &msg_tx, &mut warns);` at a point AFTER `msg_tx` exists and BEFORE `if let Some(w) = warns.first() { editor.status = w.clone(); }`. `msg_tx` is created before the plugin-load phase; place the install right after the plugin-load phase (editor still the plain `&mut`, pre-Rc-wrap) and before `build_keymap`/the `warns.first()` line, so unknown-linter warnings reach `editor.status`. Loop-exit `shutdown_all` (Task 2) is unchanged.

- [ ] **Step 6: Run + green**

Run: `cargo test -p wordcartel config diagnostics_run 2>&1 | tail -20` → PASS.
Run: `cargo test 2>&1 | tail -20` → all green; `cargo clippy --workspace --all-targets` → clean.

- [ ] **Step 7: Commit**

```bash
git commit -am "feat(diag): wire linters enabled-list + [diagnostics.harper] table; install_core_providers"
```

**Done =** new tests green; tree green; clippy clean; committed.

**§12 conformance:** the `[diagnostics.harper].grammar` key is a config-file *spelling* of the already-command-less `diagnostics.grammar` option — no new law-2 obligation (waived per §12, consistent with `diagnostics.enabled`). No new command here.

---

## Task 9: Two-provider mock proof (§14.1's 9 cases) + e2e lens journey

**Files:**
- Modify: `wordcartel/src/diagnostics_run.rs` (`#[cfg(test)]` — the fan-out/staleness/routing/disable/lens/hint cases not already covered by Tasks 3–8)
- Modify: `wordcartel/src/e2e.rs` (`#[cfg(test)]` — the lens-switch journey)
- Test: the above

**Interfaces:**
- Consumes: everything from Tasks 2–8.
- Produces: no production code — the acceptance proof.

Maps §14.1's 9 cases to tests (some already landed in earlier tasks — this task fills the gaps and adds the acceptance-bar staleness case + the e2e journey):

| §14.1 case | Where |
|---|---|
| 1 Fan-out | Task 5 `dispatch_fans_out_to_all_due_enabled_sources` ✓ |
| 2 Staleness independence (acceptance bar) | **new here** — `slow_engine_dropped_by_its_guard_fast_applies` (§14.1 item 2 exact scenario) |
| 3 Per-source in-flight | Task 5 `dispatch_skips_not_due_source_and_latches_independently` (extend to the in-flight-blocks-only-itself case) |
| 4 Per-source Accepted::No | Task 5 ✓ |
| 5 Routing | Task 4 `apply_routes_to_the_named_source_slot_only` ✓ |
| 6 Disable/enable | Task 7 `disable_clears_slots_and_relocates_lens` (+ **new** late-Done-does-not-resurrect) |
| 7 Lens | Task 6 + Task 7 (+ **new** `active_lens_diags` under `<2` engines no-op) |
| 8 Per-source hint latch | **new here** — two Unavailable engines each hint once per Review entry |
| 9 Restart re-arm | Task 4 `restarted_arms_only_its_source_slot` ✓ |

- [ ] **Step 1: Write the acceptance-bar staleness test** (`diagnostics_run.rs`, §14.1 item 2 exact)

```rust
#[test]
fn slow_engine_dropped_by_its_guard_while_fast_applies() {
    let mut e = review_editor("teh cat\n");
    e.diag_providers.install(Box::new(crate::diag_provider::RecordingProvider::new().with_source(DiagSource::Harper)), true);
    e.diag_providers.install(Box::new(crate::diag_provider::RecordingProvider::new().with_source(DiagSource::Plugin("mock"))), true);
    let id = e.active().id; let v = e.active().document.version;
    // both dispatched at v
    e.active_mut().diagnostics.slot_mut(DiagSource::Harper).in_flight_version = Some(v);
    e.active_mut().diagnostics.slot_mut(DiagSource::Plugin("mock")).in_flight_version = Some(v);
    // the document advances to v+1 (an edit)
    e.active_mut().document.version = v + 1;
    // SLOW engine (mock) terminal for v arrives → NOT stored, latch clears
    apply_diagnostics_done(&mut e, id, v, DiagSource::Plugin("mock"), vec![grammar(4..7)]);
    let ms = e.active().diagnostics.slot(DiagSource::Plugin("mock")).unwrap();
    assert!(ms.diagnostics.is_empty(), "stale v result not stored");
    assert_eq!(ms.in_flight_version, None, "slow latch cleared");
    // FAST engine (harper) terminal for v+1 arrives → stored
    apply_diagnostics_done(&mut e, id, v + 1, DiagSource::Harper, vec![spelling(0..3)]);
    let hs = e.active().diagnostics.slot(DiagSource::Harper).unwrap();
    assert_eq!(hs.diagnostics.len(), 1);
    assert_eq!(hs.computed_version, v + 1);
    assert_eq!(hs.in_flight_version, None);
    assert_ne!(hs.computed_version, ms.computed_version, "no cross-contamination");
}

#[test]
fn late_done_for_disabled_source_does_not_resurrect_slot() {
    use crate::test_support::TestClock;
    let mut e = review_editor("teh\n");
    e.diag_providers.install(Box::new(crate::diag_provider::RecordingProvider::new().with_source(DiagSource::Harper)), true);
    e.diag_providers.install(Box::new(crate::diag_provider::RecordingProvider::new().with_source(DiagSource::Plugin("mock"))), true);
    let id = e.active().id; let v = e.active().document.version;
    set_engine_enabled(&mut e, DiagSource::Plugin("mock"), false, &TestClock::new(0));
    apply_diagnostics_done(&mut e, id, v, DiagSource::Plugin("mock"), vec![spelling(0..3)]);
    assert!(e.active().diagnostics.slot(DiagSource::Plugin("mock")).is_none());
}

#[test]
fn per_source_hint_shows_once_per_review_entry() {
    let mut e = review_editor("teh\n");
    e.diag_providers.install(Box::new(crate::diag_provider::RecordingProvider::new()
        .with_source(DiagSource::Harper).with_availability(crate::diag_provider::Availability::Unavailable)), true);
    e.diag_providers.install(Box::new(crate::diag_provider::RecordingProvider::new()
        .with_source(DiagSource::Plugin("mock")).with_availability(crate::diag_provider::Availability::Unavailable)), true);
    e.active_mut().diagnostics.slot_mut(DiagSource::Harper).arm(0, 0);
    e.active_mut().diagnostics.slot_mut(DiagSource::Plugin("mock")).arm(0, 0);
    dispatch_diagnostics(&mut e, 10);
    assert!(e.diag_hint_shown.contains(&DiagSource::Harper));
    assert!(e.diag_hint_shown.contains(&DiagSource::Plugin("mock")));
    // re-entering Review clears the set
    e.set_render_mode(crate::editor::RenderMode::LivePreview, 20);
    e.set_render_mode(crate::editor::RenderMode::Review, 30);
    assert!(e.diag_hint_shown.is_empty(), "hint latch reset on Review entry");
}
```

- [ ] **Step 2: Run to verify fail** → FAIL (if any helper missing) / PASS-after-impl (pure test task — the code exists; these assert real behavior). If a test reveals a real gap, fix the production code in the owning task's module and note it.

- [ ] **Step 3: Add the e2e lens journey** (`e2e.rs`, §14.3)

Mirror the existing `diagnostics_probe`/`make_diags` harness (it seeds `diag_cfg.enabled = true`, sets Review, injects `DiagnosticsDone`). Add a test that: installs two `RecordingProvider`s (Harper + `Plugin("mock")`) into `e.diag_providers`, enters Review, injects `Msg::DiagnosticsDone { source: Harper, … }` and `{ source: Plugin("mock"), … }` at the current version, renders against the `TestBackend`, asserts the painted underline cells match the DEFAULT lens (Harper) set; dispatches `analysis_next` (via the registry, two enabled engines); re-renders; asserts the painted set switched to the mock's. Follow the existing e2e journey construction exactly (the harness's `Harness`/`step_timed`/render-capture helpers).

- [ ] **Step 4: Run + green**

Run: `cargo test -p wordcartel diagnostics_run e2e 2>&1 | tail -20` → PASS.
Run: `cargo test 2>&1 | tail -20` → all suites green; `cargo clippy --workspace --all-targets` → clean.
Run the PTY smoke suite and quote its summary in the report: `scripts/smoke/run.sh 2>&1 | tail -3` (advisory).

- [ ] **Step 5: Commit**

```bash
git commit -am "test(diag): two-provider coexistence/staleness/lens proof + e2e lens journey"
```

**Done =** all 9 §14.1 cases green across the tasks; e2e journey green; full `cargo test` + clippy clean; smoke summary quoted; committed.

**§12 conformance:** N/A (test-only). This task's report quotes the PTY smoke one-line summary verbatim (mandatory-run/advisory-pass) and confirms the command-surface invariant tests green.

---

## Self-Review (run against the spec)

**1. Spec coverage:**
- §3 core type → Task 1 ✓. §4 `ProviderSet`/trait/NullProvider-delete → Task 2 ✓. §5 source-partitioned store → Task 3 ✓. §6 marshaling (both delivery paths) → Task 1 (shapes) + Task 4 (routing) ✓. §7 dispatch fan-out + per-source hint → Task 5 + Task 4 ✓. §8 lens + commands → Task 6 + Task 7 ✓. §9 config → Task 8 ✓. §10 error handling → statuses across Tasks 4/5/6/7 ✓. §11 resources → edge-triggered arming preserved (Tasks 3/5) ✓. §12 conformance → Task 7 note (+ Task 8 grammar waiver) ✓. §13/§13.1 migration inventory → Task 1 Steps 4–6 (all 31 literals + prompts.rs both paths) ✓. §14 testing → Tasks 1–9 with the 9-case map in Task 9 ✓. §15 non-goals → nothing here designs ltex/vale/wc.async/plugin engines ✓.
- No gap found.

**2. Placeholder scan:** every code step shows complete code; no "TBD"/"handle edge cases"/"similar to Task N". The one deliberate deferral to a neighbor pattern is the `config.rs` load-from-str test helper (Step 1 of Task 8) — flagged to mirror the crate's existing config test construction because the private loader signature is house-specific; the assertion and TOML are concrete.

**3. Type consistency:** `DiagSource` (label/config_name), `ProviderSet` method names (`install`/`sources`/`enabled_sources`/`is_enabled`/`set_enabled`/`availability`/`install_hint`/`ensure_running`/`notify_change`/`configure`/`notify_close_all`/`reload_dictionary_enabled`/`shutdown_all`), `DiagStore`/`SourceSlot` (`slot`/`slot_mut`/`clear_source`/`any_due`/`due_sources`/`due_deadline`/`arm`/`valid_for`), `arm_enabled`, `active_lens_diags`, `set_analysis_source`, `cycle_analysis_source`, `set_engine_enabled`, `install_core_providers`, `Msg::DiagnosticsDone { …, source, … }`, `Msg::DiagProviderEvent { source, event }`, `apply_diagnostics_done(…, source, …)`, `apply_provider_event(…, source, …)` — all used consistently across tasks and match the spec's §§3–12 signatures.

**4. Compiles-green sequencing:** Task 1 is atomic (widening + all 31 literals + 2 helper bodies + both `Msg` variant shapes + EVERY `Msg` construct/match/destructure site per the Step-4 inventory + both apply signatures + all 8 apply-fn/delivery callers land in one green commit); Tasks 2–8 each keep the tree compiling — interim `DiagSource::Harper` literals bridge Task 2/3 call sites; `dispatch_diagnostics` keeps its old arity through Task 3 and changes it in Task 5 **with all 7 callers (timer + 6 tests) migrated in that same task** (IMPORTANT 2); `show_install_hint` gains its `source` param with its callers updated in Task 4 (§7.3 refactor of its body stays in Task 4). Each task's Step "Run + green" verifies `cargo build` + `cargo test` before commit.

**Folded Codex-plan-gate findings (re-verified against HEAD):**
- **CRITICAL 1** (`INSTALL_HINT` move): Task 2 moves the const to `harper_ls.rs` AND repoints all 5 files' references (incl. the production `diagnostics_run.rs::show_install_hint` interim `crate::harper_ls::INSTALL_HINT`, plus test imports in `diagnostics_run.rs`/`diag_provider.rs`/`prompts.rs`/`app.rs`) in the SAME task — no re-export, no broken paths. Resolution = **(a) update-all-imports**.
- **CRITICAL 2** (disabled-source drop): Task 4 adds `test_support::install_enabled_harper` and calls it before the apply at the 6 pre-existing apply-on-default-editor tests (`diagnostics_run.rs` union-filter test; `app.rs` version-gate test ×1 editor; `save.rs` reload-clean-slate + reload-fresh-accepted + recovery-late-discard). Explicitly NOT folded into `review_editor` (would duplicate-install and trip the assert). Production safe: `run()` enables harper before the loop.
- **IMPORTANT 1** (counts): Task 1 synced to the corrected spec — `app.rs`=10, `search_ui.rs`=2, total 31 literals + 2 helper bodies; grep-not-count instruction added.
- **IMPORTANT 2** (dispatch callers): Task 5 Step 4 migrates all 7 `dispatch_diagnostics` callers (timer + 6 HEAD tests).
- **MINOR 1**: Task 3 deps corrected to "1, 2".
- **MINOR 2**: Task 8 config test uses the real `load_diag` helper (mirrors `load_clip`: `std::fs::write` + `super::load(&[path])`).
- **CRITICAL (round 2 — Msg-site completeness)**: Task 1 Step 4 now carries the COMPLETE `Msg`-construct/match inventory (two tables + the governing gains-a-field-vs-tuple→struct rule + the compile-safe `..` list). `DiagnosticsDone` owns 19 must-change sites across 9 inventory rows (variant def, Debug arm, reduce arm, prompts arm, the 11 harper emits as one row, `dones` extractor, flush_guard `if let`, `e2e` probe, integration-test match) and leaves 5 `..` sites untouched; `DiagProviderEvent` owns ALL 11 must-change sites (variant def, Debug arm, reduce arm, prompts arm, 3 harper emits, 2 harper helper patterns, app.rs test construct, prompts.rs test construct) + leaves the integration-test `Ok(_)` wildcard untouched. The 4 previously-implicit GAP sites (`harper_ls` flush_guard `if let`, `e2e` probe, `app.rs` test construct, `prompts.rs` test construct) are now explicit.

**5. Grounded in real signatures:** verified against HEAD — `register`/`register_stateful` arity, `MenuMark::Value(&'static str)`, `Ctx { editor, clock, executor, msg_tx }`, `by_id_mut`, `buffers: Vec<Buffer>`, `SUBSYSTEMS`/`diag_deadline`/`on_tick` shape, `render.rs` gather block, `render_status.rs` Review arm, `HarperLs` 11 `DiagnosticsDone` + 3 `DiagProviderEvent` sites, `convert_diagnostics`/`classify_lsp`, `prompts.rs::intercept` arms, `DiagnosticsConfig`/`RawDiagnostics` fields, `DIAG_MAX_SEND_BYTES`/`HARPER_MAX_FILE_LENGTH`.
