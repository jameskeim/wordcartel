# Prose-diagnostics SPINE — multi-provider generalization + switchable lens (design spec)

**Status:** SPEC (2026-07-12, effort "a" of the prose-linter decomposition — `docs/design/prose-linters-design-space.md` §6-a).
**Scope:** generalize the single-provider diagnostics plumbing (harper-ls) to multi-provider, land the
switchable-lens UX, prove the plumbing with harper as provider #1 plus a `#[cfg(test)]` mock second
provider. No real second engine ships.
**Grounding inputs:** `docs/design/prose-linters-design-space.md` (fork map; §§0–1, 6),
`docs/design/prose-linters-scan.md` (wire facts: distinct LSP `source` namespaces — harper `"Harper"`,
ltex `"LTeX"`, vale-ls `"vale-ls"`; `code` = rule id; `codeDescription.href` = doc link), and the real
code at HEAD (every anchor re-verified — §2).

**Locked product decisions honored (not re-opened):**
- Each engine is its OWN namespaced diagnostic view — never merged, never simultaneously inline.
- View interaction = **switchable lens**: inline underlines + `diag_next`/`diag_prev`/`quick_fix` show
  ONE source at a time; a cycle command + per-engine set-per-state commands switch it.
- Default engine = default lens = harper.
- Spine = harper-only; the single→multi path is proven by a test-only mock second provider.

---

## 1. Goals and non-goals

**Goals**
1. `wordcartel_core::diagnostics::Diagnostic` gains `source` (an exhaustive `DiagSource` enum),
   `code: Option<String>`, `href: Option<String>` — the one-time core vocabulary change.
2. `Editor.diag_provider: Box<dyn DiagnosticsProvider>` → a `ProviderSet` registry; each provider keeps
   its own `Availability` (independent warm/degrade — load-bearing for the future JVM engine).
3. `DiagStore` becomes source-partitioned: per-source `{diagnostics, computed_version, recheck_due_at,
   in_flight_version}` slots — per-source instances of the existing staleness/latch machinery, no new
   mechanism.
4. `Msg::DiagnosticsDone` gains `source`; the apply path routes to the right slot and clears only that
   source's in-flight latch; a slow engine's stale result is dropped by its own guard while a fast
   engine's current result applies.
5. `dispatch_diagnostics` fans out: each enabled+due provider gets the same doc snapshot; each is
   latched independently.
6. The switchable lens: `Editor.active_analysis_source`, one shared setter, set-per-state command(s) +
   a cycle, lens-routed nav/quick-fix/underlines/status-bar.
7. Config: wire the already-parsed-but-unused `DiagnosticsConfig.linters` as the enabled-engine list;
   add the `[diagnostics.<engine>]` per-engine table shape (harper only, shape admits engine #2).
8. Test proof: two sources coexist, switch, and stay independently staleness-guarded, via a mock second
   provider.

**Non-goals (deferred — named in §14)**: real ltex/vale providers + JVM lifecycle (effort b); the
viewing/action delta — doc-link "learn more" UI, detail region, per-engine dict/rule writers,
executeCommand relay, more-suggestions population (effort c); `wc.async` (effort d); plugin-declared
engines / plugin dynamic-menu rows (effort e); a source-grouped combined list (deferred to c); a
`severity` field (§3.3).

---

## 2. Grounding — verified anchors (real code at HEAD)

All re-verified by symbol name against the current source; the design-space doc's anchors were checked
one by one. Locations below are symbol-anchored; line numbers are indicative only.

| Symbol | File | Verified shape |
|---|---|---|
| `Diagnostic` | `wordcartel-core/src/diagnostics.rs` | `{ range: Range<usize>, kind: DiagnosticKind, message: String, suggestions: Vec<Suggestion> }`, all fields `pub` — no source/code/href/severity |
| `DiagnosticKind` | same | `{ Spelling, Grammar }` |
| `DiagnosticsProvider` | `wordcartel/src/diag_provider.rs` | `name/availability/ensure_running/configure/notify_change→Accepted/notify_close/reload_dictionary/shutdown`; module doc: "the seam is Open-Closed insurance for provider #2" |
| `Availability` | same | `{ Idle, Starting, Ready, Unavailable }` |
| `NullProvider` | same | hermetic default installed by `Editor::new_from_text` |
| `RecordingProvider` | same, `#[cfg(test)]` | the actual mock (records calls; settable `Accepted`/`Availability`; `calls_handle()` Arc) |
| `INSTALL_HINT` | same | harper-specific text, also used by `harper_ls.rs` (pump spawn-failure path) |
| `apply_provider_event` | same | `Restarted` → status + re-arm; `Degraded(hint)` → status |
| `DiagStore` | `wordcartel/src/diagnostics_run.rs` | one `{ diagnostics, computed_version, recheck_due_at, in_flight_version }` per buffer (`Buffer.diagnostics`) |
| `should_run_diagnostics` / `should_show_diagnostics` | same | `diag_cfg.enabled && mode == Review`; show delegates to run |
| `arm_if_edited` | same | the single re-arm seam, called from `reduce`'s tail (app.rs) |
| `diag_due` | same | armed + reached + `in_flight_version.is_none()`; **its `_version` param is already unused** |
| `dispatch_diagnostics` | same | consumes deadline first, `DIAG_MAX_SEND_BYTES` cap, `ensure_running`, Unavailable→hint / Starting→status, latch on `Accepted::Yes` only (LATCH INVARIANT §5.1) |
| `show_install_hint` / `diag_hint_shown` | same / `editor.rs` | single bool latch, reset on Review entry in `set_render_mode` |
| `retain_unignored` / `ignore_union_lower` / `retain_over_union` | same | client-side ignore filter — Spelling-only, engine-agnostic by nature |
| `apply_diagnostics_done` | same | version-gated store; clears in_flight for that version regardless |
| `Msg::DiagnosticsDone` | `wordcartel/src/app.rs` | `{ buffer_id, version, diagnostics }` — no source; reduce arm delegates to `apply_diagnostics_done` |
| `Msg::DiagProviderEvent` | same | tuple of `ProviderEvent` — no source |
| provider install | `app.rs::run` | `editor.diag_provider = Box::new(HarperLs::new(msg_tx.clone(), ProviderConfig{ grammar, dictionary, max_file_length: HARPER_MAX_FILE_LENGTH }))`; `diag_provider.shutdown()` on loop exit |
| `diag_deadline` + `SUBSYSTEMS` row `"diagnostics"` + `advance` | `wordcartel/src/timers.rs` | deadline excluded while in-flight; advance gates on `should_run` + `diag_due` then dispatches |
| render gather | `wordcartel/src/render.rs` (`diag_active`/`diag_all` locals, `RowCtx.diag_all`) | `should_show && valid_for(version)`; underline painter windows `diag_all` by `partition_point`, face by `kind` |
| status label | `wordcartel/src/render_status.rs::status_left_text` | `REVIEW · {diag_provider.name()}` only when `Ready` |
| nav/fix commands | `wordcartel/src/registry.rs` | `quick_fix`, `diag_next`, `diag_prev`, `recheck_diagnostics` — each re-checks `should_show` + `valid_for` then reads `diagnostics.diagnostics` |
| overlay apply | `wordcartel/src/search_ui.rs::diag_apply_selected` | suggestion → `build_range_replace`; ignore → `session_ignores` + `retain_unignored`; add-dict → `editor.dictionary` + `append_word_to_dict` + `diag_provider.reload_dictionary()` |
| `DiagOverlay` | `wordcartel/src/diag_overlay.rs` | anchor = a cloned `Diagnostic`; rows = suggestions + ignore + add-dict — untouched by this effort except that the anchor now carries `source` |
| `HarperLs` / `HarperState` / `FlushGuard` | `wordcartel/src/harper_ls.rs` | `name() == "Harper"`; 11 `Msg::DiagnosticsDone` construction sites (close-terminal, publish-empty/assembly, codeAction stale/assembled, deadline watchdogs ×2, on_server_gone flush ×2 + queued, FlushGuard drain); 3 `DiagProviderEvent` sites (Restarted, CRASHED_HINT, spawn-failure INSTALL_HINT) |
| `convert_diagnostics` / `classify_lsp` | same | builds `Diagnostic { range, kind, message, suggestions: vec![] }`; `classify_lsp` already reads the LSP `code` field (so `code` is available at the construction site) |
| `DiagnosticsConfig` | `wordcartel/src/config.rs` | `{ enabled, grammar, debounce_ms, dictionary, linters: Option<Vec<String>> }`; `linters` is parsed and layered but consumed by NOTHING |
| linters fold comment | same (fold tail) | "unknown linter names are validated against the core catalog later (Task 4 assembly) — warn there" — **a promise never realized**; this effort realizes it |
| wholesale resets | `wordcartel/src/save.rs` (`reload_from_disk`, `load_recovered`) | `new_buf.diagnostics = DiagStore::new()` + `diag_provider.notify_close(id)`; also `workspace.rs::close_buffer` calls `notify_close` |
| e2e/probes | `wordcartel/src/e2e.rs` | constructs `Msg::DiagnosticsDone` and seeds `diag_cfg.enabled` directly |
| menu/registry machinery | `wordcartel/src/registry.rs`, `menu.rs` | `register(id, label, Option<MenuCategory>, handler)`, `register_stateful(…, state_fn, handler)`, `MenuMark::Value(&'static str)`, `DYNAMIC_SECTIONS` fn-pointer table |
| contract | `docs/design/command-surface-contract.md` | laws 1–7, shape rules 8–10, dynamic-section rules — conformance in §12 |

**Drift / corrections against the design-space doc (report):**
1. **"NullProvider is the mockable point"** (echoed in the effort brief): imprecise. `NullProvider` is
   the *hermetic default*; the actual test mock is `RecordingProvider` (`#[cfg(test)]`, records calls,
   settable returns). The seam (the trait) is the mockable point. This spec builds the mock second
   provider on `RecordingProvider` and **deletes `NullProvider`** (§4.3) — its "no provider" role is
   representable as the empty `ProviderSet` (the Option-over-sentinel rule applied at the set level).
2. **`dispatch_diagnostics` has two behaviors the design-space summary omits**: the global
   `DIAG_MAX_SEND_BYTES` size cap (status + early return, deadline already consumed) and the
   `Starting` → "starting grammar checker…" no-silent-wait status. Both are generalized here (§7).
3. **`diag_due`'s `_version` parameter is already dead** at HEAD; the generalization drops it.
4. **The config-fold comment promises linters-name validation "later"** — that later never happened;
   §11 ships it.
5. **The single `diag_hint_shown: bool` latch** is not mentioned in the design-space doc; with N
   engines it must become per-source (§7.3).
6. Everything else in design-space §1 checked out exactly (including the `config.rs` `linters` hook,
   the `FlushGuard` terminal guarantee, and the per-publish generation guards in `harper_ls.rs`).

---

## 3. Core type change — `wordcartel-core/src/diagnostics.rs`

The review-magnet, done once. `wordcartel-core` stays pure (`#![forbid(unsafe_code)]`, no IO): the new
items are plain data + pure functions.

### 3.1 `DiagSource` — the namespace tag

```rust
/// The engine that produced a diagnostic — the namespace tag behind the per-engine "separate
/// views, never merged" contract. An exhaustive enum, not a free-form string: match sites are
/// forced to place every engine (the `SemanticElement` exhaustive-literal discipline), and an
/// invalid source is unrepresentable (valid-by-construction). `Plugin` carries a static name for
/// non-core engines (future plugin-declared providers; the test mock uses `Plugin("mock")`).
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
    /// menu state-in-label all use this. `&'static str` on purpose: it feeds
    /// `MenuMark::Value(&'static str)` directly.
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

**Why pre-declare `LTeX`/`Vale` with no producer:** this enum is *vocabulary*, not behavior — the exact
pattern of `theme::SemanticElement`, whose variants exist so exhaustive literals are forced everywhere.
Every shell match over `DiagSource` written in this effort must already handle the two future arms
(usually one line each: a label, a config name), so the ltex/vale effort is compiler-guided rather than
grep-guided. They are consumed today by `label`/`config_name` (not dead code), and the scan verifies the
names map to real wire `source` namespaces (`"Harper"`, `"LTeX"`, `"vale-ls"`).

**Why an enum with a `Plugin(&'static str)` arm over an interned string:** exhaustive-match discipline
(the house rule against catch-all `_` absorbing new variants), `Copy` (cheap map keys, no allocation on
the render path), and one honest escape hatch for genuinely dynamic engines. Derives include
`Ord`/`Hash` so it keys the source-partitioned store (`BTreeMap`) and the per-source hint latch.

### 3.2 `Diagnostic` — the widened record

```rust
/// A single flagged issue in a checked text, as reported by one provider behind the shell's
/// `DiagnosticsProvider` seam. Pure data; the producing provider sorts its results ascending by
/// `range.start`. `source` namespaces the diagnostic into its engine's view (views are never
/// merged); `code`/`href` carry the engine's rule id and rule-documentation link when the wire
/// data supplies them (harper supplies `code`, never `href`; ltex/vale supply both — scan §B).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Diagnostic {
    /// Byte range into the checked text that the diagnostic covers.
    pub range: std::ops::Range<usize>,
    /// Whether the provider classified this as a spelling or grammar issue.
    pub kind: DiagnosticKind,
    /// The engine that produced this diagnostic.
    pub source: DiagSource,
    /// Engine rule id (LSP `code`) — harper lint name / LanguageTool ruleId / vale check.
    pub code: Option<String>,
    /// Rule-documentation URL (LSP `codeDescription.href`); `None` when the engine sends none.
    pub href: Option<String>,
    /// Human-readable explanation of the issue, as produced by the provider.
    pub message: String,
    /// Zero or more candidate fixes the provider offers; empty when none apply.
    pub suggestions: Vec<Suggestion>,
}
```

`DiagnosticKind` and `Suggestion` are unchanged. The module doc's single-provider narration ("harper-ls
over LSP … the provider") is updated to the N-provider story.

**Why fields stay `pub` (no constructor):** the existing `Diagnostic` is a plain-data record with all
fields `pub` and no cross-field invariant to protect — range validity is necessarily checked at use
sites against the text of the matching version (render clamps; `diag_apply_selected` clamps and
version-gates), and "sorted by `range.start`" is a Vec-level producer contract, not a per-value
invariant. Valid-by-construction lands where it has teeth: in the *types* (`DiagSource` exhaustive
enum instead of a free-form string; `Option` instead of `""` sentinels for `code`/`href`). Adding a
constructor that validates nothing would be ceremony, and diverging from the type's established shape
would churn every construction site for no invariant gained.

**Why `source` on each `Diagnostic` when the store is already source-partitioned:** the value must be
self-describing once it leaves the store — `quick_fix` clones a `Diagnostic` into `DiagOverlay.anchor`,
and effort c's per-engine actions (dict writers, rule disable, doc links) route off the anchor's
source. Redundancy between the slot key and the value is checked with a `debug_assert` at apply (§6.2).

### 3.3 Deliberate deferral: no `severity` field

The brief says "consider severity." Considered and **not added**: no spine producer emits one (harper's
LSP severity is already folded into `kind` by `classify_lsp`) and no spine renderer consumes one — a
field with neither producer nor consumer is dead weight (the no-dead-code rule). The ltex/vale effort
(b) defines the real severity mapping (vale error/warning/suggestion) and owns the decision of whether
that is a new `severity` field or new `DiagnosticKind` variants; adding a field to this struct then is
purely additive and cheap. Recorded here so effort b treats it as a named open point, not an oversight.

---

## 4. The provider seam — trait + `ProviderSet` (`wordcartel/src/diag_provider.rs`)

### 4.1 Trait changes

```rust
pub trait DiagnosticsProvider: std::fmt::Debug {
    /// The engine identity — keys the store slot, the enabled set, and the lens.
    fn source(&self) -> DiagSource;
    /// One-line degrade hint shown when this engine is unavailable (per-engine install copy).
    fn install_hint(&self) -> &'static str;
    fn availability(&self) -> Availability;
    fn ensure_running(&mut self);
    fn configure(&mut self, cfg: ProviderConfig);
    fn notify_change(&mut self, buffer_id: BufferId, version: u64,
        path: Option<std::path::PathBuf>, text: String) -> Accepted;
    fn notify_close(&mut self, buffer_id: BufferId);
    fn reload_dictionary(&mut self);
    fn shutdown(&mut self);
}
```

- `name() -> &'static str` is **replaced** by `source() -> DiagSource`; display goes through
  `source().label()`. One identity, one authority — two parallel name channels would drift.
- `install_hint()` is new: the hint text is per-engine knowledge and moves out of the seam module.
  `HarperLs` returns the existing harper text; the `INSTALL_HINT` const **moves to `harper_ls.rs`**
  (it is harper install copy, also used by the pump's spawn-failure path in that file).
- `notify_change`'s `Accepted` contract, `ProviderConfig`, and `Availability` are unchanged. The
  terminal-`DiagnosticsDone` guarantee (latch invariant) is now stated per provider: `Accepted::Yes`
  from provider P ⟹ at least one terminal `DiagnosticsDone` **with `source == P.source()`** for
  `(buffer_id, version)`.

### 4.2 `ProviderSet` — the registry

```rust
/// The registered diagnostic engines, identified by `DiagSource`. Insertion order is the lens
/// cycle order (core catalog order — harper first). Hermetic default: empty (no thread, no
/// process, no emissions — the role `NullProvider` used to play).
#[derive(Debug, Default)]
pub struct ProviderSet { entries: Vec<ProviderEntry> }

#[derive(Debug)]
struct ProviderEntry { enabled: bool, provider: Box<dyn DiagnosticsProvider> }
```

API (all thin delegation; fields private, accessors only — house style):

```rust
impl ProviderSet {
    /// Register an engine. Duplicate sources are a wiring bug: assert (cold, startup-only path).
    pub fn install(&mut self, provider: Box<dyn DiagnosticsProvider>, enabled: bool);
    /// All registered sources, in cycle order.
    pub fn sources(&self) -> impl Iterator<Item = DiagSource> + '_;
    /// Enabled sources, in cycle order.
    pub fn enabled_sources(&self) -> impl Iterator<Item = DiagSource> + '_;
    pub fn is_enabled(&self, source: DiagSource) -> bool;
    /// Flip enablement. Returns false when `source` is not registered (caller shows status).
    pub fn set_enabled(&mut self, source: DiagSource, on: bool) -> bool;

    // Source-keyed delegation (keeps dispatch's borrows sequential — §7.2):
    pub fn availability(&self, source: DiagSource) -> Option<Availability>;
    pub fn install_hint(&self, source: DiagSource) -> Option<&'static str>;
    pub fn ensure_running(&mut self, source: DiagSource);
    pub fn notify_change(&mut self, source: DiagSource, buffer_id: BufferId, version: u64,
        path: Option<std::path::PathBuf>, text: String) -> Accepted; // No if unknown source
    pub fn configure(&mut self, source: DiagSource, cfg: ProviderConfig);

    // Fan-outs (all registered, enabled or not — a disabled engine still tears down cleanly):
    pub fn notify_close_all(&mut self, buffer_id: BufferId);
    pub fn reload_dictionary_enabled(&mut self); // enabled only; per-engine writers are effort c
    pub fn shutdown_all(&mut self);
}
```

Editor field: `pub diag_provider: Box<dyn DiagnosticsProvider>` → **`pub diag_providers: ProviderSet`**
(the rename forces every call site through review). `Editor::new_from_text` constructs
`ProviderSet::default()` — hermetic construction preserved, one concept fewer.

### 4.3 `NullProvider` is deleted

Its only production role was "a provider-shaped nothing" for hermetic construction; the empty
`ProviderSet` expresses that directly (no sentinel object — the same reasoning as `Option<T>` over
sentinels). Its unit tests are retired; `plugin/host.rs`'s two doc-comment references to it are
updated to cite the empty `ProviderSet` as the hermetic-default precedent.

### 4.4 `HarperLs` changes

- `impl DiagnosticsProvider`: `source() -> DiagSource::Harper`; `install_hint() -> INSTALL_HINT`
  (now a `harper_ls.rs` const).
- All 11 `Msg::DiagnosticsDone` construction sites (§2 table) gain `source: DiagSource::Harper` —
  including `FlushGuard::drop`'s channel drain and every `HarperState` emit (the terminal guarantee
  is now source-tagged end to end).
- `convert_diagnostics` populates the new fields from real wire data:
  `code`: the LSP `code` (string or stringified — the same extraction `classify_lsp` already does;
  hoist it so it is read once), `None` when absent;
  `href`: `codeDescription.href` when present (harper never sends it — the mapping is still written
  and unit-tested with synthetic JSON so effort b's engines get it for free);
  `source: DiagSource::Harper`.
- The 3 `Msg::DiagProviderEvent` sites gain the source tag (§6.3).

### 4.5 `RecordingProvider` — the mock second provider

Extended (still `#[cfg(test)]`):

```rust
pub(crate) struct RecordingProvider {
    calls: Arc<Mutex<Vec<ProviderCall>>>,
    accepted: Accepted,
    availability: Availability,
    source: DiagSource, // NEW — default DiagSource::Plugin("recording")
}
impl RecordingProvider {
    pub(crate) fn with_source(mut self, source: DiagSource) -> Self { … }
}
```

`source()` returns the settable source; `install_hint()` returns a fixed test string. Two-provider
tests install `RecordingProvider::new().with_source(DiagSource::Harper)` and
`…::with_source(DiagSource::Plugin("mock"))` — the mock exercises the `Plugin` arm on purpose (the
future plugin-engine path). `RecordingProvider` still never emits; tests drive the marshaling path by
constructing `Msg::DiagnosticsDone` directly, exactly as today.

---

## 5. The source-partitioned store (`wordcartel/src/diagnostics_run.rs`)

Per-source instances of the existing machinery — the whole current `DiagStore` becomes the per-source
`SourceSlot`; `DiagStore` becomes the map:

```rust
/// Per-buffer diagnostics, partitioned by engine. Each source's slot is a full, independent
/// instance of the single-provider state machine: its own results, its own computed-version
/// validity, its own debounce deadline, its own in-flight latch. Slots exist lazily (created on
/// first arm/apply) and only for enabled engines; disabling an engine removes its slot.
#[derive(Debug, Default, Clone)]
pub struct DiagStore { slots: std::collections::BTreeMap<DiagSource, SourceSlot> }

#[derive(Debug, Default, Clone)]
pub struct SourceSlot {
    pub diagnostics: Vec<Diagnostic>,
    pub computed_version: u64,
    pub recheck_due_at: Option<u64>,
    pub in_flight_version: Option<u64>,
}
```

```rust
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
    pub fn slot(&self, source: DiagSource) -> Option<&SourceSlot>;
    /// Entry-or-default. Callers must not resurrect slots for disabled sources (§6.2 guards).
    pub fn slot_mut(&mut self, source: DiagSource) -> &mut SourceSlot;
    pub fn clear_source(&mut self, source: DiagSource); // slot removed wholesale
    /// A source is due: armed, deadline reached, and no check of ITS OWN in flight. A slot in
    /// flight blocks only itself (the anti-pile-up rule, now per engine — a slow engine no
    /// longer starves a fast one's recheck).
    pub fn any_due(&self, now: u64) -> bool;
    pub fn due_sources(&self, now: u64) -> impl Iterator<Item = DiagSource> + '_;
    /// Earliest armed deadline among slots with no check in flight — the timers feed
    /// (per-source generalization of the A3 exclude-while-in-flight rule).
    pub fn due_deadline(&self) -> Option<u64>;
}
```

Notes:
- The map is private; tests and probes that today poke `b.diagnostics.diagnostics` migrate to
  `b.diagnostics.slot_mut(DiagSource::Harper).diagnostics` (site inventory §13).
- `diag_due(store, now, _version)` is replaced by `store.any_due(now)` (the dead `_version` param
  goes away). The old single-slot semantics fall out as the one-slot special case.
- Arming is per enabled source. The free function that call sites use:

```rust
/// Arm every ENABLED engine's slot on the active buffer — the multi-provider generalization of
/// the old single `store.arm`. Callers: `arm_if_edited` (cfg debounce), `set_render_mode` and
/// `recheck_diagnostics` (debounce 0).
pub fn arm_enabled(editor: &mut Editor, now: u64, debounce_ms: u64) {
    let sources: Vec<DiagSource> = editor.diag_providers.enabled_sources().collect();
    let store = &mut editor.active_mut().diagnostics;
    for s in sources { store.slot_mut(s).arm(now, debounce_ms); }
}
```

- Wholesale resets stay wholesale: `save.rs`'s `new_buf.diagnostics = DiagStore::new()` is unchanged
  in shape and now clears every engine's slots at once (correct: the content changed wholesale).
- `retain_unignored` refilters **every slot** in place (the ignore union — personal dictionary ∪
  session ignores — is client-side and engine-agnostic; a word the user dictionaried must vanish
  from every engine's Spelling view). `retain_over_union` itself is unchanged.

**Staleness invariants, restated per source (each is the existing invariant, instanced):**
1. *Latch:* `slot(s).in_flight_version == Some(v)` ⟹ provider `s` guaranteed a terminal
   `DiagnosticsDone { source: s, version: v }`; the latch is set only on `Accepted::Yes`.
2. *Version gate:* a result stores into `slot(s)` only if `document.version == msg.version`; the
   latch for `(s, v)` clears regardless (the check completed).
3. *Validity:* `slot(s).valid_for(version)` gates painting/nav for source `s` independently.
4. *Independence:* no operation on slot A reads or writes slot B — a slow engine's stale result is
   dropped by ITS guard while a fast engine's current result applies (the effort's acceptance bar).

---

## 6. Marshaling — message, apply path, provider events

### 6.1 `Msg::DiagnosticsDone` gains `source`

```rust
DiagnosticsDone {
    buffer_id: crate::editor::BufferId,
    version: u64,
    source: wordcartel_core::diagnostics::DiagSource,
    diagnostics: Vec<wordcartel_core::diagnostics::Diagnostic>,
},
```

The `Debug` impl adds `.field("source", source)`. The reduce arm stays a thin delegation:

```rust
Msg::DiagnosticsDone { buffer_id, version, source, diagnostics } => {
    crate::diagnostics_run::apply_diagnostics_done(editor, buffer_id, version, source, diagnostics);
}
```

### 6.2 `apply_diagnostics_done` routes by source

```rust
/// Version-gated, source-routed apply: store into the source's slot only if `version` is still
/// current for `buffer_id`; clear THAT source's in-flight latch regardless (the check completed).
/// A result for a source that is no longer enabled is dropped wholesale — `set_engine_enabled(off)`
/// already removed the slot, and a late terminal must not resurrect it.
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
        if let Some(slot) = b.diagnostics.slot(source) {
            if slot.in_flight_version == Some(version) {
                b.diagnostics.slot_mut(source).in_flight_version = None;
            }
        }
    }
}
```

(The latch-clear is written through `slot`/`slot_mut` so a Done for a never-armed source does not
create a slot just to hold `None` — implementation may fold this with an `if let` on an internal
`get_mut`; the observable contract is: no slot is created unless the result stores.)

### 6.3 `Msg::DiagProviderEvent` gains `source`

```rust
/// A `DiagnosticsProvider` lifecycle event — restart re-arm / degradation hint, per engine.
DiagProviderEvent { source: DiagSource, event: crate::diag_provider::ProviderEvent },
```

`apply_provider_event(editor, source, ev, clock)`:
- `Restarted` → `status = format!("{} restarted", source.label())`; re-arm **only that source's
  slot** on the active buffer, gated on `should_run_diagnostics && is_enabled(source)`.
- `Degraded(hint)` → status = hint verbatim (the hint is already per-engine copy).
`harper_ls.rs`'s three emit sites tag `DiagSource::Harper`.

---

## 7. Dispatch fan-out (`diagnostics_run::dispatch_diagnostics`)

New signature `dispatch_diagnostics(editor: &mut Editor, now: u64)` — `now` selects which sources are
due (per-slot deadlines). The timers row stays thin:

```rust
// timers.rs::advance — the diagnostics row
if crate::diagnostics_run::should_run_diagnostics(editor)
    && editor.active().diagnostics.any_due(now)
{ crate::diagnostics_run::dispatch_diagnostics(editor, now); }

// timers.rs::diag_deadline
fn diag_deadline(e: &Editor, _now: u64) -> Option<u64> {
    if crate::diagnostics_run::should_run_diagnostics(e)
    { e.active().diagnostics.due_deadline() } else { None }
}
```

### 7.1 The dispatch body

```rust
/// Snapshot the active buffer ONCE, then hand it to every enabled engine whose slot is due —
/// consuming each slot's deadline as its first act (no spin on Unavailable), latching each slot
/// independently on Accepted::Yes only (per-source latch invariant §5).
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
        return; // no latches; nothing outstanding
    }
    for source in due { dispatch_one(editor, source, buffer_id, version, &path, &text); }
}
```

`dispatch_one` is the extracted per-provider leg (keeps both functions comfortably under the
100-line `too_many_lines` gate):

```rust
fn dispatch_one(editor: &mut Editor, source: DiagSource, buffer_id: BufferId, version: u64,
    path: &Option<std::path::PathBuf>, text: &str) {
    use crate::diag_provider::{Availability, Accepted};
    editor.active_mut().diagnostics.slot_mut(source).recheck_due_at = None; // consumed
    editor.diag_providers.ensure_running(source);
    match editor.diag_providers.availability(source) {
        Some(Availability::Unavailable) | None => { show_install_hint(editor, source); return; }
        Some(Availability::Starting) => {
            editor.status = format!("starting {}…", source.label()); // no silent wait
        }
        Some(Availability::Idle) | Some(Availability::Ready) => {}
    }
    // Per-source LATCH INVARIANT: latch ONLY on Accepted::Yes (an Accepted::No means no terminal
    // will ever arrive for this source — latching would wedge THIS engine permanently).
    match editor.diag_providers.notify_change(source, buffer_id, version, path.clone(),
        text.to_string()) {
        Accepted::Yes => {
            editor.active_mut().diagnostics.slot_mut(source).in_flight_version = Some(version);
        }
        Accepted::No => show_install_hint(editor, source),
    }
}
```

### 7.2 Borrow discipline and cost

- Borrows are sequential, never simultaneous: each `editor.diag_providers.…(source, …)` call ends
  before the next `editor.active_mut()` — that is *why* `ProviderSet` exposes source-keyed delegation
  instead of handing out `&mut dyn` references across store mutations.
- Cost stays hot-path-lawful: the snapshot is O(document) **once**, Review-gated and debounced (same
  as today); `notify_change` clones the text per enabled engine — bounded by the enabled-engine count
  (exactly 1 in production this effort). If a profiled need appears when real engine #2 lands, the
  trait's `text: String` can move to `Arc<str>` then; not speculatively now.
- All provider methods remain non-blocking `cmd_tx.send` (blocking `recv` stays in each provider's
  worker thread). Idle stays free: deadlines exist only on armed slots; slots arm only in Review with
  checking enabled; disabled sources have no slots at all.

### 7.3 Per-source install hint

`diag_hint_shown: bool` → `diag_hint_shown: std::collections::BTreeSet<DiagSource>` (editor field —
"engines whose degrade hint has been shown this Review entry"). `set_render_mode` clears the set on
entering Review (the existing reset point). The helper becomes:

```rust
/// Surface an engine's install/degrade hint at most once per deliberate Review entry —
/// informative, not naggy, now per engine.
fn show_install_hint(editor: &mut Editor, source: DiagSource) {
    if editor.diag_hint_shown.insert(source) {
        if let Some(hint) = editor.diag_providers.install_hint(source) {
            editor.status = hint.into();
        }
    }
}
```

---

## 8. The switchable lens — editor state, routing, render

### 8.1 State + the one setter (contract law 6)

```rust
// editor.rs — near the other diagnostics fields
/// The analysis lens: which engine's diagnostics the writer is currently looking through.
/// Editor-level (a way of looking, not document state — like the keymap, unlike folds). Inline
/// underlines, diag_next/diag_prev, quick_fix, and the REVIEW status label all follow it; only
/// one engine's view is ever rendered (the locked never-merge decision). Session-only (the
/// render-mode precedent) — the durable default is the configured engine list's first entry.
pub active_analysis_source: wordcartel_core::diagnostics::DiagSource,
```

Default in `new_from_text`: `DiagSource::Harper`. `run()` seeds it to the first ENABLED source in
cycle order (falls back to `Harper`, inert, when nothing is enabled).

```rust
/// The single setter for the analysis lens (command-surface contract law 6) — the set-per-state
/// primitives, the cycle, and any future profile/plugin all route here. Refuses a disabled
/// engine: the lens ranges over enabled engines only.
pub fn set_analysis_source(&mut self, source: wordcartel_core::diagnostics::DiagSource) {
    if !self.diag_providers.is_enabled(source) {
        self.status = format!("{} is not enabled", source.label());
        return;
    }
    self.active_analysis_source = source;
    self.status = format!("analysis: {}", source.label());
}
```

Invariant: whenever at least one engine is enabled, `active_analysis_source` names an enabled engine
(`set_analysis_source` refuses disabled targets; `set_engine_enabled` relocates the lens on disable AND
on enable-when-the-lens-is-parked-on-a-disabled-engine — §8.4, both transitions). With zero enabled
engines the field retains its last value and is inert (nothing arms, nothing paints, nav gates return
early); the first subsequent enable relocates the lens onto it.

The cycle lives beside the dispatch code:

```rust
// diagnostics_run.rs
/// Advance the lens to the next enabled engine in cycle order (registration order), wrapping.
/// With fewer than two enabled engines this is a status no-op — honest, not silent.
pub fn cycle_analysis_source(editor: &mut Editor) {
    let enabled: Vec<DiagSource> = editor.diag_providers.enabled_sources().collect();
    if enabled.len() < 2 { editor.status = "no other analysis engine".into(); return; }
    let cur = editor.active_analysis_source;
    let idx = enabled.iter().position(|s| *s == cur).unwrap_or(0);
    editor.set_analysis_source(enabled[(idx + 1) % enabled.len()]);
}
```

### 8.2 Lens-routed reads — one helper, four consumers

The four sites that today do the `should_show && valid_for` dance against the single store (`render.rs`
gather, `quick_fix`, `diag_next`, `diag_prev`) all route through one helper:

```rust
// diagnostics_run.rs
/// The active lens's paintable diagnostics: Some(slice) only when the Review/show gate passes AND
/// the lens engine's slot is valid for the current document version. The single source of truth
/// for "what the lens shows" — render gather, quick_fix, and diag nav all consume it.
pub fn active_lens_diags(editor: &Editor) -> Option<&[Diagnostic]> {
    if !should_show_diagnostics(editor) { return None; }
    let b = editor.active();
    b.diagnostics.slot(editor.active_analysis_source)
        .filter(|s| s.valid_for(b.document.version))
        .map(|s| s.diagnostics.as_slice())
}
```

- `render.rs` gather: `diag_active`/`diag_all` become `let diag_all =
  crate::diagnostics_run::active_lens_diags(editor).unwrap_or(&[]); let diag_active =
  !diag_all.is_empty();` — `RowCtx.diag_all`, the `partition_point` windowing, the face-by-`kind`
  underline, and `use_placed` are all unchanged (one lens ⟹ one notation at a time, by construction).
- `quick_fix` / `diag_next` / `diag_prev` (registry.rs): the guard prelude collapses to
  `let Some(diags) = active_lens_diags(c.editor) else { …status/return… }` (quick_fix keeps its "no
  diagnostic here" status; nav keeps its silent early return); the caret-relative find, fold-unfold,
  selection move, and `open_diag` tail are unchanged. `DiagOverlay` and `diag_apply_selected` are
  untouched — the anchor now carries `source` by construction (it was cloned from the lens slot), and
  the add-dict path's `reload_dictionary` call becomes `diag_providers.reload_dictionary_enabled()`
  (harper re-reads its userDictPath; other engines treat it best-effort; per-engine writers are
  effort c).
- `recheck_diagnostics`: body becomes `if should_run_diagnostics { arm_enabled(c.editor, now, 0) }` —
  rechecks every enabled engine (a "recheck" means "recheck my checkers", not one lens).

### 8.3 Status bar

`render_status.rs::status_left_text`, Review arm — attribution follows the LENS, shown only when that
engine is live (the existing asserts-a-working-checker rule, per engine):

```rust
crate::editor::RenderMode::Review => {
    let lens = editor.active_analysis_source;
    if editor.diag_providers.availability(lens)
        == Some(crate::diag_provider::Availability::Ready)
    { format!("REVIEW · {}", lens.label()).into() } else { "REVIEW".into() }
}
```

### 8.4 Per-engine enablement — state + the one setter

Enablement lives in `ProviderSet` (the entry's `enabled` flag — one structure owns registration and
enablement; no parallel set to drift). The shared setter:

```rust
// diagnostics_run.rs
/// The single setter for per-engine enablement (contract law 6) — the toggle command and startup
/// config seeding both express enablement through ProviderSet state; runtime mutation routes
/// here. Disable: remove the engine's slot from EVERY buffer (underlines drop immediately; a
/// late in-flight terminal is dropped by apply's enabled guard) and relocate the lens if it
/// pointed here. Enable: arm the engine on the active buffer when Review is live, and — if the
/// lens was parked on a now-disabled engine (the re-enable-after-disable-to-zero path) — relocate
/// it to the engine just enabled, so §8.1's invariant holds on BOTH transitions, not only disable.
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
        // Lens invariant (§8.1): the only way the lens can name a disabled engine here is a
        // disable-to-zero followed by re-enable — point it at the engine just enabled so its
        // results are visible and reachable. Otherwise keep the current (enabled) lens.
        if editor.diag_providers.is_enabled(editor.active_analysis_source) {
            editor.status = format!("{} enabled", source.label());
        } else {
            editor.set_analysis_source(source); // relocates lens + sets "analysis: {label}"
        }
    } else {
        for b in editor.buffers.iter_mut() { b.diagnostics.clear_source(source); }
        if editor.active_analysis_source == source {
            match editor.diag_providers.enabled_sources().next() {
                Some(next) => editor.set_analysis_source(next), // sets its own status
                None => editor.status = format!("{} disabled — no analysis engine enabled",
                    source.label()),
            }
        } else { editor.status = format!("{} disabled", source.label()); }
    }
}
```

(Disable does NOT `shutdown()` the provider — a running harper stays warm for re-enable within the
session; process teardown remains the loop-exit `shutdown_all`. This mirrors "leave Review keeps the
provider warm" today. Idle-shutdown policy is the ltex effort's problem — design-space ⚠OPEN 6.)

### 8.5 Commands (registry.rs — the Diagnostics registration block)

Three new registrations, all thin delegations:

```rust
// Analysis lens — set-per-state primitive (palette-only) + stateful cycle representative
// (contract rule 8 — the keymap_next / cycle_render_mode precedent). One primitive per
// AVAILABLE core engine: the ltex/vale effort adds its siblings as new rows here.
r.register("analysis_engine_harper", "Analysis Engine: Harper", None, |c| {
    c.editor.set_analysis_source(wordcartel_core::diagnostics::DiagSource::Harper);
    CommandResult::Handled
});
r.register_stateful("analysis_next", "Analysis Engine", Some(MenuCategory::View),
    |e| MenuMark::Value(e.active_analysis_source.label()),
    |c| { crate::diagnostics_run::cycle_analysis_source(c.editor); CommandResult::Handled });
// Per-engine enablement — a 2-state toggle (contract rule 8), palette-only (the placement
// judgment: engine management is not a browse-for menu action while one engine exists; the
// ltex/vale effort revisits menu surfacing — design-space §5's Analysis section idea).
r.register("toggle_engine_harper", "Toggle Harper Engine", None, |c| {
    let on = !c.editor.diag_providers.is_enabled(wordcartel_core::diagnostics::DiagSource::Harper);
    crate::diagnostics_run::set_engine_enabled(c.editor,
        wordcartel_core::diagnostics::DiagSource::Harper, on, c.clock);
    CommandResult::Handled
});
```

No default keybindings are shipped (palette-dispatch; users may bind — hints resolve automatically,
law 7). `MenuMark::Value` takes the `&'static str` from `DiagSource::label` directly.

---

## 9. Config wiring (`wordcartel/src/config.rs` + `app.rs::run`)

### 9.1 `[diagnostics] linters` — the enabled-engine list (now consumed)

Semantics of the already-parsed `DiagnosticsConfig.linters: Option<Vec<String>>`:
- `None` (unset) → every registered core engine enabled (today: `["harper"]`) — zero-config default.
- `Some(list)` → exactly the named engines enabled; names match `DiagSource::config_name()`.
- `Some([])` → no engines enabled (legitimate: diagnostics machinery off without touching `enabled`).
- Unknown names → a startup warning per name (`config: diagnostics.linters — unknown engine "foo"
  (known: harper)`), name skipped — realizing the promise in the existing fold comment (§2 drift 4).
  The known-name catalog is the core-provider install list, so it and the warning text grow with
  effort b automatically.

### 9.2 `[diagnostics.harper]` — the per-engine table shape

`RawDiagnostics` gains a sub-table (serde, `#[serde(default)]`):

```rust
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawHarperEngine { grammar: Option<bool> }
// RawDiagnostics gains: harper: RawHarperEngine,
```

Resolution is a fold-time override into the existing resolved field — `DiagnosticsConfig` keeps its
shape: top-level `diagnostics.grammar` applies first (retained as the compatible spelling), then
`[diagnostics.harper] grammar` overrides it when set. `[diagnostics.harper].grammar` is documented as
the canonical spelling going forward. This gives effort b a purely additive path (`RawLtexEngine`,
`RawValeEngine` tables with their own resolved fields) without a second home for any harper knob.

`dictionary` deliberately stays top-level: it is dual-role (the client-side suppression seed loaded
into `editor.dictionary` — engine-agnostic — AND harper's `userDictPath`); splitting it per-engine is
effort c's per-engine-writers question. `enabled` and `debounce_ms` stay global (the master switch and
the shared edit debounce).

**Command-surface note — grammar-toggle is a pre-existing command-less gap, unchanged (law 2).**
`[diagnostics.harper].grammar` is NOT a new user option: it is a config-file *spelling* of the
already-existing top-level `diagnostics.grammar` key, which has **no command and no runtime
setter/toggle anywhere in the tree today** (verified at HEAD). This effort adds a second spelling of
that same command-less option, not a new toggleable option — so it neither creates nor widens a
command-surface obligation. It is deliberately left command-less by the spine (whose job is plumbing),
exactly as §12 waives the sibling `diagnostics.enabled` master-switch gap; a future curation pass owns
adding grammar/enabled toggle commands together. Cross-referenced in §12 law 2. (Should a
`toggle_grammar_harper` command ever be judged in-scope, it would slot beside `toggle_engine_harper` in
§8.5 — but the consistent choice, given the waived siblings, is the documented deferral.)

### 9.3 Provider assembly in `run()`

The single-provider install block in `app.rs::run` is replaced by one call into the domain module
(keeps the `app.rs` hub inside its 1000-line budget):

```rust
// diagnostics_run.rs
/// Build the core provider catalog (harper today), fold `linters` into per-engine enablement
/// (warning on unknown names), install into `editor.diag_providers`, and seed the default lens
/// (first enabled source in cycle order). Providers spawn nothing here — lazy, as before.
pub fn install_core_providers(editor: &mut Editor, cfg: &crate::config::Config,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>, warns: &mut Vec<String>) { … }
```

Harper's `ProviderConfig` derivation is unchanged (`grammar` now the folded per-engine value,
`dictionary`, `max_file_length: HARPER_MAX_FILE_LENGTH`). Loop-exit teardown becomes
`editor.borrow_mut().diag_providers.shutdown_all();`.

**Call-ordering (finding — unknown-linter warnings must reach `editor.status`).** `app.rs::run`
surfaces the FIRST accumulated warning at a fixed point: `if let Some(w) = warns.first() { editor.status
= w.clone(); }` (right after `build_keymap`). `install_core_providers` appends its unknown-linter
warnings (§9.1) into that **same `warns` vec**, and MUST run **before** that `warns.first()` status
point — otherwise a bad `linters` name is silently swallowed. This moves the provider install earlier
than harper's current post-keymap install site; that is safe because install spawns nothing (lazy — the
harper client thread still starts on first Review dispatch) and needs only `msg_tx` (already created
before keymap build) + `cfg` + `&mut editor`. Concretely: create `msg_tx` (already moved up at HEAD) →
`install_core_providers(&mut editor, &cfg, &msg_tx, &mut warns)` → `build_keymap` → append keymap warns
→ `warns.first()` status point. (`build_keymap`'s own warnings still append after install's, preserving
"first warning wins" with config-linter errors taking precedence — the desired ordering.)

---

## 10. Error handling — status line, never silent, never the console

Unchanged posture, per-engine copy:
- Engine unavailable / notify refused → that engine's `install_hint()`, at most once per Review entry
  per engine (`diag_hint_shown` set).
- Engine starting → `starting {label}…` (no silent wait).
- Provider degraded/restarted → `Msg::DiagProviderEvent { source, … }` → status with the engine label.
- Oversized document → the existing global cap message, all due deadlines consumed (no spin).
- Lens/enable command misuse → explicit status (`"{label} is not enabled"`, `"no other analysis
  engine"`, `"unknown analysis engine: {label}"`).
- IO errors (dictionary append) → unchanged (`add to dictionary failed: {e}`).
No `.unwrap()` on any new fallible path; the only `expect`/`assert` added are the guarded
duplicate-install assert (cold startup wiring) and the payload-source `debug_assert` (§6.2).

---

## 11. Resource behavior

- **Idle is free, unchanged:** deadlines exist only on armed slots of enabled engines; arming happens
  only on real edits in Review (`arm_if_edited`), Review entry, restart events, and explicit recheck —
  all edge-triggered. Disabled engines have no slots, hence no deadlines, hence no wakes.
- **Per-keystroke work:** `arm_if_edited` → `arm_enabled` is O(enabled engines) map inserts (constant
  in practice); nothing else on the hot path changed.
- **O(document) work** (snapshot + per-engine text clone) remains Review-gated + debounced, now ×
  enabled-engine count (1 in production this effort).
- **Memory:** per-buffer diagnostics ≈ Σ per-enabled-engine result sets; slots are dropped on engine
  disable, wholesale-reset on reload/recovery, and freed on buffer close (unchanged ownership).

---

## 12. Command-surface-contract conformance

This effort touches the command surface; conformance, law by law
(`docs/design/command-surface-contract.md`):

- **Law 1 (registry = single source of truth):** the new mutations — lens and per-engine enablement —
  are reachable only through registered commands; no other code path flips them at runtime.
- **Law 2 (every user-settable option is a command):** the lens (runtime option) →
  `analysis_engine_harper` + `analysis_next`; engine enablement (the `[diagnostics] linters` config
  key) → `toggle_engine_harper`. Honest note on two pre-existing command-less gaps, both waived
  consistently (neither predates-nor-widens by this effort; each left to a future curation pass —
  adding them here would be silent scope growth past the approved spine): (a) the `diagnostics.enabled`
  master switch has no command today; (b) `diagnostics.grammar` (grammar-toggle) has no command/setter
  today, and this effort's `[diagnostics.harper].grammar` (§9.2) is merely a second config-file
  *spelling* of that same command-less option — not a new option, so it adds no new law-2 obligation.
  Both remain deliberately command-less this effort.
- **Law 3 (palette exhaustive):** all three commands register normally → the palette-completeness
  invariant test covers them with zero new machinery.
- **Law 4 (menu ⊆ palette):** only `analysis_next` surfaces in the menu (View); it names a registered
  command → in the palette by law 3. No dynamic menu section is added this effort (design-space §5's
  Analysis section is effort-b/e material).
- **Law 5 (keyboard path):** all three are palette-dispatchable; no mouse-only affordance is added.
- **Law 6 (one setter; profiles use it):** lens mutation → `Editor::set_analysis_source` (primitives
  + cycle + any future profile/plugin); enablement mutation → `diagnostics_run::set_engine_enabled`
  (the toggle; startup seeding expresses initial enablement as `ProviderSet::install(…, enabled)` —
  construction, not runtime mutation, matching the clipboard-provider seeding precedent).
- **Law 7 (hints track the active keymap):** free via registration — palette/menu hint resolution is
  registry-generic; the existing re-resolution tests stay green untouched.
- **Rule 8 (multi-state = set-per-state primitives + a stateful representative):** the lens is the
  multi-state option → per-engine set primitives (palette-only) + the `analysis_next` cycle carried in
  the menu with state-in-label (`MenuMark::Value(lens.label())`) — the `keymap_next` /
  `cycle_render_mode` precedent exactly. Per-engine enablement is 2-state → a toggle. Placement
  judgment (the contract's one judgment call): the toggle is palette-only this effort (§8.5 rationale).
- **Rule 9 (presets never the only door):** no preset touches these options.
- **Rule 10 (commands are the plugin spine):** all three are nullary registry commands → dispatchable
  by plugins via the existing command bridge, no special-casing.
- **Enforcing tests exercised:** palette-completeness (extends over the new commands automatically);
  hint re-resolution (unchanged, must stay green); the every-persisted-setting guard (no new
  `SettingsSnapshot` fields — the lens is session-only by the render-mode precedent, enablement's
  durable home is the config file).

---

## 13. Migration-site inventory (completeness checklist for the plan)

Core (`wordcartel-core/src/diagnostics.rs`): `DiagSource` new; `Diagnostic` +3 fields; module doc.

Shell, by file (locate by symbol, not line):
- `diag_provider.rs`: trait (`source`/`install_hint`, drop `name`); `ProviderSet` new; `NullProvider`
  deleted (+ its tests); `INSTALL_HINT` moved out to `harper_ls.rs`; `apply_provider_event` gains
  `source`; `RecordingProvider` gains `source`; `ProviderCall` unchanged.
- `harper_ls.rs`: `INSTALL_HINT` lands here; `source()`/`install_hint()`; `source: DiagSource::Harper`
  on all 11 `DiagnosticsDone` sites (incl. `FlushGuard::drop`) + 3 `DiagProviderEvent` sites;
  `convert_diagnostics` populates `source`/`code`/`href`; inline tests updated (the `dones`/`degraded`
  helpers, `name()` assertion → `source()`).
- `diagnostics_run.rs`: `DiagStore`/`SourceSlot` restructure; `arm_enabled`, `any_due`/`due_sources`/
  `due_deadline`, `dispatch_diagnostics(editor, now)` + `dispatch_one`, `show_install_hint(editor,
  source)`, `apply_diagnostics_done(+source)`, `retain_unignored` over all slots,
  `active_lens_diags`, `cycle_analysis_source`, `set_engine_enabled`, `install_core_providers`;
  tests migrated.
- `editor.rs`: `diag_providers: ProviderSet`; `active_analysis_source`; `diag_hint_shown:
  BTreeSet<DiagSource>` (+ `set_render_mode`'s Review-entry `.clear()` and its arm call →
  `arm_enabled`); `set_analysis_source`; `open_diag` unchanged.
- `app.rs`: `Msg::DiagnosticsDone` + `Msg::DiagProviderEvent` shapes + `Debug` impl + both reduce
  arms; provider install block → `install_core_providers` (before the `warns.first()` status point —
  §9.3); loop-exit `shutdown_all`; inline tests.
- `prompts.rs`: **the second message-delivery path** — `intercept` (under an open modal prompt) has
  its OWN `Msg::DiagnosticsDone`/`Msg::DiagProviderEvent` match arms, separate from `reduce`. Both
  arms migrate identically to app.rs's: `DiagnosticsDone { buffer_id, version, source, diagnostics }`
  → `apply_diagnostics_done(editor, buffer_id, version, source, diagnostics)`; `DiagProviderEvent {
  source, event }` → `apply_provider_event(editor, source, event, clock)`. ALSO its degrade-reaches-
  status test imports `INSTALL_HINT` from `diag_provider` — repoint that import to `harper_ls`
  (const moved, §4.4) and construct the event with a `source` field.
- `timers.rs`: `diag_deadline` → `due_deadline`; `advance`'s diagnostics row → `any_due` +
  `dispatch_diagnostics(editor, now)`; guardrail tests re-anchored.
- `registry.rs`: `quick_fix`/`diag_next`/`diag_prev` → `active_lens_diags`; `recheck_diagnostics` →
  `arm_enabled`; three new registrations (§8.5); inline tests.
- `render.rs`: gather → `active_lens_diags`; painter unchanged.
- `render_status.rs`: Review arm → lens label (§8.3); tests updated.
- `search_ui.rs`: `reload_dictionary` → `reload_dictionary_enabled`; 2 test `Diagnostic {` literals
  (§13.1); tests updated.
- `save.rs`: 2× `notify_close` → `notify_close_all`; wholesale `DiagStore::new()` resets unchanged;
  inline tests migrate to slot accessors AND their 5 `Diagnostic {` literals gain the new fields
  (§13.1).
- `mouse.rs`: **construction-only** — 3 `#[cfg(test)]` `Diagnostic {` literals (§13.1) that fail to
  compile until the new fields are added; no production diagnostics logic here. Also its store-poke
  test helpers migrate to slot accessors if they touch `diagnostics.diagnostics`.
- `diag_overlay.rs`: **construction-only** — 1 `#[cfg(test)]` `Diagnostic {` literal in `tall_diag`
  (§13.1); the `DiagOverlay` production type is otherwise untouched (its `anchor` now carries
  `source` by construction — cloned from a lens slot).
- `workspace.rs`: `notify_close` → `notify_close_all`.
- `e2e.rs`: `Msg::DiagnosticsDone` constructions gain `source`; the `make_diags` helper's
  `Diagnostic {` literal gains the new fields (§13.1); diagnostics probe seeds via
  `slot_mut(DiagSource::Harper)`.
- `plugin/host.rs`: two doc-comment references to `NullProvider` → the empty-`ProviderSet` precedent.
- `tests/harper_ls_integration.rs`: pattern-match gains `source` (assert `DiagSource::Harper`).
- `config.rs`: `RawHarperEngine`; linters consumption semantics doc; the stale "validated later"
  comment replaced by a pointer to `install_core_providers`.

### 13.1 Full `Diagnostic {` construction-site sweep (compile-completeness)

Adding three fields to `Diagnostic` breaks every struct-literal construction. The complete sweep at
HEAD (all `#[cfg(test)]` except `harper_ls.rs`'s production `convert_diagnostics`), per file — the
plan MUST update every one:

| File | Count | Nature |
|---|---:|---|
| `app.rs` | 10 | test literals (reduce/nav/probe tests) |
| `save.rs` | 5 | reload/recovery stale-diagnostics tests |
| `render.rs` | 4 | underline-paint tests |
| `mouse.rs` | 3 | click-on-diagnostic tests |
| `registry.rs` | 3 | diag_next/quick_fix tests |
| `search_ui.rs` | 2 | ignore-union + open-diag tests |
| `harper_ls.rs` | 1 | **production** (`convert_diagnostics` — sets real `source`/`code`/`href`) |
| `diagnostics_run.rs` | 1 (+2 helper fns `spelling`/`grammar`) | test literals |
| `diag_overlay.rs` | 1 | `tall_diag` test helper |
| `e2e.rs` | 1 | `make_diags` helper |

**Total: 31 struct literals + 2 helper-fn bodies across 10 files.** Counts are indicative, not the
completion criterion: the implementer MUST `rg 'Diagnostic\s*\{'` each listed file and update EVERY
literal it returns (excluding `-> Diagnostic {` return-type signatures, which the pattern also matches
— e.g. `diagnostics_run.rs`'s `fn spelling`/`fn grammar` headers), rather than stopping at the count.

Test literals take `source: DiagSource::Harper` (or `Plugin("mock")` where a second source is under
test), `code: None`, `href: None` unless the test asserts on them. `wordcartel-core`'s own
`diagnostics.rs` tests (if any construct literals) are swept the same way. No `Diagnostic {` literal
lives outside these files (verified by `grep -rn "Diagnostic {"` across `wordcartel-core/src`,
`wordcartel/src`, `wordcartel/tests`).

Module-structure/GATE notes: no hub grows — the fan-out lives in `diagnostics_run.rs` (domain module);
new commands are rows in the existing registration block; the reduce/timers arms keep their one-line
delegation shape. `app.rs` (budget 1000) *shrinks* (install block → one call). All new/split functions
sized under the 100-line `too_many_lines` gate. Workspace clippy clean and hand-formatting rules apply
as usual (no rustfmt).

---

## 14. Testing

### 14.1 The mock-provider proof (the effort's acceptance core)

Two `RecordingProvider`s installed as `DiagSource::Harper` + `DiagSource::Plugin("mock")` (the mock
deliberately exercises the `Plugin` arm). In `diagnostics_run.rs` / `diag_provider.rs` /
`registry.rs` `#[cfg(test)]` modules:

1. **Fan-out:** one due dispatch notifies BOTH providers with the same `(buffer_id, version, text)`
   (call logs) and latches both slots independently.
2. **Per-source staleness independence** (the effort's acceptance bar — a slow engine's stale result
   dropped by ITS OWN guard while a fast engine's current result applies). Coherent with the real
   latch model (`apply` stores only when `document.version == msg.version`, and clears in_flight only
   when `in_flight_version == Some(msg.version)`):
   - Dispatch both at v → both slots latched `in_flight = Some(v)`.
   - The document then advances to v+1 (an edit), which re-arms both slots.
   - The SLOW engine's terminal `Done` for v arrives: `document.version (v+1) != v` → NOT stored
     (its stale diagnostics never paint), but `in_flight == Some(v) == msg.version` → the latch
     DOES clear, freeing the slot to re-dispatch for v+1. Its slot's `computed_version` stays at
     whatever it last validly held (its old diagnostics remain hidden via `valid_for(v+1) == false`).
   - The FAST engine's terminal `Done` for v+1 arrives: `document.version (v+1) == v+1` → stored in
     the fast slot, `computed_version = v+1`, its latch clears. The fast lens paints current
     underlines while the slow engine shows nothing until its v+1 recheck lands.
   Assert: the slow slot never stored the v result; both latches cleared correctly; the two slots'
   `computed_version` diverge (fast = v+1, slow = stale) with zero cross-contamination.
3. **Per-source in-flight:** mock latched + harper idle, both armed and due → dispatch notifies harper
   only; mock's deadline is retained (not consumed) and dispatches after its terminal clears the latch.
4. **Per-source Accepted::No:** mock refuses → mock unlatch + mock hint; harper accepted + latched in
   the same dispatch (no cross-engine wedge).
5. **Routing:** `Msg::DiagnosticsDone { source: Plugin("mock"), … }` never touches the Harper slot
   (and vice versa).
6. **Disable/enable:** `set_engine_enabled(off)` clears the engine's slots in all buffers, skips it in
   fan-out, relocates the lens off it; a late Done for the disabled source does not resurrect a slot;
   re-enable arms in Review.
7. **Lens:** `set_analysis_source` refuses a disabled engine; `cycle_analysis_source` cycles
   enabled-only in registration order and status-no-ops with <2 enabled; `active_lens_diags` flips
   between the two seeded slots as the lens moves; `diag_next`/`quick_fix` target only the active
   lens's diagnostics.
8. **Per-source hint latch:** two Unavailable engines each show their own `install_hint` once per
   Review entry; re-entering Review re-arms both hints.
9. **Restart re-arm:** `DiagProviderEvent { source: mock, Restarted }` arms only the mock slot.

### 14.2 Unit and boundary tests

- Core: `DiagSource::label`/`config_name` exhaustive expectations; `Diagnostic` carries
  `source`/`code`/`href` (construction + equality).
- `harper_ls.rs`: `convert_diagnostics` maps LSP `code` (string and numeric) → `code`,
  `codeDescription.href` → `href` (synthetic JSON — harper never sends href, the mapping is for the
  seam), tags `DiagSource::Harper`; flush/close/watchdog terminals carry `source: Harper`.
- Config: `linters` None → harper enabled; `["harper"]` → enabled; `[]` → none; unknown name → warn +
  skip; `[diagnostics.harper] grammar=false` overrides top-level into `ProviderConfig.grammar`.
- Status bar: `REVIEW · Harper` only when the LENS engine is Ready; lens on Plugin("mock") shows its
  label; non-Ready lens → plain `REVIEW`.
- Timers: `due_deadline` excludes per-source in-flight; armed-outside-Review stays `None` (the
  existing spin-class guardrail, re-anchored).
- Existing migrated suites stay green: dispatch latch tests, apply/ignore-union tests, arm_if_edited,
  save-reload/recovery stale-diagnostics tests, e2e journeys + diagnostics-landing probe,
  `harper_ls_integration` (ignored-by-default live test), palette-completeness, hint re-resolution.

### 14.3 e2e journey (TestBackend)

One in-process journey: Review mode, inject `DiagnosticsDone` for two sources at the current version,
assert underlines painted for the default lens only; dispatch `analysis_next` (two enabled engines in
the test registry); assert the painted underline set switches — the switchable lens observed at the
real `reduce → advance → render` loop.

Pre-merge report additionally runs `scripts/smoke/run.sh` and quotes its one-line summary verbatim
(mandatory-run, advisory-pass).

---

## 15. Non-goals — the deferred efforts (named, not designed)

- **(b) Real engines:** ltex-ls-plus provider (JVM lazy-start/warming/idle-shutdown) and vale/vale-ls
  provider; their `[diagnostics.<engine>]` tables; their `analysis_engine_*`/`toggle_engine_*` rows;
  the severity decision (§3.3).
- **(c) Viewing/action delta:** doc-link "learn more" UI + detail region (the `href` field lands now,
  consumer lands there); per-engine dictionary/rule writers; `workspace/executeCommand` relay;
  more-suggestions population; the source-grouped "see everything" combined list.
- **(d) `wc.async`:** the one-shot subprocess primitive — zero dependency on this spine.
- **(e) Plugin-declared engines + plugin dynamic-menu rows:** the `Plugin(&'static str)` arm and the
  registration-order cycle are the prepared seams; nothing else ships now.
- Also out: any change to `DiagOverlay` rows/interaction, hover/completion (per the scan's vale-ls
  trap — no prose hover exists to wire), bundling any engine, and persisting the lens in
  `SettingsSnapshot`.

---

## 16. Design self-review (done before hand-off; re-run after folding the Codex-gate findings)

- **Placeholder scan:** no TBD/TODO/`…`-as-content remains; every code block names real symbols
  verified in §2 (the `…` in the `install_core_providers` signature block is an intentional body
  elision — its behavior is fully specified in §9.3).
- **Internal consistency:** `DiagSource` derives support every stated use (BTreeMap key, BTreeSet
  hint latch, `MenuMark::Value(&'static str)` via `label`); dispatch's borrow sequencing matches the
  `ProviderSet` delegation API; the lens invariant is maintained by exactly the two setters that can
  affect it; apply's disabled-source guard matches disable's slot-clearing. The §14.1-item-2 test now
  matches the real latch model exactly (store gated on `version` equality; latch cleared on
  `in_flight == msg.version` — never on a mismatched stale version). §9.2/§12-law-2 tell one
  consistent grammar-waiver story; §9.3's warns-ordering matches app.rs's real `warns.first()` point.
- **Scope check:** everything in §1 Goals is designed; nothing from §15 is designed; the two
  adjacent command-less gaps (`diagnostics.enabled` master switch; `diagnostics.grammar` toggle) are
  explicitly and consistently declined (§12 law 2, §9.2) — not filled by the spine.
- **Migration-completeness (the gate's focus):** §13 now covers both message-delivery paths
  (app.rs::reduce AND prompts.rs::intercept), and §13.1 is a full `Diagnostic {` construction-site
  sweep (31 struct literals + 2 helper-fn bodies across 10 files — verified by grep across
  `wordcartel-core/src`, `wordcartel/src`, `wordcartel/tests`), so no site fails to compile silently.
- **Ambiguity check:** cycle order (registration order), default lens (first enabled, Harper
  fallback), disable-while-in-flight (slot cleared; late terminal dropped), zero-enabled behavior
  (inert lens + honest statuses), text-clone cost stance (accepted at N=1, `Arc<str>` named as the
  future lever), config-linter-warning ordering (before the status point) — all pinned.

---

## 17. History

- **2026-07-12 — §8.4 lens-invariant amendment (post-implementation, pre-merge).** The Fable
  whole-branch gate's probe found that `set_engine_enabled`'s enable branch, as originally specified,
  did not maintain §8.1's lens invariant on one path: a disable-to-zero (lens parked on the
  last-disabled engine) followed by re-enabling an engine left `active_analysis_source` naming a
  *disabled* engine — its results invisible (`active_lens_diags → None`) and unreachable by the cycle
  (a single enabled engine → no-op), recoverable only via the `analysis_engine_*` set-primitive. The
  original §8.4 code faithfully matched the spec but contradicted §8.1's stated invariant (a
  spec-internal inconsistency). Unreachable in the shipped single-engine (harper-only) surface;
  reachable via the test mock and effort b's real multi-engine future. **Resolution (human-approved):**
  the enable branch now relocates the lens to the just-enabled engine when the current lens is not
  enabled, so §8.1 holds on both the enable and disable transitions. `set_engine_enabled` remains the
  single enablement setter (contract law 6). Change scope: ~4 lines + one regression test
  (`re_enable_after_zero_relocates_lens_onto_enabled_engine`); no other section affected.
