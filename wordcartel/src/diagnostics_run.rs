//! Diagnostics runtime (shell): per-source-partitioned store, pure debounce helpers,
//! worker dispatch (Task 4), version-gated apply (Task 4), dictionary IO.
use wordcartel_core::diagnostics::{Diagnostic, DiagnosticKind, DiagSource};
use crate::editor::{BufferId, Editor};

/// One engine's diagnostics state on one buffer: the current results, the version they were
/// computed against, and the debounce/in-flight latch (spec §5) — an INSTANCE of the machinery
/// the flat pre-SPINE `DiagStore` used to own directly, now held once per `DiagSource`.
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

/// Per-buffer diagnostics, partitioned by engine (`DiagSource`). A source with no entry has
/// never been armed/computed — equivalent to an all-default `SourceSlot`, but without paying for
/// one until the source is actually used (spec §5, multi-provider generalization of the old flat
/// single-slot store).
#[derive(Debug, Default, Clone)]
pub struct DiagStore { slots: std::collections::BTreeMap<DiagSource, SourceSlot> }

impl DiagStore {
    /// An empty store — no source has a slot yet.
    pub fn new() -> Self { DiagStore::default() }
    /// The slot for `source`, if it has ever been touched (armed, computed, or latched).
    pub fn slot(&self, source: DiagSource) -> Option<&SourceSlot> { self.slots.get(&source) }
    /// The slot for `source`, creating a fresh default one on first touch.
    /// Callers must not resurrect slots for disabled sources — a result routed here for a source
    /// that is not enabled would leave a phantom empty slot behind (spec §6.2: the apply path reads
    /// `slot(source)` first and only touches `slot_mut` when the source is enabled and current).
    pub fn slot_mut(&mut self, source: DiagSource) -> &mut SourceSlot {
        self.slots.entry(source).or_default()
    }
    /// Drop `source`'s slot entirely (e.g. the engine was disabled/uninstalled).
    pub fn clear_source(&mut self, source: DiagSource) { self.slots.remove(&source); }
    /// Every slot, mutably — for whole-store operations (the ignore-union refilter).
    pub fn slots_mut(&mut self) -> impl Iterator<Item = &mut SourceSlot> { self.slots.values_mut() }
    /// Earliest armed deadline among slots with NO check in flight (per-source A3 gate) — `None`
    /// when nothing is armed or every armed slot is mid-check.
    pub fn due_deadline(&self) -> Option<u64> {
        self.slots.values()
            .filter(|s| s.in_flight_version.is_none())
            .filter_map(|s| s.recheck_due_at).min()
    }
    /// Whether ANY slot's re-check is due at `now` (armed, reached, not in flight).
    pub fn any_due(&self, now: u64) -> bool { self.due_sources(now).next().is_some() }
    /// Every source whose re-check is due at `now` (armed, reached, not in flight), in
    /// `DiagSource`'s `Ord` (BTreeMap iteration) order.
    pub fn due_sources(&self, now: u64) -> impl Iterator<Item = DiagSource> + '_ {
        self.slots.iter()
            .filter(move |(_, s)| s.in_flight_version.is_none()
                && matches!(s.recheck_due_at, Some(t) if now >= t))
            .map(|(src, _)| *src)
    }
}

/// Arm every ENABLED engine's slot on the active buffer — the multi-provider generalization of
/// the old single `store.arm`. Callers: `set_render_mode`'s arm-on-enter-Review, `arm_if_edited`'s
/// per-edit re-arm, `recheck_diagnostics`. (Dispatch itself stays single-provider-shaped this
/// task — Task 5 fans the actual worker send out over enabled sources.)
pub fn arm_enabled(editor: &mut Editor, now: u64, debounce_ms: u64) {
    let sources: Vec<DiagSource> = editor.diag_providers.enabled_sources().collect();
    let store = &mut editor.active_mut().diagnostics;
    for s in sources { store.slot_mut(s).arm(now, debounce_ms); }
}

/// Smallest of the deadline terms; None terms ignored.
pub fn next_deadline(terms: &[Option<u64>]) -> Option<u64> {
    terms.iter().flatten().copied().min()
}

/// Compute gate: diagnostics arm/dispatch only when the feature is enabled AND the active buffer
/// is in the Review render mode. (Spec §2.1.)
pub fn should_run_diagnostics(editor: &Editor) -> bool {
    editor.diag_cfg.enabled && editor.active().view.mode == crate::editor::RenderMode::Review
}

/// Display gate: underlines paint under exactly the same predicate. Distinct name for the distinct
/// role (compute vs paint); delegates so the two cannot drift.
pub fn should_show_diagnostics(editor: &Editor) -> bool { should_run_diagnostics(editor) }

/// The single source of truth for "what the switchable lens shows" (spec §8.2): the active
/// buffer's diagnostics for `editor.active_analysis_source`, but only when the Review/show gate
/// passes AND that source's slot is `valid_for` the current document version — i.e. exactly the
/// slice `render`'s underline painter and the quick-fix/nav commands are allowed to act on. Every
/// other engine's slot stays computed but invisible until the lens is switched onto it (the
/// locked never-merge decision: one source painted at a time).
pub fn active_lens_diags(editor: &Editor) -> Option<&[Diagnostic]> {
    if !should_show_diagnostics(editor) { return None; }
    let b = editor.active();
    b.diagnostics.slot(editor.active_analysis_source)
        .filter(|s| s.valid_for(b.document.version))
        .map(|s| s.diagnostics.as_slice())
}

/// The single diagnostics re-arm seam (spec §2.2 item 1). After a `reduce` message, if the SAME
/// buffer is still active AND its document.version advanced since the pre-dispatch snapshot, arm the
/// debounced recheck — but only when in Review with checking enabled. Wraps every `reduce` exit path
/// (interceptor early-returns AND the normal tail), so every active-buffer edit re-arms exactly once,
/// with no per-path enumeration, no double-arm, and no false arm on a buffer switch (§2.3).
pub fn arm_if_edited(editor: &mut Editor, before_id: BufferId, before_version: u64,
    clock: &dyn wordcartel_core::history::Clock) {
    if editor.active().id == before_id
        && editor.active().document.version != before_version
        && should_run_diagnostics(editor)
    {
        let debounce_ms = editor.diag_cfg.debounce_ms;
        // Re-arm every enabled engine on the edit (spec §5): each source debounces independently.
        arm_enabled(editor, clock.now_ms(), debounce_ms);
    }
}

/// E10 §6: observe the summon-predicate TRANSITION at the reduce-exit seam (the
/// arm_if_edited chokepoint — every normal reduce exit; the sole bypass is the debug-only
/// WCARTEL_SMOKE_PANIC branch, a panic path). Arm the idle-suspend deadline on leaving
/// Review (mode change OR buffer switch), clear it on re-entry. Edge-triggered, never
/// level-triggered (the resource law). The arm gate is ENABLEMENT only — started-ness is
/// guarded provider-side (spec §6: no accessor; LspProvider::suspend no-ops unless
/// SUSPENDABLE && started).
pub fn idle_shutdown_track(editor: &mut Editor, summoned_before: bool,
    clock: &dyn wordcartel_core::history::Clock) {
    let summoned_now = should_run_diagnostics(editor);
    if summoned_before && !summoned_now {
        if editor.diag_providers.is_enabled(DiagSource::LTeX)
            && editor.diag_cfg.ltex_idle_shutdown_min > 0
        {
            editor.diag_idle_due = Some(clock.now_ms()
                .saturating_add(editor.diag_cfg.ltex_idle_shutdown_min.saturating_mul(60_000)));
        }
    } else if !summoned_before && summoned_now {
        editor.diag_idle_due = None;
    }
}

/// E10 §6: the one-shot fire — reached the due ⇒ clear it and suspend the heavy engines
/// (only SUSPENDABLE providers act). No re-arm until the next leaving-Review transition.
pub fn diag_idle_fire(editor: &mut Editor, now: u64) {
    if matches!(editor.diag_idle_due, Some(due) if now >= due) {
        editor.diag_idle_due = None;
        editor.diag_providers.suspend_all_idle_heavy();
    }
}

/// Fan out to every ENABLED source whose re-check is due at `now` (spec §7): snapshot the active
/// buffer ONCE, apply the whole-document `DIAG_MAX_SEND_BYTES` cap once, then hand the snapshot to
/// `dispatch_one` per due source — each source consumes its own slot's deadline and latches
/// independently (spec §5.1), so one source's `Accepted::No`/unavailable never blocks another's.
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
        editor.set_status_full(crate::status::StatusKind::Warning, "document too large for grammar checking",
            crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
        return;
    }
    for source in due { dispatch_one(editor, source, buffer_id, version, &path, &text); }
}

/// Dispatch a single due `source` against the already-snapshotted buffer (Effort A seam,
/// generalized to per-source in Task 5). Consumes the armed deadline, ensures the provider is
/// running, honors `Unavailable`/`Starting` with an explicit status (no silent wait, spec §4.3),
/// and sets `in_flight_version` ONLY on `Accepted::Yes` — the provider guarantees a (possibly
/// empty) `Msg::DiagnosticsDone` for every accepted version (latch invariant, spec §5.1); on
/// `Accepted::No` the thread died between the availability read and the send, so latching here
/// would wedge diagnostics permanently — leave the latch clear (a fresh dispatch retries) and
/// surface the degrade hint instead.
fn dispatch_one(editor: &mut Editor, source: DiagSource, buffer_id: BufferId, version: u64,
    path: &Option<std::path::PathBuf>, text: &str) {
    use crate::diag_provider::{Availability, Accepted};
    editor.active_mut().diagnostics.slot_mut(source).recheck_due_at = None; // consumed
    editor.diag_providers.ensure_running(source);
    // `None` (no entry registered for this source) is treated as unavailable — same as the
    // provider itself reporting `Unavailable`.
    match editor.diag_providers.availability(source) {
        Some(Availability::Unavailable) | None => { show_install_hint(editor, source); return; }
        Some(Availability::Starting) => {
            editor.set_status(crate::status::StatusKind::Info, format!("starting {}…", source.label())); // no silent wait (spec §4.3)
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

/// Surface `source`'s install hint at most once per deliberate Review entry (`diag_hint_shown` is a
/// per-source latch, cleared in `set_render_mode` on entering Review). Spec §9 — informative, not
/// naggy: each engine gets to explain itself once, independently of the others.
fn show_install_hint(editor: &mut Editor, source: DiagSource) {
    if editor.diag_hint_shown.insert(source) {
        if let Some(hint) = editor.diag_providers.install_hint(source) {
            editor.set_status_full(crate::status::StatusKind::Warning, hint,
                crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
        }
    }
}

/// The client-side ignore union — personal dictionary ∪ session ignores — lowercased for
/// case-insensitive membership (spec §7.3/§7.4). Empty ⟹ nothing is suppressed (the common case).
fn ignore_union_lower(editor: &Editor) -> std::collections::HashSet<String> {
    editor.dictionary.iter().chain(editor.session_ignores.iter())
        .map(|w| w.to_lowercase()).collect()
}

/// E11 §5.3: the two text units that together identify ONE dismissed occurrence — the enclosing
/// sentence and the enclosing source line. Neither alone is enough: a sentence key alone silences
/// the same wording wherever that sentence is repeated (and cannot separate a heading from prose
/// that quotes it); a line key alone cannot separate two sentences sharing one long line. Both are
/// required EQUAL, and both are derived parse-free, so the filter is safe on a buffer whose block
/// tree is deliberately stale (the lazy-reparse law).
///
/// The pair carries no notion of Markdown role, by design: nothing classifies the CANDIDATE, so
/// there is no classification step to disagree with the one taken at dismiss time. The residue is
/// a named collision class — two occurrences whose sentence AND line units are byte-identical are
/// indistinguishable and are suppressed together, whatever their roles.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct DismissKey {
    /// The enclosing sentence, as `textobj::sentence_bounds` cuts it from the blank-line window.
    pub sentence: String,
    /// The enclosing source line, without its trailing newline.
    pub line: String,
}

/// E11 §5.3: the session-dismissal set — `(source, code-or-empty, pair key)` triples. An absent
/// `Diagnostic::code` keys as the empty string, so a code-less engine still gets per-occurrence
/// dismissal rather than a wildcard.
pub type DismissSet = std::collections::HashSet<(DiagSource, String, DismissKey)>;

/// E11 §5.3: BOTH parse-free units at `pos` — the blank-line-window sentence + the source
/// line. Rope + `textobj` only (no block tree — safe on any buffer; the lazy-reparse law).
///
/// Out-of-range `pos` is clamped to the document end; a slice that cannot be taken yields an
/// empty unit rather than a panic (the units are compared, never indexed back into the text).
///
/// # Examples
/// Crate-private, so rustdoc does not collect this as a doctest — the EXECUTED version is
/// `dismissal_units_pair_sentence_and_line` below.
/// ```ignore
/// let buf = wordcartel_core::buffer::TextBuffer::from_str("One two. Three four.\n");
/// let k = dismissal_units_at(&buf, 12);
/// assert_eq!(k.sentence, "Three four.");
/// assert_eq!(k.line, "One two. Three four.");
/// ```
pub(crate) fn dismissal_units_at(buf: &wordcartel_core::buffer::TextBuffer, pos: usize)
    -> DismissKey {
    let pos = pos.min(buf.len());
    let line = buf.byte_to_line(pos);
    // Expand to the nearest blank-line/document boundaries (source-level paragraph). "Blank" is
    // TRIM-empty, not strict-empty: CommonMark treats a whitespace-only line as blank, and both
    // shipped paragraph walkers (`nav.rs`, `ventilate.rs`) agree — a strict-empty test silently
    // swallowed the preceding block into the key. One predicate, both directions, one function:
    // the store and filter sides cannot disagree.
    let blank = |n: usize| crate::lines::line_text(buf, n).trim().is_empty();
    let mut first = line;
    while first > 0 && !blank(first - 1) { first -= 1; }
    let total = crate::lines::total_logical_lines(buf);
    let mut last = line;
    while last + 1 < total && !blank(last + 1) { last += 1; }
    let win_start = crate::lines::line_start(buf, first);
    let win_end = if last + 1 < total { crate::lines::line_start(buf, last + 1) }
        else { buf.len() };
    let window = buf.slice(win_start..win_end);
    let rel = pos.saturating_sub(win_start).min(window.len());
    let (from, to) = wordcartel_core::textobj::sentence_bounds(&window, rel);
    DismissKey {
        sentence: window.get(from..to).unwrap_or("").to_string(),
        line: crate::lines::line_text(buf, line),
    }
}

/// Whether `d` was dismissed for this session (E11 §5.3): the same engine, the same code
/// (`None` keys as the empty string), and BOTH pair units byte-equal at the diagnostic's start.
/// Equality, never containment — a dismissed sentence must not silence a longer sentence that
/// merely contains it.
///
/// `buf` is the buffer the ranges were computed against, which is NOT necessarily the active one
/// (a publish can land on a background buffer); the derivation is parse-free precisely so that is
/// safe. Cost is `O(enclosing paragraph)` per diagnostic, and the caller only reaches it for a
/// diagnostic whose `(source, code)` already matched a dismissal — see `retain_over_union`.
fn is_dismissed(d: &Diagnostic, buf: &wordcartel_core::buffer::TextBuffer,
    dismissals: &DismissSet) -> bool {
    let key = dismissal_units_at(buf, d.range.start);
    dismissals.contains(&(d.source, d.code.clone().unwrap_or_default(), key))
}

/// Drop every `Spelling` diagnostic whose surface word (sliced from `text`) is in `union`, and
/// every diagnostic of any kind that matches a session dismissal; retain everything else. Byte
/// ranges index into `text`/`buf`, which are the buffer content of the diagnostics' version — the
/// two are one version by contract, checked below.
fn retain_over_union(diags: &mut Vec<Diagnostic>, buf: &wordcartel_core::buffer::TextBuffer,
    text: &str, union: &std::collections::HashSet<String>, dismissals: &DismissSet) {
    // `text` IS `buf`'s content — the callers pass a `buf.to_string()` they already hold, so the
    // spelling slice and the pair-key derivation see one and the same version. A stale `text`
    // would filter spelling against one document and dismissals against another; not evaluated in
    // release (the compare is `O(document)`).
    debug_assert_eq!(text, buf.to_string(),
        "retain_over_union: `text` must be `buf`'s own content");
    // The cheap half of the dismissal rule, hoisted (spec §5.3 cost note): `(source, code)` is a
    // hash lookup, the pair key costs `O(enclosing paragraph)` to derive. Prune by the prefix
    // FIRST, so a diagnostic from an engine/rule the writer never dismissed derives no unit at
    // all — the cost tracks the MATCHING-source diagnostics, not every diagnostic.
    let prefixes: std::collections::HashSet<(DiagSource, &str)> =
        dismissals.iter().map(|(s, c, _)| (*s, c.as_str())).collect();
    diags.retain(|d| {
        if prefixes.contains(&(d.source, d.code.as_deref().unwrap_or("")))
            && is_dismissed(d, buf, dismissals) { return false; }
        if d.kind != DiagnosticKind::Spelling { return true; }
        let surface = text.get(d.range.start..d.range.end).unwrap_or("");
        !union.contains(&surface.to_lowercase())
    });
}

/// In-place refilter of the ACTIVE buffer's `DiagStore` against the ignore union — an immediate,
/// server-round-trip-free underline update after an ignore/add-dict overlay row (spec §7.3).
/// Refilters EVERY source's slot (not just Harper's) — the ignore union is client-side and
/// engine-agnostic, so a newly-ignored word must disappear from whichever engine flagged it.
pub fn retain_unignored(editor: &mut Editor) {
    let union = ignore_union_lower(editor);
    // Nothing suppressed → no work, no snapshot. BOTH suppressors have to be empty: a session
    // dismissal with an EMPTY spelling union is the ordinary non-spelling case, and a union-only
    // guard here would silently apply none of them (E11 §5.3).
    if union.is_empty() && editor.session_dismissals.is_empty() { return; }
    let dismissals = editor.session_dismissals.clone();
    let b = editor.active_mut();
    // Disjoint field borrows of the same buffer: the store is refiltered against the document
    // that produced the ranges.
    let (doc, store) = (&b.document, &mut b.diagnostics);
    let text = doc.buffer.to_string();
    for slot in store.slots_mut() {
        retain_over_union(&mut slot.diagnostics, &doc.buffer, &text, &union, &dismissals);
    }
}

/// Append `word` to the personal dictionary file (create if missing).
/// Returns `Ok(())` on success, `Err(e)` on IO failure (caller shows status).
pub fn append_word_to_dict(path: &std::path::Path, word: &str) -> std::io::Result<()> {
    // fs-chokepoint-allow: (w) the `RealFs` wrapper itself — its `*_with_fs` seam is what injected callers use
    append_word_to_dict_with_fs(&crate::fsx::RealFs, path, word)
}

/// Append `word` as a line to the personal dictionary — READ, append in memory, then
/// ATOMIC REPLACE.
///
/// This was the only durable write in the app outside `atomic_replace`: an
/// `OpenOptions::append` + `writeln!`, non-atomic, uncapped, with no symlink guard. A torn
/// append could leave a half-written line; the atomic form cannot. Behaviour preserved:
/// the parent directory is still created (see `append_word_to_dict_creates_parent_dir`).
pub(crate) fn append_word_to_dict_with_fs(fs: &dyn crate::fsx::Fs, path: &std::path::Path,
    word: &str) -> std::io::Result<()>
{
    if let Some(parent) = path.parent() {
        // fs-chokepoint-allow: (b) directory provisioning for the dictionary's parent
        std::fs::create_dir_all(parent)?;
    }
    // Symlink refusal, matching every other durable write.
    if matches!(fs.stat(path), Ok(st) if st.is_symlink) {
        return Err(std::io::Error::other("refusing to write through symlink"));
    }
    // Read what is there (missing/over-cap → start empty, the same degradation the old
    // create(true).append(true) had for a missing file).
    let mut buf = crate::file::bounded_read_opt_with_fs(fs, path, crate::limits::MAX_OPEN_BYTES)
        .unwrap_or_default();
    if !buf.is_empty() && !buf.ends_with(b"\n") { buf.push(b'\n'); }
    buf.extend_from_slice(word.as_bytes());
    buf.push(b'\n');
    crate::fsx::atomic_replace(fs, path, &buf, crate::fsx::WriteOpts {
        mode: crate::fsx::ModePolicy::PreserveExistingOr(0o600),
        dir_fsync: true,
    })
}

/// Version-gated apply: store only if `version` is still current for `buffer_id`. Routes into
/// `source`'s own slot — the store is source-partitioned, so a result from one engine never
/// clobbers another's (spec §5).
pub fn apply_diagnostics_done(
    editor: &mut Editor,
    buffer_id: BufferId,
    version: u64,
    source: DiagSource,
    diagnostics: Vec<Diagnostic>,
) {
    // Disabled-source drop (spec §6.2): a result for an engine that is no longer enabled is
    // dropped and its slot removed — never resurrected. `clear_source` on an absent slot is a
    // no-op, so no phantom empty slot is created (the durability invariant save.rs asserts).
    if !editor.diag_providers.is_enabled(source) {
        if let Some(b) = editor.by_id_mut(buffer_id) { b.diagnostics.clear_source(source); }
        return;
    }
    // Build the ignore union and snapshot the dismissals BEFORE borrowing the buffer mutably
    // (dictionary/session_ignores/session_dismissals live on `editor`, not the buffer). Both
    // empty in the common case → the filter below is skipped.
    let union = ignore_union_lower(editor);
    let dismissals = editor.session_dismissals.clone();
    if let Some(b) = editor.by_id_mut(buffer_id) {
        if b.document.version == version {
            let mut diagnostics = diagnostics;
            debug_assert!(diagnostics.iter().all(|d| d.source == source),
                "DiagnosticsDone payload sources match the message tag");
            if !union.is_empty() || !dismissals.is_empty() {
                // Apply-time ignore/dismissal filter (spec §7.3, §5.3): the text is this buffer at
                // `version`, so the stored byte ranges slice the right surface words and the pair
                // units derive from the very document the engine checked. A dismissal alone must
                // reach here — hence the `||`, not a union-only guard.
                let text = b.document.buffer.to_string();
                retain_over_union(&mut diagnostics, &b.document.buffer, &text, &union, &dismissals);
            }
            let slot = b.diagnostics.slot_mut(source);
            slot.diagnostics = diagnostics;
            slot.computed_version = version;
        }
        // Clear in_flight for this version if the latch is still armed for it (the check completed) —
        // but READ `slot(source)` first, so a stale result for a source with no live slot never
        // creates a phantom one (spec §6.2 non-creating latch-clear).
        if b.diagnostics.slot(source).map(|s| s.in_flight_version) == Some(Some(version)) {
            b.diagnostics.slot_mut(source).in_flight_version = None;
        }
    }
}

/// Advance the switchable analysis lens (spec §8.1) to the next ENABLED engine in registration
/// (cycle) order, wrapping past the end. With fewer than two enabled engines there is nowhere to
/// cycle to — an honest status no-op rather than a silent do-nothing (no silent UI, house rule).
pub fn cycle_analysis_source(editor: &mut Editor) {
    let enabled: Vec<DiagSource> = editor.diag_providers.enabled_sources().collect();
    if enabled.len() < 2 {
        editor.set_status(crate::status::StatusKind::Info, "no other analysis engine");
        return;
    }
    let cur = editor.active_analysis_source;
    let idx = enabled.iter().position(|s| *s == cur).unwrap_or(0);
    editor.set_analysis_source(enabled[(idx + 1) % enabled.len()]);
}

/// The single setter for per-engine enablement (contract law 6) — the toggle command and startup
/// config seeding both express enablement through ProviderSet state; runtime mutation routes
/// here. Disable: remove the engine's slot from EVERY buffer (underlines drop immediately; a
/// late in-flight terminal is dropped by apply's enabled guard) and relocate the lens if it
/// pointed here. Enable: arm the engine on the active buffer when Review is live, and — if the
/// lens was parked on a now-disabled engine (the re-enable-after-disable-to-zero path) — relocate
/// it to the engine just enabled, so §8.1's invariant holds on BOTH transitions, not only disable.
/// Does NOT `shutdown()` the provider — it stays warm for a quick re-enable; teardown remains the
/// loop-exit `shutdown_all`.
pub fn set_engine_enabled(editor: &mut Editor, source: DiagSource, on: bool,
    clock: &dyn wordcartel_core::history::Clock) {
    if !editor.diag_providers.set_enabled(source, on) {
        editor.set_status(crate::status::StatusKind::Info, format!("unknown analysis engine: {}", source.label()));
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
            editor.set_status(crate::status::StatusKind::Info, format!("{} enabled", source.label()));
        } else {
            editor.set_analysis_source(source); // relocates lens + sets "analysis: {label}"
        }
    } else {
        for b in editor.buffers.iter_mut() { b.diagnostics.clear_source(source); }
        if editor.active_analysis_source == source {
            let next = editor.diag_providers.enabled_sources().next();
            match next {
                Some(next) => editor.set_analysis_source(next),
                None => editor.set_status(crate::status::StatusKind::Info, format!("{} disabled — no analysis engine enabled",
                    source.label())),
            }
        } else {
            editor.set_status(crate::status::StatusKind::Info, format!("{} disabled", source.label()));
        }
    }
}

/// Build the core provider catalog (harper today), fold `linters` into per-engine enablement
/// (warning on unknown names), install into `editor.diag_providers`, and seed the default lens
/// (first enabled source in cycle order). Providers spawn nothing here — lazy, as before.
/// `linters`: `None` → every core engine enabled; `Some(list)` → exactly the named engines
/// (names = `DiagSource::config_name()`); `Some([])` → none. This is the promised validation
/// site the config fold's `linters` comment points at (SPINE Task 8, spec §9).
pub fn install_core_providers(editor: &mut Editor, cfg: &crate::config::Config,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>, warns: &mut Vec<String>) {
    // The complete core catalog in cycle order. `vale` is NOT in it: a live probe established
    // that vale-ls lints the file ON DISK and never the buffer we sync to it (`didChange`
    // produces no server messages; `didSave` re-reads disk and ignores its own `text`), so it
    // cannot back a live lens at all. The provider is gone; `DiagSource::Vale` stays reserved in
    // core for the replacement transport (the vale CLI, one-shot over the buffer text).
    let catalog: &[DiagSource] = &[DiagSource::Harper, DiagSource::LTeX];
    // Which engines are enabled: None → the whole core catalog; Some(list) → exactly the named
    // (config_name).
    let enabled_of = |src: DiagSource| -> bool {
        match &cfg.diagnostics.linters {
            None => true,
            Some(list) => list.iter().any(|n| n == src.config_name()),
        }
    };
    if let Some(list) = &cfg.diagnostics.linters {
        for name in list {
            if catalog.iter().any(|s| s.config_name() == name) { continue; }
            // `vale` stays a RECOGNISED name — the engine is known, the transport is gone. Saying
            // "unknown engine" would be false, and this warns rather than ignoring silently.
            if name == DiagSource::Vale.config_name() {
                warns.push("config: diagnostics.linters — \"vale\" is not available in this build \
                    (vale-ls cannot lint unsaved buffers); ignoring it".to_string());
            } else {
                warns.push(format!(
                    "config: diagnostics.linters — unknown engine \"{name}\" (known: {})",
                    catalog.iter().map(|s| s.config_name()).collect::<Vec<_>>().join(", ")));
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
                    language: None,
                })),
            DiagSource::LTeX => Box::new(crate::lsp_client::LspProvider::<crate::ltex_ls::LtexEngine>::new(
                msg_tx.clone(),
                crate::diag_provider::ProviderConfig {
                    grammar: cfg.diagnostics.grammar,
                    dictionary: None, // per-engine dictionaries are E11's (spec §14.2)
                    max_file_length: crate::limits::HARPER_MAX_FILE_LENGTH, // inert for ltex (spec §9)
                    language: Some(cfg.diagnostics.ltex_language.clone()),
                })),
            // Exhaustive — every core engine has an arm. `Vale` has no provider in this build and
            // `Plugin` engines are not in this catalog, so neither can appear here.
            DiagSource::Vale | DiagSource::Plugin(_) => continue,
        };
        editor.diag_providers.install(provider, enabled_of(src));
    }
    // Seed the lens to the first enabled source (Harper fallback when none enabled — inert).
    // Writes the field directly (not `set_analysis_source`, which would status-message and
    // refuse a not-yet-populated set) — this is construction, matching the clipboard-provider
    // seeding precedent.
    if let Some(first) = editor.diag_providers.enabled_sources().next() {
        editor.active_analysis_source = first;
    }
    // E10 §13: the config-only default-engine override — applied ONLY when the named engine
    // is enabled; known-but-disabled falls back loudly. (Unknown NAMES were already rejected
    // at the config fold.) Direct field write, matching the seed above — construction, not
    // set_analysis_source (which would status-message).
    if let Some(want) = cfg.diagnostics.default_engine {
        if editor.diag_providers.is_enabled(want) {
            editor.active_analysis_source = want;
        } else {
            warns.push(format!(
                "config: diagnostics.default_engine — \"{}\" is not enabled; using {}",
                want.config_name(), editor.active_analysis_source.label()));
        }
    }
}

/// The engine-management dynamic menu rows (E10 §11): one row per registered engine,
/// state-in-label ("on" / "off" / "warming…" / "not installed"), dispatching that engine's
/// toggle command — menu ⊆ palette by construction. `Plugin` sources are skipped (their
/// rows are E12's plugin-contributed-menu effort), as is `Vale`, whose provider and commands
/// were removed. Availability is lazily discovered: an absent binary reads "on" until Review
/// first attempts a spawn (spec §11 display note).
pub fn engine_menu_rows(editor: &Editor) -> Vec<(String, crate::menu::MenuRowAction)> {
    use crate::diag_provider::Availability;
    editor.diag_providers.sources().filter_map(|src| {
        let cmd = match src {
            DiagSource::Harper => "toggle_engine_harper",
            DiagSource::LTeX => "toggle_engine_ltex",
            // No provider, hence no `toggle_engine_vale` command to dispatch — a row here would
            // exist only to refuse (contract law 4: every menu row names a real command).
            DiagSource::Vale => return None,
            DiagSource::Plugin(_) => return None,
        };
        let state = if !editor.diag_providers.is_enabled(src) { "off" }
            else {
                match editor.diag_providers.availability(src) {
                    Some(Availability::Unavailable) => "not installed",
                    Some(Availability::Starting) => "warming…",
                    _ => "on", // Idle | Ready | None-entry (unreachable for a listed source)
                }
            };
        Some((format!("{} — {}", src.label(), state),
            crate::menu::MenuRowAction::Command(crate::registry::CommandId(cmd))))
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_deadline_is_min_ignoring_none() {
        assert_eq!(next_deadline(&[None, Some(50), None, Some(20), Some(99)]), Some(20));
        assert_eq!(next_deadline(&[None, None]), None);
    }

    #[test]
    fn arm_sets_due_and_valid_for_tracks_version() {
        let mut s = SourceSlot::default();
        assert!(!s.valid_for(0)); // empty slot: no diagnostics yet, regardless of version
        s.arm(1000, 400);
        assert_eq!(s.recheck_due_at, Some(1400));
    }

    #[test]
    fn any_due_requires_armed_reached_and_not_in_flight() {
        let mut s = DiagStore::new();
        s.slot_mut(DiagSource::Harper).arm(1000, 400);
        assert!(!s.any_due(1399), "not yet due");
        assert!(s.any_due(1400), "due at deadline");
        // same version in flight → blocks
        s.slot_mut(DiagSource::Harper).in_flight_version = Some(7);
        assert!(!s.any_due(1500), "already in flight for this version");
        // different version in flight → ALSO blocks (single-in-flight invariant)
        s.slot_mut(DiagSource::Harper).in_flight_version = Some(8);
        assert!(!s.any_due(1500), "in flight for a different version also blocks dispatch");
    }

    #[test]
    fn valid_for_only_when_computed_version_matches() {
        let mut s = SourceSlot { computed_version: 5, ..Default::default() };
        s.diagnostics.push(wordcartel_core::diagnostics::Diagnostic {
            range: 0..1, kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling,
            source: DiagSource::Harper, code: None, href: None,
            message: "x".into(), suggestions: vec![] });
        assert!(s.valid_for(5));
        assert!(!s.valid_for(6)); // edited since → hidden
    }

    // ------------------------------------------------------------------
    // Task 3: source-partitioned DiagStore/SourceSlot
    // ------------------------------------------------------------------

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

    #[test]
    fn should_run_diagnostics_only_in_review_and_enabled() {
        use crate::editor::{Editor, RenderMode};
        let mut e = Editor::new_from_text("x\n", None, (40, 10));
        e.diag_cfg.enabled = true;
        for (mode, want) in [(RenderMode::LivePreview, false), (RenderMode::Review, true),
                             (RenderMode::SourceHighlighted, false), (RenderMode::SourcePlain, false)] {
            e.active_mut().view.mode = mode;
            assert_eq!(should_run_diagnostics(&e), want, "{mode:?} enabled");
            assert_eq!(should_show_diagnostics(&e), want, "show mirrors run: {mode:?}");
        }
        e.active_mut().view.mode = RenderMode::Review;
        e.diag_cfg.enabled = false;
        assert!(!should_run_diagnostics(&e), "disabled → false even in Review");
    }

    #[test]
    fn arm_if_edited_arms_only_on_active_buffer_edit_in_review() {
        use crate::editor::{Editor, RenderMode};
        let mut e = Editor::new_from_text("x\n", None, (40, 10));
        crate::test_support::install_enabled_harper(&mut e); // arm_if_edited → arm_enabled arms enabled sources
        e.diag_cfg.enabled = true;
        e.active_mut().view.mode = RenderMode::Review;
        use crate::test_support::TestClock;
        let id = e.active().id;
        let v = e.active().document.version;
        // no version change → no arm
        arm_if_edited(&mut e, id, v, &TestClock(100));
        assert!(e.active().diagnostics.slot(DiagSource::Harper).is_none(), "equal version: no arm");
        // version increased, same buffer, Review, enabled → arm at now+debounce
        e.active_mut().document.version += 1;
        arm_if_edited(&mut e, id, v, &TestClock(100));
        assert_eq!(e.active().diagnostics.slot(DiagSource::Harper).unwrap().recheck_due_at,
            Some(100 + e.diag_cfg.debounce_ms));
        // same edit but in LivePreview → no arm
        e.active_mut().diagnostics.slot_mut(DiagSource::Harper).recheck_due_at = None;
        e.active_mut().view.mode = RenderMode::LivePreview;
        arm_if_edited(&mut e, id, v, &TestClock(200));
        assert_eq!(e.active().diagnostics.slot(DiagSource::Harper).unwrap().recheck_due_at, None,
            "not Review: no arm");
        // buffer-identity guard: active id != before_id → no arm even with a version delta
        e.active_mut().view.mode = RenderMode::Review;
        let other = crate::editor::BufferId(id.0.wrapping_add(999));
        arm_if_edited(&mut e, other, v, &TestClock(300));
        assert_eq!(e.active().diagnostics.slot(DiagSource::Harper).unwrap().recheck_due_at, None,
            "switch (id changed): no arm");
    }

    #[test]
    fn append_word_to_dict_creates_parent_dir() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let dict_path = temp_dir.path()
            .join("subdir")
            .join("nested")
            .join("dictionary.txt");

        // Should succeed even though parent dirs don't exist
        append_word_to_dict(&dict_path, "testword").expect("append should succeed");

        assert!(dict_path.exists(), "dictionary file should exist");
        let content = std::fs::read_to_string(&dict_path).expect("should read file");
        assert!(content.contains("testword"), "file should contain the appended word");
    }

    #[test]
    fn append_word_to_dict_is_atomic_and_preserves_existing_words() {
        // The append becomes read -> append in memory -> atomic_replace, so a torn write
        // is impossible. Existing content must survive verbatim.
        let d = std::env::temp_dir().join(format!("wc-dict-atomic-{}", std::process::id()));
        let p = d.join("dictionary.txt");
        let _ = std::fs::remove_dir_all(&d);
        append_word_to_dict_with_fs(&crate::fsx::RealFs, &p, "alpha").expect("first append");
        append_word_to_dict_with_fs(&crate::fsx::RealFs, &p, "beta").expect("second append");
        let got = std::fs::read_to_string(&p).expect("read back");
        assert_eq!(got, "alpha\nbeta\n", "both words present, newline-terminated, in order");

        // ATOMICITY, actually observed. The assertion above passes identically under the OLD
        // non-atomic `OpenOptions::append` + `writeln!` — appending twice produces the same
        // bytes either way. What separates them is a FAILED write: the atomic form leaves the
        // previous contents intact, the append form leaves a torn file.
        //
        // FAIL-VERIFY: restore the append implementation, watch this fail with "alpha\nbeta\ngam".
        let ff = crate::test_support::FaultFs::new(
            crate::test_support::FaultAt::Write { after: 3 });
        let err = append_word_to_dict_with_fs(&ff, &p, "gamma")
            .expect_err("an injected mid-write failure must surface");
        let _ = err;
        assert_eq!(std::fs::read_to_string(&p).expect("read back"), "alpha\nbeta\n",
            "a FAILED append leaves the dictionary exactly as it was — no torn line. This is \
             the property `atomic_replace` buys and the old append could not.");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn append_word_to_dict_refuses_a_symlinked_dictionary() {
        // The append gains the symlink guard every other durable write has. Writing through
        // the link would replace it with a regular file and destroy the link.
        let d = std::env::temp_dir().join(format!("wc-dict-link-{}", std::process::id()));
        std::fs::create_dir_all(&d).expect("dir");
        let real = d.join("real.txt");
        let link = d.join("dict.txt");
        std::fs::write(&real, "existing\n").expect("seed");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");
        let err = append_word_to_dict_with_fs(&crate::fsx::RealFs, &link, "nope")
            .expect_err("symlinked dictionary must be refused");
        assert!(err.to_string().to_lowercase().contains("symlink"), "got {err}");
        assert!(link.symlink_metadata().expect("lstat").file_type().is_symlink(),
            "the link must survive — that is what the refusal protects");
        assert_eq!(std::fs::read_to_string(&real).expect("read"), "existing\n",
            "target untouched");
        let _ = std::fs::remove_dir_all(&d);
    }

    // ------------------------------------------------------------------
    // Effort A: dispatch_diagnostics over the DiagnosticsProvider seam.
    // ------------------------------------------------------------------
    use crate::editor::{Editor, RenderMode};
    use crate::diag_provider::{RecordingProvider, Availability, Accepted};
    use wordcartel_core::diagnostics::DiagSource;

    fn review_editor(text: &str) -> Editor {
        let mut e = Editor::new_from_text(text, None, (80, 24));
        e.diag_cfg.enabled = true;
        e.active_mut().view.mode = RenderMode::Review;
        e
    }

    #[test]
    fn dispatch_latches_in_flight_only_on_accepted_yes() {
        let mut e = review_editor("teh\n");
        let rec = RecordingProvider::new().with_source(DiagSource::Harper); // Ready, accepts
        let calls = rec.calls_handle();
        e.diag_providers.install(Box::new(rec), true);
        let v = e.active().document.version;
        e.active_mut().diagnostics.slot_mut(DiagSource::Harper).arm(0, 0);
        dispatch_diagnostics(&mut e, 10);
        assert_eq!(e.active().diagnostics.slot(DiagSource::Harper).unwrap().in_flight_version, Some(v),
            "accepted → latch set");
        assert_eq!(e.active().diagnostics.slot(DiagSource::Harper).unwrap().recheck_due_at, None,
            "armed deadline consumed");
        let log = calls.lock().unwrap();
        assert!(log.iter().any(|c| matches!(c, crate::diag_provider::ProviderCall::EnsureRunning)));
        assert!(log.iter().any(|c| matches!(c,
            crate::diag_provider::ProviderCall::NotifyChange { version, .. } if *version == v)));
    }

    #[test]
    fn dispatch_no_latch_and_hint_on_accepted_no() {
        let mut e = review_editor("teh\n");
        e.diag_providers.install(Box::new(RecordingProvider::new().with_source(DiagSource::Harper)
            .with_accepted(Accepted::No).with_availability(Availability::Ready)), true);
        e.active_mut().diagnostics.slot_mut(DiagSource::Harper).arm(0, 0);
        dispatch_diagnostics(&mut e, 10);
        assert_eq!(e.active().diagnostics.slot(DiagSource::Harper).unwrap().in_flight_version, None,
            "Accepted::No must not latch");
        assert!(e.diag_hint_shown.contains(&DiagSource::Harper), "the degrade hint latch is set");
        assert_eq!(e.status_text(), "test provider unavailable", "the installed provider's own hint");
        // A17 T5 (F4 Warning table): a Sticky Warning.
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Warning);
        assert_eq!(e.status().unwrap().lifetime(), crate::status::StatusLifetime::Sticky);
    }

    #[test]
    fn dispatch_over_cap_sets_status_and_never_touches_provider() {
        let big = "x".repeat((crate::limits::DIAG_MAX_SEND_BYTES as usize) + 1);
        let mut e = review_editor(&big);
        let rec = RecordingProvider::new().with_source(DiagSource::Harper);
        let calls = rec.calls_handle();
        e.diag_providers.install(Box::new(rec), true);
        e.active_mut().diagnostics.slot_mut(DiagSource::Harper).arm(0, 0);
        dispatch_diagnostics(&mut e, 10);
        assert_eq!(e.status_text(), "document too large for grammar checking");
        // A17 T5 (F4 Warning table): a Sticky Warning.
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Warning);
        assert_eq!(e.status().unwrap().lifetime(), crate::status::StatusLifetime::Sticky);
        assert_eq!(e.active().diagnostics.slot(DiagSource::Harper).unwrap().in_flight_version, None,
            "over-cap: no latch");
        assert!(calls.lock().unwrap().is_empty(), "over-cap short-circuits before the provider");
    }

    #[test]
    fn dispatch_unavailable_shows_hint_once() {
        let mut e = review_editor("teh\n");
        e.diag_providers.install(Box::new(RecordingProvider::new().with_source(DiagSource::Harper)
            .with_availability(Availability::Unavailable)), true);
        e.active_mut().diagnostics.slot_mut(DiagSource::Harper).arm(0, 0);
        dispatch_diagnostics(&mut e, 10);
        assert_eq!(e.status_text(), "test provider unavailable");
        // A17 T5 (F4 Warning table): a Sticky Warning.
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Warning);
        assert_eq!(e.status().unwrap().lifetime(), crate::status::StatusLifetime::Sticky);
        assert!(e.diag_hint_shown.contains(&DiagSource::Harper));
        // Second dispatch: hint already shown → status is not re-set (informative, not naggy).
        e.clear_status();
        e.active_mut().diagnostics.slot_mut(DiagSource::Harper).arm(0, 0);
        dispatch_diagnostics(&mut e, 20);
        assert_eq!(e.status_text(), "", "hint shows at most once per Review entry");
        assert_eq!(e.active().diagnostics.slot(DiagSource::Harper).unwrap().in_flight_version, None);
    }

    #[test]
    fn dispatch_starting_shows_no_silent_wait_status_and_latches() {
        let mut e = review_editor("teh\n");
        e.diag_providers.install(Box::new(RecordingProvider::new().with_source(DiagSource::Harper)
            .with_availability(Availability::Starting)), true); // still accepts (queued post-handshake)
        let v = e.active().document.version;
        e.active_mut().diagnostics.slot_mut(DiagSource::Harper).arm(0, 0);
        dispatch_diagnostics(&mut e, 10);
        assert_eq!(e.status_text(), "starting Harper…");
        assert_eq!(e.active().diagnostics.slot(DiagSource::Harper).unwrap().in_flight_version, Some(v),
            "Starting still accepts + latches");
    }

    // ------------------------------------------------------------------
    // Task 5: dispatch fan-out over enabled+due sources (dispatch_one).
    // ------------------------------------------------------------------

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

    /// §14.1 case 3 extension (per-source in-flight): a source already mid-check (latched) is
    /// excluded from `due_sources` and never gets a second `dispatch_one` call — but the OTHER
    /// source, which is due and NOT in flight, still dispatches normally. Proves the in-flight
    /// guard blocks only the latched source, not the whole fan-out (non-vacuous: harper's call
    /// log stays empty while mock's records exactly one `NotifyChange`).
    #[test]
    fn dispatch_in_flight_source_is_skipped_other_due_source_still_dispatches() {
        let mut e = review_editor("teh cat\n");
        let h = crate::diag_provider::RecordingProvider::new().with_source(DiagSource::Harper);
        let m = crate::diag_provider::RecordingProvider::new().with_source(DiagSource::Plugin("mock"));
        let (hc, mc) = (h.calls_handle(), m.calls_handle());
        e.diag_providers.install(Box::new(h), true);
        e.diag_providers.install(Box::new(m), true);
        // Harper is mid-check for an earlier version — armed AND in-flight simultaneously (the
        // state `due_sources` must exclude regardless of the armed deadline being reached).
        e.active_mut().diagnostics.slot_mut(DiagSource::Harper).arm(0, 0);
        e.active_mut().diagnostics.slot_mut(DiagSource::Harper).in_flight_version = Some(0);
        e.active_mut().diagnostics.slot_mut(DiagSource::Plugin("mock")).arm(0, 0);
        dispatch_diagnostics(&mut e, 10);
        assert_eq!(e.active().diagnostics.slot(DiagSource::Harper).unwrap().in_flight_version, Some(0),
            "harper's in-flight latch is untouched — no second dispatch while one is outstanding");
        assert!(e.active().diagnostics.slot(DiagSource::Plugin("mock")).unwrap().in_flight_version.is_some(),
            "mock, not in-flight, still dispatches normally");
        assert!(hc.lock().unwrap().is_empty(), "the in-flight source's provider is never called again");
        assert_eq!(mc.lock().unwrap().iter()
            .filter(|c| matches!(c, crate::diag_provider::ProviderCall::NotifyChange { .. })).count(), 1,
            "the due, non-in-flight source dispatches exactly once");
    }

    #[test]
    fn dispatch_over_cap_consumes_deadlines_and_never_latches() {
        let big = "x".repeat((crate::limits::DIAG_MAX_SEND_BYTES as usize) + 1);
        let mut e = review_editor(&big);
        e.diag_providers.install(Box::new(
            crate::diag_provider::RecordingProvider::new().with_source(DiagSource::Harper)), true);
        e.active_mut().diagnostics.slot_mut(DiagSource::Harper).arm(0, 0);
        dispatch_diagnostics(&mut e, 10);
        assert_eq!(e.status_text(), "document too large for grammar checking");
        assert_eq!(e.active().diagnostics.slot(DiagSource::Harper).unwrap().in_flight_version, None);
        assert_eq!(e.active().diagnostics.slot(DiagSource::Harper).unwrap().recheck_due_at, None);
    }

    fn spelling(range: std::ops::Range<usize>) -> Diagnostic {
        Diagnostic { range, kind: DiagnosticKind::Spelling, source: DiagSource::Harper, code: None,
            href: None, message: "x".into(), suggestions: vec![] }
    }
    fn grammar(range: std::ops::Range<usize>) -> Diagnostic {
        Diagnostic { range, kind: DiagnosticKind::Grammar, source: DiagSource::Harper, code: None,
            href: None, message: "x".into(), suggestions: vec![] }
    }

    #[test]
    fn apply_filters_ignored_spelling_over_the_union_keeps_grammar() {
        let mut e = Editor::new_from_text("teh cat\n", None, (80, 24));
        crate::test_support::install_enabled_harper(&mut e); // apply's is_enabled(Harper) guard
        let id = e.active().id;
        let v = e.active().document.version;
        e.dictionary.insert("TEH".into()); // case-insensitive union membership
        // "teh" (0..3) is a spelling hit → dropped; the grammar diagnostic on "cat" (4..7) stays.
        apply_diagnostics_done(&mut e, id, v, DiagSource::Harper, vec![spelling(0..3), grammar(4..7)]);
        let kept = &e.active().diagnostics.slot(DiagSource::Harper).unwrap().diagnostics;
        assert_eq!(kept.len(), 1, "spelling 'teh' filtered by dictionary; grammar retained");
        assert_eq!(kept[0].kind, DiagnosticKind::Grammar);
    }

    // ------------------------------------------------------------------
    // Task 4: source-routed apply (is_enabled guard + non-creating latch-clear).
    // ------------------------------------------------------------------

    #[test]
    fn apply_routes_to_the_named_source_slot_only() {
        let mut e = review_editor("teh cat\n");
        crate::test_support::install_enabled_harper(&mut e);
        e.diag_providers.install(Box::new(
            RecordingProvider::new().with_source(DiagSource::Plugin("mock"))), true);
        let id = e.active().id;
        let v = e.active().document.version;
        let mock_grammar = Diagnostic { range: 4..7, kind: DiagnosticKind::Grammar,
            source: DiagSource::Plugin("mock"), code: None, href: None,
            message: "x".into(), suggestions: vec![] };
        apply_diagnostics_done(&mut e, id, v, DiagSource::Harper, vec![spelling(0..3)]);
        apply_diagnostics_done(&mut e, id, v, DiagSource::Plugin("mock"), vec![mock_grammar]);
        assert_eq!(e.active().diagnostics.slot(DiagSource::Harper).unwrap().diagnostics.len(), 1);
        assert_eq!(e.active().diagnostics.slot(DiagSource::Plugin("mock")).unwrap().diagnostics.len(), 1);
        assert_eq!(e.active().diagnostics.slot(DiagSource::Harper).unwrap().diagnostics[0].kind,
            DiagnosticKind::Spelling, "each result lands in its own source's slot, no cross-clobber");
    }

    #[test]
    fn apply_stale_version_clears_latch_without_storing() {
        let mut e = review_editor("teh\n");
        crate::test_support::install_enabled_harper(&mut e);
        let id = e.active().id;
        let v = e.active().document.version;
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
        // mock is NOT installed/enabled → result dropped, slot removed.
        e.active_mut().diagnostics.slot_mut(DiagSource::Plugin("mock")).in_flight_version = Some(1);
        let id = e.active().id;
        let v = e.active().document.version;
        let mock = Diagnostic { range: 0..3, kind: DiagnosticKind::Spelling,
            source: DiagSource::Plugin("mock"), code: None, href: None,
            message: "x".into(), suggestions: vec![] };
        apply_diagnostics_done(&mut e, id, v, DiagSource::Plugin("mock"), vec![mock]);
        assert!(e.active().diagnostics.slot(DiagSource::Plugin("mock")).is_none(),
            "disabled source: result dropped and phantom slot removed");
    }

    // ------------------------------------------------------------------
    // Task 9 (SPINE): two-provider acceptance core — §14.1 case 2 (staleness
    // independence), case 6 addition (late-Done-does-not-resurrect), case 8
    // (per-source hint latch).
    // ------------------------------------------------------------------

    /// §14.1 item 2 EXACT scenario, the acceptance bar: two engines both dispatched at the same
    /// version `v`; the document then advances to `v+1`. The SLOW engine's terminal result for
    /// the now-stale `v` must be dropped WITHOUT storing, and its OWN latch clears (so it is
    /// free to be re-dispatched) — while the FAST engine's terminal result for the CURRENT
    /// version `v+1` stores normally in its OWN slot. Non-vacuous: the two engines' diagnostics
    /// end up in DIFFERENT kinds/slots (grammar dropped vs spelling stored) and the assertions
    /// on `ms` (the mock/slow slot) and `hs` (the harper/fast slot) are independently checked —
    /// a version-gate bug that let the stale result leak through, or a latch bug that failed to
    /// clear the slow engine's OR wrongly cleared the fast engine's, would trip a distinct
    /// assertion here.
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
        // SLOW engine (mock) terminal for v arrives → NOT stored, latch clears. The payload
        // carries its OWN source (Plugin("mock"), not the shared `grammar()` helper's Harper
        // default — see `apply_routes_to_the_named_source_slot_only`'s `mock_grammar`
        // precedent) so the debug_assert in `apply_diagnostics_done` (payload source must match
        // the message tag) genuinely holds rather than merely never firing because this
        // particular call takes the stale/drop branch.
        let mock_grammar = Diagnostic { range: 4..7, kind: DiagnosticKind::Grammar,
            source: DiagSource::Plugin("mock"), code: None, href: None,
            message: "x".into(), suggestions: vec![] };
        apply_diagnostics_done(&mut e, id, v, DiagSource::Plugin("mock"), vec![mock_grammar]);
        {
            let ms = e.active().diagnostics.slot(DiagSource::Plugin("mock")).unwrap();
            assert!(ms.diagnostics.is_empty(), "stale v result not stored");
            assert_eq!(ms.in_flight_version, None, "slow latch cleared");
        }
        let ms_computed_version = e.active().diagnostics.slot(DiagSource::Plugin("mock")).unwrap().computed_version;
        // The harper (fast) in-flight latch is re-armed to the CURRENT version — the real
        // dispatch flow the moment its own stale-v cycle clears and it is re-checked (a source
        // with no live match for the version it is asked to apply is never latch-cleared; see
        // `apply_stale_version_clears_latch_without_storing`), so the terminal result below
        // arrives for the SAME version it was dispatched against.
        e.active_mut().diagnostics.slot_mut(DiagSource::Harper).in_flight_version = Some(v + 1);
        // FAST engine (harper) terminal for v+1 arrives → stored
        apply_diagnostics_done(&mut e, id, v + 1, DiagSource::Harper, vec![spelling(0..3)]);
        let hs = e.active().diagnostics.slot(DiagSource::Harper).unwrap();
        assert_eq!(hs.diagnostics.len(), 1);
        assert_eq!(hs.computed_version, v + 1);
        assert_eq!(hs.in_flight_version, None);
        assert_ne!(hs.computed_version, ms_computed_version, "no cross-contamination");
    }

    /// §14.1 case 6 addition: a `DiagnosticsDone` for a source that was disabled AFTER dispatch
    /// (but before its terminal result lands) must not resurrect the slot `set_engine_enabled`
    /// already cleared — the disabled-source drop path (spec §6.2) applies regardless of WHEN
    /// the disable happened relative to the in-flight dispatch.
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

    /// §14.1 case 8: each engine's degrade hint is an INDEPENDENT per-source latch — two
    /// simultaneously Unavailable engines each surface their hint exactly once per Review entry,
    /// neither suppressing the other. Non-vacuous: both sources' presence in `diag_hint_shown`
    /// is asserted individually, and the set is proven to actually reset (not just "happen to be
    /// empty") by re-entering Review via the real `set_render_mode` seam.
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

    // ------------------------------------------------------------------
    // Task 6 (SPINE): the switchable analysis lens — active_lens_diags.
    // ------------------------------------------------------------------

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

    #[test]
    fn active_lens_diags_none_outside_review_and_when_slot_stale() {
        let mut e = review_editor("teh\n");
        e.diag_providers.install(Box::new(
            crate::diag_provider::RecordingProvider::new().with_source(DiagSource::Harper)), true);
        let v = e.active().document.version;
        let hs = e.active_mut().diagnostics.slot_mut(DiagSource::Harper);
        hs.diagnostics = vec![spelling(0..3)]; hs.computed_version = v;
        assert!(active_lens_diags(&e).is_some(), "Review + valid slot: Some");
        e.active_mut().view.mode = RenderMode::LivePreview;
        assert!(active_lens_diags(&e).is_none(), "outside Review: None regardless of slot validity");
        e.active_mut().view.mode = RenderMode::Review;
        e.active_mut().document.version += 1; // edited since compute → stale
        assert!(active_lens_diags(&e).is_none(), "stale slot: None");
    }

    /// §14.1 case 7 gap: `cycle_analysis_source` is a status no-op with fewer than two enabled
    /// engines (`cycle_with_fewer_than_two_enabled_is_a_status_no_op`) — but the LENS itself
    /// must stay fully functional in that same single-engine state; "nowhere to cycle to" is not
    /// "nothing to show". Proves the two are orthogonal: cycling being a no-op does not make
    /// `active_lens_diags` a no-op too. Non-vacuous: asserts the no-op status AND the still-live
    /// diagnostics slice together — a hypothetical regression that gated `active_lens_diags` on
    /// `enabled_sources().len() >= 2` would trip the second assertion even though the first
    /// (cycle no-op) still passes.
    #[test]
    fn active_lens_diags_functions_normally_when_cycle_is_a_no_op() {
        let mut e = review_editor("teh\n");
        e.diag_providers.install(Box::new(
            crate::diag_provider::RecordingProvider::new().with_source(DiagSource::Harper)), true);
        let v = e.active().document.version;
        let hs = e.active_mut().diagnostics.slot_mut(DiagSource::Harper);
        hs.diagnostics = vec![spelling(0..3)]; hs.computed_version = v;
        cycle_analysis_source(&mut e); // < 2 enabled engines → status no-op, lens UNCHANGED
        assert_eq!(e.status_text(), "no other analysis engine");
        assert_eq!(e.active_analysis_source, DiagSource::Harper, "lens stayed put — nowhere to cycle");
        let diags = active_lens_diags(&e)
            .expect("the lens must still resolve the single enabled engine's diagnostics");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].kind, DiagnosticKind::Spelling);
    }

    // ------------------------------------------------------------------
    // Task 7: lens/enable commands — cycle_analysis_source, set_engine_enabled.
    // ------------------------------------------------------------------

    #[test]
    fn cycle_wraps_enabled_sources_only() {
        let mut e = Editor::new_from_text("x\n", None, (40, 10));
        e.diag_providers.install(Box::new(
            crate::diag_provider::RecordingProvider::new().with_source(DiagSource::Harper)), true);
        e.diag_providers.install(Box::new(
            crate::diag_provider::RecordingProvider::new().with_source(DiagSource::Plugin("mock"))), true);
        assert_eq!(e.active_analysis_source, DiagSource::Harper);
        cycle_analysis_source(&mut e);
        assert_eq!(e.active_analysis_source, DiagSource::Plugin("mock"));
        cycle_analysis_source(&mut e);
        assert_eq!(e.active_analysis_source, DiagSource::Harper);
    }

    #[test]
    fn cycle_with_fewer_than_two_enabled_is_a_status_no_op() {
        let mut e = Editor::new_from_text("x\n", None, (40, 10));
        e.diag_providers.install(Box::new(
            crate::diag_provider::RecordingProvider::new().with_source(DiagSource::Harper)), true);
        cycle_analysis_source(&mut e);
        assert_eq!(e.active_analysis_source, DiagSource::Harper, "nowhere to cycle to");
        assert_eq!(e.status_text(), "no other analysis engine");
    }

    #[test]
    fn disable_clears_slots_and_relocates_lens() {
        use crate::test_support::TestClock;
        let mut e = review_editor("teh\n");
        e.diag_providers.install(Box::new(
            crate::diag_provider::RecordingProvider::new().with_source(DiagSource::Harper)), true);
        e.diag_providers.install(Box::new(
            crate::diag_provider::RecordingProvider::new().with_source(DiagSource::Plugin("mock"))), true);
        e.set_analysis_source(DiagSource::Plugin("mock"));
        e.active_mut().diagnostics.slot_mut(DiagSource::Plugin("mock")).diagnostics = vec![spelling(0..3)];
        set_engine_enabled(&mut e, DiagSource::Plugin("mock"), false, &TestClock::new(0));
        assert!(e.active().diagnostics.slot(DiagSource::Plugin("mock")).is_none(), "slot cleared");
        assert_eq!(e.active_analysis_source, DiagSource::Harper, "lens relocated off disabled engine");
        assert!(!e.diag_providers.is_enabled(DiagSource::Plugin("mock")));
    }

    #[test]
    fn disable_last_enabled_engine_leaves_an_honest_status_and_no_lens_source() {
        use crate::test_support::TestClock;
        let mut e = review_editor("teh\n");
        e.diag_providers.install(Box::new(
            crate::diag_provider::RecordingProvider::new().with_source(DiagSource::Harper)), true);
        set_engine_enabled(&mut e, DiagSource::Harper, false, &TestClock::new(0));
        assert_eq!(e.status_text(), "Harper disabled — no analysis engine enabled");
        assert!(!e.diag_providers.is_enabled(DiagSource::Harper));
    }

    #[test]
    fn enable_arms_the_active_buffer_when_review_is_live() {
        use crate::test_support::TestClock;
        let mut e = review_editor("teh\n");
        e.diag_providers.install(Box::new(
            crate::diag_provider::RecordingProvider::new().with_source(DiagSource::Harper)), false);
        assert!(!e.diag_providers.is_enabled(DiagSource::Harper));
        set_engine_enabled(&mut e, DiagSource::Harper, true, &TestClock::new(500));
        assert!(e.diag_providers.is_enabled(DiagSource::Harper));
        assert_eq!(e.active().diagnostics.slot(DiagSource::Harper).unwrap().recheck_due_at, Some(500),
            "enable arms the active buffer's slot when Review checking is live");
        assert_eq!(e.status_text(), "Harper enabled");
    }

    #[test]
    fn re_enable_after_zero_relocates_lens_onto_enabled_engine() {
        use crate::test_support::TestClock;
        let mut e = review_editor("teh\n");
        e.diag_providers.install(Box::new(
            crate::diag_provider::RecordingProvider::new().with_source(DiagSource::Harper)), true);
        e.diag_providers.install(Box::new(
            crate::diag_provider::RecordingProvider::new().with_source(DiagSource::Plugin("mock"))), true);
        e.set_analysis_source(DiagSource::Harper);
        // Disable Harper: lens relocates to the remaining enabled engine (mock).
        set_engine_enabled(&mut e, DiagSource::Harper, false, &TestClock::new(0));
        assert_eq!(e.active_analysis_source, DiagSource::Plugin("mock"));
        // Disable mock: none enabled, lens is left where it was (no enabled source to relocate to).
        set_engine_enabled(&mut e, DiagSource::Plugin("mock"), false, &TestClock::new(0));
        assert_eq!(e.active_analysis_source, DiagSource::Plugin("mock"));
        assert!(!e.diag_providers.is_enabled(DiagSource::Plugin("mock")));
        // Re-enable Harper: the lens still names the now-disabled mock — §8.1 requires it relocate
        // onto the engine just enabled so results are visible and reachable.
        set_engine_enabled(&mut e, DiagSource::Harper, true, &TestClock::new(0));
        assert_eq!(e.active_analysis_source, DiagSource::Harper,
            "lens relocated onto the re-enabled engine, not left on the disabled one");
        assert!(e.diag_providers.is_enabled(e.active_analysis_source), "lens names an enabled engine");
    }

    #[test]
    fn set_enabled_on_unknown_source_is_a_status_no_op() {
        use crate::test_support::TestClock;
        let mut e = Editor::new_from_text("x\n", None, (40, 10));
        set_engine_enabled(&mut e, DiagSource::Plugin("ghost"), true, &TestClock::new(0));
        assert_eq!(e.status_text(), "unknown analysis engine: ghost");
    }

    #[test]
    fn retain_unignored_refilters_the_active_store_in_place() {
        let mut e = Editor::new_from_text("teh cat\n", None, (80, 24));
        e.active_mut().diagnostics.slot_mut(DiagSource::Harper).diagnostics = vec![spelling(0..3), grammar(4..7)];
        e.session_ignores.insert("teh".into());
        retain_unignored(&mut e);
        let kept = &e.active().diagnostics.slot(DiagSource::Harper).unwrap().diagnostics;
        assert_eq!(kept.len(), 1, "the newly-ignored spelling word is dropped in place");
        assert_eq!(kept[0].kind, DiagnosticKind::Grammar);
    }

    // ------------------------------------------------------------------
    // Task 8 (SPINE): install_core_providers — config-driven enablement.
    // ------------------------------------------------------------------

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

    #[test]
    fn install_core_providers_empty_linters_enables_none() {
        use crate::config::Config;
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut e = Editor::new_from_text("x\n", None, (40, 10));
        let mut cfg = Config::default();
        cfg.diagnostics.linters = Some(Vec::new());
        let mut warns = Vec::new();
        install_core_providers(&mut e, &cfg, &tx, &mut warns);
        assert!(!e.diag_providers.is_enabled(DiagSource::Harper), "empty list enables nothing");
        assert!(warns.is_empty(), "no unknown names to warn about");
    }

    // ------------------------------------------------------------------
    // Task 5 (E10): install_core_providers — the ltex catalog arm.
    // ------------------------------------------------------------------

    #[test]
    fn install_core_providers_registers_ltex_after_harper() {
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (40, 10));
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut warns = Vec::new();
        install_core_providers(&mut e, &crate::config::Config::default(), &tx, &mut warns);
        let sources: Vec<DiagSource> = e.diag_providers.sources().collect();
        assert_eq!(sources, vec![DiagSource::Harper, DiagSource::LTeX],
            "the complete catalog in cycle order — vale has no provider in this build");
        assert!(e.diag_providers.is_enabled(DiagSource::Harper), "both ship enabled");
        assert!(e.diag_providers.is_enabled(DiagSource::LTeX));
        assert!(warns.is_empty());
    }

    // ------------------------------------------------------------------
    // The vale removal: no provider, but the NAME is still recognised.
    // ------------------------------------------------------------------

    #[test]
    fn vale_has_no_provider_and_no_engine_menu_row() {
        // vale-ls lints the file on disk, never the synced buffer, so no provider is installed.
        // Nothing may pretend otherwise: no registration, no menu row, and the lens setter
        // refuses (there is no `analysis_engine_vale` command left to reach it with anyway).
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (40, 10));
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut warns = Vec::new();
        install_core_providers(&mut e, &crate::config::Config::default(), &tx, &mut warns);
        assert!(!e.diag_providers.sources().any(|s| s == DiagSource::Vale), "no vale provider");
        assert_eq!(e.active_analysis_source, DiagSource::Harper, "the lens seed is harper");
        assert!(warns.is_empty());
        let labels: Vec<String> = engine_menu_rows(&e).into_iter().map(|(l, _)| l).collect();
        assert!(!labels.iter().any(|l| l.starts_with("vale")),
            "no vale row in the engine menu; rows = {labels:?}");
        e.set_analysis_source(DiagSource::Vale);
        assert_eq!(e.active_analysis_source, DiagSource::Harper, "the lens did not switch");
    }

    #[test]
    fn an_explicit_linters_vale_entry_warns_honestly_and_enables_nothing() {
        // "vale" is a KNOWN name with no transport — the warning must say so rather than claim
        // the name is unknown, and it must never be dropped on the floor silently.
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (40, 10));
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut cfg = crate::config::Config::default();
        cfg.diagnostics.linters = Some(vec!["harper".into(), "vale".into()]);
        let mut warns = Vec::new();
        install_core_providers(&mut e, &cfg, &tx, &mut warns);
        assert!(e.diag_providers.is_enabled(DiagSource::Harper), "the rest of the list applies");
        assert!(!e.diag_providers.is_enabled(DiagSource::LTeX));
        assert!(!e.diag_providers.is_enabled(DiagSource::Vale), "and vale enables nothing");
        assert_eq!(warns, vec!["config: diagnostics.linters — \"vale\" is not available in this \
            build (vale-ls cannot lint unsaved buffers); ignoring it".to_string()]);
        assert!(!warns[0].contains("unknown"), "the name IS known — only the transport is gone");
    }

    #[test]
    fn an_unknown_linters_entry_still_warns_as_unknown() {
        // The vale arm must not swallow genuinely unknown names, and the known-set it prints
        // lists only engines that actually run.
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (40, 10));
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut cfg = crate::config::Config::default();
        cfg.diagnostics.linters = Some(vec!["grammarly".into()]);
        let mut warns = Vec::new();
        install_core_providers(&mut e, &cfg, &tx, &mut warns);
        assert_eq!(warns, vec!["config: diagnostics.linters — unknown engine \"grammarly\" \
            (known: harper, ltex)".to_string()]);
    }

    // ------------------------------------------------------------------
    // Task 10 (E10 §13): the config-only default-engine seed override.
    // ------------------------------------------------------------------

    #[test]
    fn default_engine_overrides_the_seed_when_enabled() {
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (40, 10));
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut warns = Vec::new();
        let mut cfg = crate::config::Config::default();
        cfg.diagnostics.default_engine = Some(DiagSource::LTeX);
        install_core_providers(&mut e, &cfg, &tx, &mut warns);
        assert_eq!(e.active_analysis_source, DiagSource::LTeX, "spec §13 override");
        assert!(warns.is_empty());
    }

    #[test]
    fn default_engine_disabled_falls_back_with_a_warning() {
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (40, 10));
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut warns = Vec::new();
        let mut cfg = crate::config::Config::default();
        cfg.diagnostics.default_engine = Some(DiagSource::LTeX);
        cfg.diagnostics.linters = Some(vec!["harper".into()]); // ltex NOT enabled
        install_core_providers(&mut e, &cfg, &tx, &mut warns);
        assert_eq!(e.active_analysis_source, DiagSource::Harper,
            "known-but-disabled → harper-first fallback (spec §13)");
        assert!(warns.iter().any(|w| w.contains("default_engine")),
            "the fallback is loud (config warning), never silent");
    }

    // ------------------------------------------------------------------
    // Task 8 (E10 §6): idle_shutdown_track / diag_idle_fire — the leaving-Review
    // transition seam that arms the heavy (ltex) engine's suspend deadline.
    // ------------------------------------------------------------------

    fn ltex_enabled_editor() -> crate::editor::Editor {
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (40, 10));
        e.diag_providers.install(Box::new(crate::diag_provider::RecordingProvider::new()
            .with_source(DiagSource::LTeX)), true);
        e.diag_cfg.enabled = true;
        e
    }

    #[test]
    fn leaving_review_arms_the_idle_due_and_reentry_clears_it() {
        use crate::test_support::TestClock;
        let mut e = ltex_enabled_editor();
        e.active_mut().view.mode = crate::editor::RenderMode::Review;
        // true → false (mode change out of Review).
        let before = should_run_diagnostics(&e);
        e.active_mut().view.mode = crate::editor::RenderMode::LivePreview;
        idle_shutdown_track(&mut e, before, &TestClock::new(1_000));
        assert_eq!(e.diag_idle_due, Some(1_000 + 15 * 60_000), "default 15 min (spec §6)");
        // false → true (re-entry) clears.
        let before = should_run_diagnostics(&e);
        e.active_mut().view.mode = crate::editor::RenderMode::Review;
        idle_shutdown_track(&mut e, before, &TestClock::new(2_000));
        assert_eq!(e.diag_idle_due, None, "the grace: re-entry cancels");
    }

    #[test]
    fn buffer_switch_out_of_review_also_arms() {
        use crate::test_support::TestClock;
        let mut e = ltex_enabled_editor();
        e.active_mut().view.mode = crate::editor::RenderMode::Review;
        e.install_scratch(); // a second, non-Review buffer
        let before = should_run_diagnostics(&e); // true (active is the Review buffer)
        e.switch_to_index(1); // scratch — Draft mode
        idle_shutdown_track(&mut e, before, &TestClock::new(500));
        assert!(e.diag_idle_due.is_some(),
            "the predicate transition fires on buffer switches, not only set_render_mode (spec §6)");
    }

    #[test]
    fn no_arm_when_ltex_disabled_or_zero_config_or_no_transition() {
        use crate::test_support::TestClock;
        // Disabled ltex: never arms.
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (40, 10));
        e.diag_cfg.enabled = true;
        e.active_mut().view.mode = crate::editor::RenderMode::Review;
        let before = should_run_diagnostics(&e);
        e.active_mut().view.mode = crate::editor::RenderMode::LivePreview;
        idle_shutdown_track(&mut e, before, &TestClock::new(0));
        assert_eq!(e.diag_idle_due, None, "no ltex entry → no arm");
        // Zero config: never arms.
        let mut e = ltex_enabled_editor();
        e.diag_cfg.ltex_idle_shutdown_min = 0;
        e.active_mut().view.mode = crate::editor::RenderMode::Review;
        let before = should_run_diagnostics(&e);
        e.active_mut().view.mode = crate::editor::RenderMode::LivePreview;
        idle_shutdown_track(&mut e, before, &TestClock::new(0));
        assert_eq!(e.diag_idle_due, None, "0 = keep warm forever (ruling 3)");
        // No transition: staying out of Review is a no-op (edge-triggered, not level).
        let mut e = ltex_enabled_editor();
        idle_shutdown_track(&mut e, false, &TestClock::new(0));
        assert_eq!(e.diag_idle_due, None);
    }

    #[test]
    fn diag_idle_fire_suspends_and_clears_once() {
        let mut e = ltex_enabled_editor();
        let rec = crate::diag_provider::RecordingProvider::new().with_source(DiagSource::Harper);
        let calls = rec.calls_handle();
        e.diag_providers.install(Box::new(rec), true);
        e.diag_idle_due = Some(1_000);
        diag_idle_fire(&mut e, 999);
        assert!(e.diag_idle_due.is_some(), "not yet due");
        diag_idle_fire(&mut e, 1_000);
        assert_eq!(e.diag_idle_due, None, "one-shot: cleared on fire");
        assert!(calls.lock().unwrap().iter().any(|c|
            matches!(c, crate::diag_provider::ProviderCall::Suspend)),
            "fire delegates to suspend_all_idle_heavy (every entry; SUSPENDABLE gating is provider-side)");
    }

    // ------------------------------------------------------------------
    // Task 9 (E10 §11): engine_menu_rows — the state-in-label matrix.
    // ------------------------------------------------------------------

    /// The COMPLETE spec-§11 label matrix — every cell of enabled×availability:
    /// disabled → "off" (wins over availability); enabled+Unavailable → "not installed";
    /// enabled+Starting → "warming…"; enabled+Ready → "on"; enabled+Idle → "on"; plus the
    /// command-less skips (Plugin, and Vale now that its provider and commands are gone).
    /// Three fixture editors cover the five cells across the two command-bearing sources.
    #[test]
    fn engine_menu_rows_state_labels_and_toggle_actions() {
        use crate::diag_provider::{Availability, RecordingProvider};
        use crate::menu::MenuRowAction;
        use crate::registry::CommandId;
        /// A fixture editor with the two core engines at the given (availability, enabled).
        fn fixture(cells: [(DiagSource, Availability, bool); 2]) -> crate::editor::Editor {
            let mut e = crate::editor::Editor::new_from_text("x\n", None, (40, 10));
            for (src, avail, enabled) in cells {
                e.diag_providers.install(Box::new(RecordingProvider::new()
                    .with_source(src).with_availability(avail)), enabled);
            }
            e
        }

        // Scenario A: on (Ready) / warming… — both ENABLED — plus the two command-less skips.
        let mut e = fixture([
            (DiagSource::Harper, Availability::Ready, true),
            (DiagSource::LTeX, Availability::Starting, true),
        ]);
        e.diag_providers.install(Box::new(RecordingProvider::new()
            .with_source(DiagSource::Plugin("mock"))), true); // skipped: no command (E12)
        // Skipped for the same reason: even an installed+enabled+Ready vale has no
        // `toggle_engine_vale` command for a row to name.
        e.diag_providers.install(Box::new(RecordingProvider::new()
            .with_source(DiagSource::Vale).with_availability(Availability::Ready)), true);
        let rows = engine_menu_rows(&e);
        assert_eq!(rows.len(), 2, "command-less sources are skipped (spec §11)");
        assert_eq!(rows[0], ("Harper — on".to_string(),
            MenuRowAction::Command(CommandId("toggle_engine_harper"))));
        assert_eq!(rows[1], ("LTeX — warming…".to_string(),
            MenuRowAction::Command(CommandId("toggle_engine_ltex"))));

        // Scenario B: enabled+Unavailable → "not installed"; disabled → "off" (wins over
        // availability, here a Ready recorder).
        let e2 = fixture([
            (DiagSource::Harper, Availability::Unavailable, true),
            (DiagSource::LTeX, Availability::Ready, false),
        ]);
        let rows2 = engine_menu_rows(&e2);
        assert_eq!(rows2[0].0, "Harper — not installed", "enabled + Unavailable → not installed");
        assert_eq!(rows2[1].0, "LTeX — off", "disabled wins over Ready availability");

        // Scenario C: the last two cells — enabled+Idle → "on"; disabled+Starting → "off".
        let e3 = fixture([
            (DiagSource::Harper, Availability::Idle, true),
            (DiagSource::LTeX, Availability::Starting, false),
        ]);
        let rows3 = engine_menu_rows(&e3);
        assert_eq!(rows3[0].0, "Harper — on", "enabled + Idle (not yet summoned) → on");
        assert_eq!(rows3[1].0, "LTeX — off", "disabled wins over Starting availability");
    }

    // ── T7: the session dismiss — pair key (sentence + line) + equality filter ────────────────

    #[test]
    fn dismissal_units_pair_sentence_and_line() {
        let e = crate::editor::Editor::new_from_text(
            "Para one here. Para two here.\n\nOther block.\n", None, (80, 24));
        let k = dismissal_units_at(&e.active().document.buffer, 16); // inside "Para two"
        assert_eq!(k.sentence, "Para two here.");
        assert_eq!(k.line, "Para one here. Para two here.");
    }

    /// ADDED (review FIX 2) — "blank" is TRIM-empty, not strict-empty. `line_text` strips only the
    /// trailing newline, so a whitespace-only separator used to count as paragraph CONTENT and the
    /// window swallowed the neighbouring block: the key was then bound to text the writer never
    /// selected. One fixture per walk direction: the two loops share ONE predicate, so both are
    /// pinned against a future edit that re-splits it.
    #[test]
    fn a_whitespace_only_line_is_a_blank_boundary() {
        // Backward walk: the heading above must NOT be pulled into the window.
        let up = crate::editor::Editor::new_from_text("# Title\n   \nBeta two.\n", None, (80, 24));
        let k = dismissal_units_at(&up.active().document.buffer, 12); // start of "Beta two."
        assert_eq!(k.sentence, "Beta two.", "the whitespace-only line above stops the window");
        assert_eq!(k.line, "Beta two.");
        // Forward walk: an unterminated first sentence must not run on into the block below.
        // (A terminated one would be cut by the segmenter anyway — no discrimination.)
        let down = crate::editor::Editor::new_from_text("Alpha beta\n \t \nGamma three.\n",
            None, (80, 24));
        let k2 = dismissal_units_at(&down.active().document.buffer, 0);
        assert_eq!(k2.sentence, "Alpha beta", "the whitespace-only line below stops the window");
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
        seed_slot(&mut e, vec![gdiag(26..28, "R")]); // "go" inside sentence 2
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
        // Round-3, AMENDED (review M1): what this actually pins is that a dismissal does not
        // reach a flag in a DIFFERENT PARAGRAPH whose sentence merely contains the dismissed one —
        // and here the two sentences also sit on different LINES, so the `line` conjunct alone
        // already decides it. A containment implementation of `sentence` still passes. The test
        // that isolates equality-vs-containment is the sibling
        // `containment_on_the_same_line_does_not_suppress`, where ONE line holds both sentences.
        // The segmenter keeps "Dr. Smith arrived." as ONE sentence (the shipped textobj doctest).
        let mut e = crate::editor::Editor::new_from_text(
            "Smith arrived. Yes.\n\nDr. Smith arrived. Indeed.\n", None, (80, 24));
        e.diag_providers.install(Box::new(crate::diag_provider::RecordingProvider::new()
            .with_source(DiagSource::LTeX)), true);
        dismiss_at(&mut e, 0, "R"); // key sentence: "Smith arrived."
        seed_slot(&mut e, vec![gdiag(25..30, "R")]); // "Smith" inside "Dr. Smith arrived."
        retain_unignored(&mut e);
        assert_eq!(slot_ranges(&e), vec![25..30], "superstring sentence ⇒ NOT equal ⇒ survives");
    }

    /// ADDED (not in the T7 brief) — the `line` half of the pair was otherwise UNPINNED: every
    /// brief fixture whose sentence units differ also has differing line units, so a key that
    /// compared `sentence` alone passed all nine. Here the two occurrences share ONE sentence
    /// unit (the sentence spans a hard line break, so the blank-line window cuts the same span
    /// for both) and differ only by line — which must be enough to keep the second flag.
    #[test]
    fn same_sentence_on_a_different_line_survives() {
        let mut e = crate::editor::Editor::new_from_text(
            "Alpha beta\ngamma delta.\n", None, (80, 24));
        e.diag_providers.install(Box::new(crate::diag_provider::RecordingProvider::new()
            .with_source(DiagSource::LTeX)), true);
        let k0 = dismissal_units_at(&e.active().document.buffer, 0);
        let k1 = dismissal_units_at(&e.active().document.buffer, 11);
        assert_eq!(k0.sentence, k1.sentence, "precondition: ONE sentence unit spans both lines");
        assert_ne!(k0.line, k1.line, "precondition: the line units differ");
        dismiss_at(&mut e, 0, "R"); // line unit "Alpha beta"
        seed_slot(&mut e, vec![gdiag(11..16, "R")]); // "gamma", line unit "gamma delta."
        retain_unignored(&mut e);
        assert_eq!(slot_ranges(&e), vec![11..16], "different line-unit ⇒ survives");
    }

    /// ADDED (not in the T7 brief) — the equality-vs-containment discriminator the brief's
    /// `dismissed_sentence_does_not_suppress_a_longer_containing_sentence` INTENDS but does not
    /// isolate: in that fixture the two sentences sit on different LINES, so a containment rule
    /// on `sentence` still leaves it green. Here both sentences share one line, and the dismissed
    /// "Smith arrived." is a strict substring of the candidate's "Dr. Smith arrived." — so only
    /// EQUALITY keeps the second flag.
    #[test]
    fn containment_on_the_same_line_does_not_suppress() {
        let mut e = crate::editor::Editor::new_from_text(
            "Smith arrived. Dr. Smith arrived.\n", None, (80, 24));
        e.diag_providers.install(Box::new(crate::diag_provider::RecordingProvider::new()
            .with_source(DiagSource::LTeX)), true);
        let k0 = dismissal_units_at(&e.active().document.buffer, 0);
        let k1 = dismissal_units_at(&e.active().document.buffer, 19);
        assert_eq!(k0.line, k1.line, "precondition: ONE line unit holds both sentences");
        assert!(k1.sentence.contains(&k0.sentence) && k1.sentence != k0.sentence,
            "precondition: the candidate sentence strictly CONTAINS the dismissed one");
        dismiss_at(&mut e, 0, "R"); // "Smith arrived."
        seed_slot(&mut e, vec![gdiag(19..24, "R")]); // "Smith" inside "Dr. Smith arrived."
        retain_unignored(&mut e);
        assert_eq!(slot_ranges(&e), vec![19..24], "containment is NOT equality ⇒ survives");
    }

    /// ADDED (review FIX 1) — the `(source, code)` half of the triple was UNPINNED: every fixture
    /// used ONE source (`LTeX`) and ONE code (`"R"`), so dropping either conjunct from the match
    /// left the whole battery green. The three tests below each hold the pair key FIXED and vary
    /// exactly ONE conjunct, so that conjunct alone decides the outcome.
    ///
    /// Here: another ENGINE's flag on the same sentence AND line. Without the `source` conjunct a
    /// writer dismissing an ltex rule would also silence harper/vale on that very text.
    #[test]
    fn a_dismissal_does_not_cross_engines() {
        let mut e = crate::editor::Editor::new_from_text(
            "Alpha beta gamma. Delta epsilon zeta.\n", None, (80, 24));
        dismiss_at(&mut e, 18, "R"); // LTeX / "R" / the "Delta epsilon zeta." pair
        let mut d = gdiag(18..23, "R");
        d.source = DiagSource::Harper; // SAME pair key, SAME code — only the engine differs
        let v = e.active().document.version;
        let slot = e.active_mut().diagnostics.slot_mut(DiagSource::Harper);
        slot.diagnostics = vec![d];
        slot.computed_version = v;
        retain_unignored(&mut e);
        let kept: Vec<std::ops::Range<usize>> = e.active().diagnostics.slot(DiagSource::Harper)
            .map(|s| s.diagnostics.iter().map(|d| d.range.clone()).collect()).unwrap_or_default();
        assert_eq!(kept, vec![18..23], "a different engine's flag on the same units survives");
    }

    /// The `code` conjunct in isolation: same engine, same pair key, a different RULE.
    /// Without it, dismissing one ltex rule would silence every other rule on that sentence.
    #[test]
    fn a_dismissal_does_not_cross_codes() {
        let mut e = crate::editor::Editor::new_from_text(
            "Alpha beta gamma. Delta epsilon zeta.\n", None, (80, 24));
        dismiss_at(&mut e, 18, "R");
        seed_slot(&mut e, vec![gdiag(18..23, "S")]); // SAME source, SAME pair key, code "S"
        retain_unignored(&mut e);
        assert_eq!(slot_ranges(&e), vec![18..23], "a different rule code survives");
    }

    /// The `None → ""` keying `DismissSet`'s doc specifies: a code-less engine still gets
    /// per-occurrence dismissal, not a wildcard over the key. Both flags below share ONE pair key
    /// and differ only in `code`, so the empty-string dismissal must take the code-less one and
    /// leave the coded one — the `unwrap_or_default` on the FILTER side.
    #[test]
    fn an_absent_code_keys_as_the_empty_string() {
        let mut e = crate::editor::Editor::new_from_text(
            "Alpha beta gamma. Delta epsilon zeta.\n", None, (80, 24));
        let key = dismissal_units_at(&e.active().document.buffer, 18);
        e.session_dismissals.insert((DiagSource::LTeX, String::new(), key));
        let mut no_code = gdiag(18..23, "R");
        no_code.code = None;
        // Both inside "Delta epsilon zeta." ⇒ identical pair key; only `code` separates them.
        seed_slot(&mut e, vec![no_code, gdiag(24..31, "R")]);
        retain_unignored(&mut e);
        assert_eq!(slot_ranges(&e), vec![24..31],
            "the code-less flag is dismissed; a coded flag on the same key is not");
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
        // Review M3: pin the headline claim. The block tree of a non-active buffer is deliberately
        // stale (the lazy-reparse law), so the filter must not touch it — a future edit reaching
        // for the lens/classifier here bumps this generation and trips the assert.
        let gen_before = e.by_id(target).unwrap().document.blocks_generation();
        apply_diagnostics_done(&mut e, target, v, DiagSource::LTeX, vec![gdiag(18..23, "R")]);
        assert_eq!(e.by_id(target).unwrap().document.blocks_generation(), gen_before,
            "no reparse of the non-active buffer — no tree was touched");
        // `unwrap_or(0)` would also yield 0 for a missing slot, so pin the slot's EXISTENCE first.
        let slot = e.by_id(target).unwrap().diagnostics.slot(DiagSource::LTeX)
            .expect("the apply created the LTeX slot on the target buffer");
        assert!(slot.diagnostics.is_empty(),
            "dismissal filtered on the non-active buffer's own text");
    }
}
