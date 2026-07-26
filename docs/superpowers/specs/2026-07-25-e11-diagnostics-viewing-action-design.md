# E11 — diagnostics viewing/action layer + fix-pipeline repair: design spec

**Date:** 2026-07-25. **Status:** draft for Codex spec gate.
**Item:** E11 (`backlog.toml` id `E11`, "Multi-engine linting (c)"). Scope decided D1–D8 in
`scratchpad/e11/decisions.md` (authoritative — where anything below and that file disagree, that
file wins). Grounding: `scratchpad/e11/fable-grounding.md` (pre-probe forks + post-probe
addendum); wire evidence: `scratchpad/e11/probe/ltex-probe-results.md` and
`scratchpad/e11/probe/vale/vale-probe-results.md` (raw-JSON transcripts on disk beside them).
**Branch:** `effort-e11-diag-viewing-action` off main. No source edits before plan execution.
All anchors are SYMBOL names (file + symbol), never line numbers.

**Framing intent (human, D1):** the live probes found the fix substrate broken three ways — ltex
fixes never parse (kind + edit-shape mismatch), vale fixes are lost to our batched codeAction
request, and the two standing overlay rows are semantically wrong for grammar diagnostics. E11 is
therefore ONE effort: repair the fix pipeline for all three engines, then build the viewing/action
layer on top of it. Larger than the filed `M`, deliberately.

---

## 1. Summary and locked decisions

Human decisions (D1–D8, `scratchpad/e11/decisions.md` — reasoning lives there; not re-derived and
not re-openable here):

1. **D1 — fold the fix-pipeline repair into E11.** One effort: repair + viewing/action layer.
2. **D2 — fixes are fetched ON DEMAND:** one `textDocument/codeAction` request for ONE diagnostic,
   at quick-fix-overlay open. Publishes stop being parked; the Assembly/watchdog machinery is
   DELETED, not extended. Five owed items (D2's list) are §3's spine: no send before the document
   is open on that engine; version-gated response drop; a per-request deadline degrading to an
   honest "fixes unavailable"; an in-overlay fetching state whose row list grows on arrival; and
   the explicit statement that this rewrites behavior encoded in harper's pinned tests (§3.7).
3. **D3 as amended by D9 + D10 — kind-aware action rows + a session dismiss keyed on (engine
   source, rule `code`, ENCLOSING SENTENCE — or ENCLOSING LINE for non-prose anchors).**
   "Ignore once"/"Add to dictionary" appear on Spelling diagnostics only; non-spelling
   diagnostics get "Dismiss for this session" — client-side, keyed on the PAIR of parse-free
   enclosing units, sentence AND line (§5.3 — a conjunctive refinement implementing D9's
   sentence identity, the one ltex itself uses for `hideFalsePositives`
   (`{"rule": …, "sentence": "^\\Q…\\E$"}`), and D10's line identity simultaneously; never the
   flagged surface), filtered alongside the session ignores. No persistence, no server round
   trip. Named limits (D9 + §5.3): identical flags inside one sentence-and-line are not
   separated; rewrapping the containing lines drops a session dismissal.
4. **D4 — a bottom-anchored detail box** (the `paint_prompt_detail` precedent) carrying the
   wrapped full message + `engine · code` attribution. **Nothing load-bearing lives in it** — it
   is allowed to vanish on small terminals (`prompt_detail_rect` returns `None` when <3 rows are
   free; that behavior is inherited on purpose).
5. **D5 — "Learn more" is an overlay ROW** (not a detail-box line — the box can vanish) that
   copies `href` to the clipboard via the shipped seam, with a mandatory status-line ack
   ("link copied"). The URL also shows in the detail box as a courtesy when there is room.
6. **Row-model refactor:** conditional rows force `DiagOverlay` off index arithmetic onto a row
   enum (§5.1).
7. **Cut/deferred (say-so-where-a-reader-wonders):** the executeCommand relay is **CUT** — a
   human-blessed edit of E11's own filed hook (D8; §8.1); LSP `severity` stays OUT of the core
   `Diagnostic` type (probe-settled: a per-engine constant that contradicts across engines —
   §8.2); the ltex dictionary settings-push is **deferred to E13** (D6); rule-level disable is
   **deferred to E14** (D7). E13/E14 are already filed in `backlog.toml`.

---

## 2. Grounding: what HEAD does, and what the wire actually looks like

### 2.1 The shipped pipeline (all verified at source)

- `lsp_client.rs::ClientState::on_publish`: converts diagnostics
  (`convert_diagnostics` — populates `range`/`kind`/`source`/`code`/`href`/`message`,
  `suggestions: Vec::new()`), then — unless empty — **parks** the converted set in an
  `Assembly` and sends ONE batched `textDocument/codeAction` over the whole raw array's UTF-16
  envelope (`raw_envelope`, `codeaction_request` with `context.diagnostics = raw`). Paint waits
  for `on_codeaction_response` or the assembly watchdog (`E::CODEACTION_TIMEOUT_MS = 5_000`).
- `lsp_client.rs::on_codeaction_response`: attaches suggestions via
  `lsp_rpc::quickfix_suggestion`, **at most ONE per diagnostic** (`d.suggestions.push(s); break;`
  — first matching action wins).
- `lsp_rpc.rs::quickfix_suggestion`: accepts ONLY `kind == "quickfix"` (exact string) and ONLY
  the `edit.changes[uri]` shape — `edit.documentChanges[]` short-circuits to `None`;
  command-only actions are dropped (test-pinned).
- `diag_overlay.rs::DiagOverlay`: rows by index arithmetic — `row_count() =
  suggestions.len() + 2`, `is_ignore()`/`is_add_dict()` compare `selected` against
  `suggestions.len()`/`+1`. The two trailing rows are UNCONDITIONAL.
- `search_ui.rs::diag_apply_selected`: ignore inserts the flagged SURFACE slice into
  `editor.session_ignores`; `diagnostics_run::retain_over_union` drops **only
  `DiagnosticKind::Spelling`** — so on a grammar/style diagnostic, "Ignore once" closes the
  overlay and changes nothing visible, and "Add to dictionary" appends the whole flagged range
  (a full sentence, for a passive flag) to `dictionary.txt`.
- The overlay's message is the window TITLE only (`render_overlays.rs::paint_diag` —
  single-line, ratatui-clipped). `Diagnostic.code`/`href` are populated and read by NOTHING
  (sole reads: the core type's own unit test).
- Detail-box precedent: `render_overlays.rs::paint_prompt_detail` +
  `chrome_geom.rs::prompt_detail_rect(area, status_row, lines) -> Option<Rect>` (bottom-anchored
  above the status row; `None` when fewer than 3 rows are free; per-line heading/item elision) —
  called from `render.rs::paint_status`, "painted *for* that one overlay, not a second render
  site" (the shipped comment; `RenderSite` keeps its single-valued axis).
- Clipboard copy-out seam: `Editor.clipboard_sync_request: Option<String>`, drained after reduce
  by `clipboard::drain_clipboard_intents` (Layer-1 backend + OSC 52 — works over SSH/tmux).

### 2.2 The wire, per probe (every engine-shape claim below traces HERE, not to the upstream scan)

| Fact | Evidence |
|---|---|
| ltex fix actions: `kind: "quickfix.ltex.acceptSuggestions"`, edits ALWAYS `edit.documentChanges[]`, never `edit.changes` | ltex probe Q2 (raw responses; "No action in either response used `edit.changes`") |
| ltex non-fix actions (`quickfix.ltex.addToDictionary`/`.hideFalsePositives`/`.disableRules`) are COMMAND-ONLY, client-handled | ltex probe Q2 (no `edit` field on any of the three; `_ltex.*` command payloads quoted) |
| ltex multi-candidate is real (2 accept-suggestion actions for `recieve`) | ltex probe Q2 full-range request |
| ltex sends `codeDescription.href` on EVERY diagnostic; `code` = ruleId; `source: "LTeX"`; `severity: 3` on all observed | ltex probe Q1 |
| vale fix actions: bare `kind: "quickfix"` + `edit.changes[uri]`; no command-only actions observed | vale probe §2 |
| vale multi-candidate is real (5 actions for one spelling diagnostic) | vale probe §2 |
| **vale drops non-spelling fixes from a BATCHED request** (all-8-in-context returned the same 5 spelling actions; each echoed all 8 diagnostics — indistinguishably incomplete); per-diagnostic requests returned each repetition fix (7/7) | vale probe §2 "counter-intuitive finding" + `lsp_probe_repetition_ca.py` |
| vale sends NO `codeDescription`; `severity: 1` on everything observed; native finding embedded verbatim under `data` (incl. `Link`, empty for built-ins) | vale probe §1 |
| harper: bare `"quickfix"` + `edit.changes` (the shipped parser's origin shape) | harper 2.1.0 verification pinned in `lsp_rpc::quickfix_suggestion`'s doc comment + its tests |
| executeCommand: ltex advertises only `_ltex.checkDocument`/`_ltex.getServerStatus`; vale only `cli.sync`/`cli.compile` | ltex probe Q5; vale probe §3 |
| vale settings channel inert (`didChangeConfiguration` → `logMessage` only; never pulls) | vale probe §4 |
| vale hover advertised but `null` on markdown prose (the documented trap) | vale probe §5 |

### 2.3 The ltex per-diagnostic question — RESOLVED by the T1 probe (Outcome A); the original
### premise was a fixture artifact

**Correction (T1 execution, `scratchpad/e11/probe/t1-perdiag-results.md`; this section
originally asserted an unexplained anomaly — that assertion was FALSE).** The pre-spec probe's
write-up claimed its single-diagnostic `floopmuffin` request returned only command-only actions
while the whole-document batch returned an accept-suggestion for that same diagnostic. T1
re-checked the pre-spec probe's own raw capture (`results_summary.json`,
`phase_a3_codeaction_all_response`): **`floopmuffin` receives NO `acceptSuggestions` action in
ANY request shape, including the batch** — the earlier write-up's prose contradicted its own
raw JSON. Root cause: LanguageTool has zero suggested replacements for the invented nonword, and
accept-suggestion actions are generated per-suggestion — the fixture could not demonstrate the
phenomenon in any construction. No construction-dependent gap ever existed.

**What T1 actually established (real `ltex-ls-plus` 18.7.0, target `recieve` — two real
LanguageTool candidates):** all three request variants (single raw echoed / all raws echoed /
caret-style empty range) elicit the `quickfix.ltex.acceptSuggestions` fixes identically, and
controls show the response tracks the requested RANGE, not the contents of
`context.diagnostics`. **Binding outcome A:** T4's default single-raw echo stands;
`FIX_CONTEXT_ALL_RAWS` stays `false` for every engine. The Outcome-B seam (the const + its
`TestEngineAllRaws` test) SHIPS as designed — dead-but-designed, tested insurance (controller
ruling at T1 adjudication; this spec concurs: a zero-cost tested seam beats deleting it
mid-execution). The vale probe had already established per-diagnostic requests work there
(7/7 repetition fixes).

The §3.3 raw-echo design (retain and echo the server's own bytes) stands on its remaining
rationale — eliminating reconstruction-fidelity as a failure mode — no longer on the retracted
anomaly. **Unchanged and NOT softened by this correction (probe-observed, request-construction-
independent):** the two real ltex breakages driving D1 — the shipped `kind == "quickfix"` gate
rejects `quickfix.ltex.acceptSuggestions`, and `quickfix_suggestion` reads only `edit.changes`
while ltex sends exclusively `edit.documentChanges[]` (§2.2's table; §4's repair).

---

## 3. The on-demand fix lifecycle (D2)

### 3.1 What is deleted

From `lsp_client.rs`: the `Assembly` struct, the `assembling: HashMap` field, the assembly arm of
`on_deadline`, `raw_envelope`, `PendingKind::CodeAction`, and `on_publish`'s parking branch —
`on_publish` now ALWAYS emits `Msg::DiagnosticsDone` immediately on conversion (empty or not).
Underlines paint on publish, unconditionally — this removes the shipped up-to-5 s paint stall
behind the batch round trip, for every engine. `on_codeaction_response` is replaced by the §3.4
response path. `lsp_rpc::quickfix_suggestion` is replaced by the §4 mapping.
`Diagnostic.suggestions` in the STORE is now always empty; the field remains the carrier on the
overlay's anchor (§3.5).

### 3.2 The request seam

- `DiagnosticsProvider` gains ONE defaulted method — returning `Accepted`, mirroring
  `notify_change`'s disconnected-send contract (gate finding 1: a `()` return would let a dead
  client thread strand the overlay in Fetching with no slot for any deadline to rescue):
  ```rust
    /// E11 §3: fetch fix candidates for one diagnostic, on demand. `token` is the request's
    /// correlation identity (§3.4 — minted per request; the ONLY value the reduce arm keys
    /// delivery on). `Accepted::Yes` ⟹ exactly one terminal `Msg::DiagFixesReady` carrying
    /// this token is guaranteed (§3.4); `Accepted::No` ⟹ nothing will be emitted — the
    /// caller must resolve the overlay's fetch state itself. Default: `Accepted::No`
    /// (non-LSP providers offer no fixes).
    fn request_fixes(&mut self, token: u64, buffer_id: BufferId, version: u64,
        range: std::ops::Range<usize>, code: Option<String>, message: String) -> Accepted {
        let _ = (token, buffer_id, version, range, code, message); Accepted::No
    }
  ```
  `LspProvider::<E>::request_fixes` sends
  `Cmd::RequestFixes { token, buffer_id, version, range, code, message }` and maps the send result
  exactly as `notify_change` does: `Ok` → `Accepted::Yes`; `Err` (channel disconnected — the
  client thread is gone) → `set_availability(Unavailable)` + `Accepted::No`. Non-blocking
  either way (hot-path law). The request carries the anchor's `code` + `message` clones
  because a byte range alone cannot deterministically identify the raw diagnostic to echo
  (gate finding 6: overlapping/identical ranges exist; `code`/`message` are preserved
  verbatim by `convert_diagnostics`, so the triple (converted range, code, message) matches
  the raw object exactly — fully identical raws are mutually indistinguishable and any one
  serves). `ProviderCall` gains `RequestFixes { .. }` and `RecordingProvider` records it with
  a settable `Accepted` (test observability — the E10 `Suspend` lesson).
- **The correlation token (gate round-2 Critical 1):** `Editor` gains a monotonic
  `next_fix_token: u64` counter; the `quick_fix` handler mints `token = next_fix_token` (then
  increments) per request and stores it on the overlay (`DiagOverlay.fix_token: Option<u64>`,
  `Some` iff the request was accepted). The token rides `Cmd::RequestFixes` → `PendingFix` →
  `PendingKind::FixRequest` → `Msg::DiagFixesReady` unchanged, and the reduce arm delivers ON
  TOKEN EQUALITY ALONE (§3.4). Why identity fields cannot correlate: `Editor::open_diag`
  closes-and-reopens with the ACTIVE buffer's current version, so reopening the same
  diagnostic unedited mints a new overlay with the SAME (buffer_id, opened_version,
  anchor.range) — a displaced/expired request A's empty terminal would pass every field guard
  and clear the NEW request B's Fetching state before B terminates. A per-request token is the
  only correct discriminator.
- The sole caller: the `quick_fix` command handler (`registry.rs`), immediately after
  `Editor::open_diag` — routed to the ACTIVE lens engine
  (`editor.diag_providers` keyed by `editor.active_analysis_source`, via a `ProviderSet`
  delegation `request_fixes(source, token, ..) -> Accepted` mirroring the existing
  source-keyed delegations, `None`-entry → `Accepted::No`). The overlay's fetch state is set
  from the RESULT: `Accepted::Yes` → `FixState::Fetching` + `fix_token = Some(token)`;
  `Accepted::No` (unavailable engine, dead thread, defaulted provider) → `FixState::Done` with
  the `NoFixes` row immediately and `fix_token = None` — no silent wait, and no state that
  only a message could clear when no message is coming.
- **`FlushGuard` coverage (gate finding 1):** the drop-drain arm in `lsp_client.rs::FlushGuard`
  currently drains only unread `Cmd::Change`; it is EXTENDED to also emit an empty
  `Msg::DiagFixesReady` for any unread `Cmd::RequestFixes` in the channel, and
  `flush_outstanding` emits the same for a live `pending_fix` slot — so thread death between
  accept and apply cannot strand a Fetching overlay.

### 3.3 In the state machine: the `pending_fix` slot + raw retention

`ClientState<E>` gains:
- **Raw retention, with a REAL attribution rule (gate round-3 Critical 1 — supersedes the
  round-2 tag-only design):** `DocState` gains
  `last_raw: Option<(u64, Vec<serde_json::Value>)>`, but the tag's TRUTH cannot come from the
  ambient `d.our_version` — `on_publish` attributes with the CURRENT document version, the
  `params.version` echo is optional (harper omits it, per the shipped comment), and a
  generation distinguishes reopens, not successive `didChange` snapshots. Two rules make the
  tag trustworthy:
  1. **Generation retirement on watchdog expiry — WITHOUT leaking the retired document
     (round-4 Important 2):** when `on_deadline` expires a publish-await for buffer B, it
     (i) removes B's current URI from `uri_owner`, (ii) pushes
     `Action::Send(textDocument/didClose)` for that URI (the child is alive — a publish
     timeout is not a crash; server-side the obsolete document is closed, not left checking
     forever), (iii) clears B's `last_raw`, and (iv) marks `DocState.open = false`. The next
     check then re-opens under a FRESH generation/URI via the existing `on_change` reopen
     branch, and any late publish for the timed-out change carries the RETIRED URI → dropped
     whole at the `uri_owner` lookup, never converted, never stored, never tagged. The
     explicit removal + didClose are load-bearing: the shipped reopen branch only INSERTS the
     new mapping (today's only reopen path follows a respawn, where `on_server_gone` clears
     `uri_owner` wholesale and the dead server needs no didClose) — a timeout-retirement
     reopen without them would strand one `uri_owner` entry and one open server-side document
     PER TIMEOUT, unbounded. T4 pins the no-leak property (after N retirement cycles,
     `uri_owner` holds exactly the live URIs and a didClose was sent per retired one).
     **T4-review fold-in (adjacent PRE-EXISTING debt, not new E11 behavior): `on_spawned` now
     also clears `uri_owner`.** The suspend→unpark resume path deliberately bypasses
     `on_server_gone` (the expected-EOF drain), and `on_spawned` — unlike `on_server_gone` —
     never cleared the map, so every idle-suspend/resume cycle stranded the pre-suspend URI's
     entry. That leak shipped with E10's suspend feature (merge `9c2d9e4`); it is
     correctness-safe (a late publish to a stranded URI fails the `docs` generation check and
     drops) but unbounded in principle. Clearing in `on_spawned` is semantically right (it
     marks every doc closed, retiring every open URI) and harmless on the crash path (where
     `on_server_gone` already cleared). With it the no-leak invariant is UNIFORM across all
     three reopen paths — crash-respawn, watchdog retirement, and suspend/resume — pinned by
     the T4 cycle-count test extended to the resume path.
  2. **Await-attribution:** `last_raw` is stored ONLY when the publish ANSWERS a live
     `awaiting_publish` entry, and its tag is the AWAIT's `our_version` (`a.our_version` —
     recorded when the soliciting didOpen/didChange was sent), never the ambient
     `d.our_version`. Unsolicited publishes (e.g. ltex's config-triggered republishes,
     probe-observed in Q3) do NOT update `last_raw`. Within a never-timed-out generation the
     app-side in-flight latch guarantees at most one solicited check outstanding per buffer,
     so an answering publish's await-version is the honest snapshot identity.
  **The residual, and why a mis-attributed request cannot become a wrong EDIT (round-4
  Critical 1 — consequence bounded by MECHANISM, not prose):** with a version-omitting server,
  one mis-attribution interleaving survives any client-side rule — an UNSOLICITED republish
  (reflecting older text) arriving while a newer change's await is live answers that await. No
  wire data distinguishes it, and the spec claims no proof. What it claims instead is that the
  APPLY PATH is structurally incapable of consuming the questionable attribution, because of
  four shipped/specified facts (each symbol-cited, each test-pinned in T2/T4/T5):
  1. **Server ranges never reach the apply.** The §4 mapping returns `Suggestion` values —
     TEXT ONLY (`ReplaceWith(String)`/`InsertAfter(String)`/`Remove`; the core enum carries no
     range). The server's edit ranges are consumed solely as a MATCHING key (edit range must
     equal the anchor range against current text) and then discarded.
  2. **The applied edit is client-constructed from the frozen anchor:**
     `diag_apply_selected` builds `build_range_replace(a, b, t, doc_len)` from the ANCHOR's
     clamped range and the user-SELECTED suggestion string — never from anything the server
     sent except the visible string itself.
  3. **The document is frozen between open and apply:** the overlay is modal
     (`diag_overlay::intercept` consumes all keys — no user edits), and any non-user edit
     bumps `document.version`, which `diag_apply_selected`'s `opened_version` refusal rejects
     ("document changed; re-open").
  4. **The string is seen before it is applied:** suggestion rows RENDER the replacement text
     (`suggestion_label`); Enter applies what the writer read.
  Consequence envelope of the residual, precisely: a stale-provenance fetch can at worst
  DISPLAY a stale/nonsense suggestion row (a display-quality defect, the same class as the
  shipped mis-attributed underline) — it cannot cause an edit other than "replace the visible
  underlined range with the visible chosen string, undoably, at the frozen version." The
  earlier draft's other two bounds are DEMOTED to mitigations, per the round-4 audit: the
  send-time triple-match filters but proves nothing, and server-side ordering is not a
  guarantee our code enforces. Belt: the §3.4 delivery guard also requires the overlay's
  `opened_version` to still equal the buffer's current version, so stale-fetch rows are not
  even shown once the document has moved. Bounded memory: one tagged array per open doc.
  (`wordcartel-core` stays serde-free — retention is entirely shell-side.)
- **One request slot** (the overlay is XOR-single, so at most one fix request can be live):
  ```rust
    pub(crate) struct PendingFix { token: u64, buffer_id: BufferId, version: u64,
        range: std::ops::Range<usize>, code: Option<String>, message: String,
        deadline: u64, sent_id: Option<u64> }
    pub(crate) pending_fix: Option<PendingFix>,
  ```
  **Materialized in `on_inbound` itself, in EVERY phase — `Cmd::RequestFixes` is NEVER pushed
  onto `queued`** (gate finding 2: the non-Running queue arm holds bare `Cmd`s that
  `next_deadline()` cannot see, so a queue-side request during a JVM warm would get its
  deadline only after initialization — minutes of visible Fetching, violating D2's leash).
  Like `Cmd::Suspend`, `RequestFixes` gets first-class routing ahead of the generic queue arm:
  it sets/replaces `pending_fix` with `deadline = now + FIX_REQUEST_TIMEOUT_MS` immediately, so
  the slot's deadline is visible to `next_deadline()` from the moment of acceptance, in any
  phase — the 10 s leash genuinely covers held + warm + server latency. The slot IS this
  command's queue.
  **Replacement rule (gate finding 3 — made consistent with exactly-once):** replacing a live
  slot (a newer overlay supersedes an older request) FIRST emits an empty `DiagFixesReady`
  terminal for the displaced request AND removes its `sent_id`'s `PendingKind::FixRequest`
  entry from `pending_requests` (a late response then falls to the existing unknown-id arm —
  no double emission). Exactly-once therefore holds universally, not just for the surviving
  request; the displaced terminal is dropped harmlessly by the reduce-side delivery guards
  (its overlay is gone). `on_deadline` expires a due slot the same way: empty terminal, clear
  slot, remove the sent-id entry if any.
- **Materialize the WIRE REQUEST at SEND time, never at slot-creation time (D2's constraint):**
  the JSON is built only when the slot is actually SENT — from the THEN-current `DocState.uri`
  (generation-fresh), the anchor range converted against the THEN-current `DocState.text`, and
  the raw diagnostic selected from `last_raw` by the deterministic triple match — converted
  byte range equality AND `code` AND `message` equality (gate finding 6) — echoed verbatim in
  `context.diagnostics`. A slot-creation-time URI would name a retired generation after a
  suspend/resume (the D2 analysis). No triple match in `last_raw` → the slot resolves as an
  empty terminal (the fix target no longer exists server-side).
- **Send condition (guard 1 — the sub-tick ordering edge + snapshot provenance):** the slot is
  sent only when the machine is `Running` AND the target document's `DocState.open` is true AND
  **`last_raw` is tagged with exactly `PendingFix.version` AND `DocState.our_version ==
  PendingFix.version`** (one snapshot, all three artifacts — text, raws, request — provably
  from it) AND the tagged array holds a triple-matching raw diagnostic. The send attempt
  happens (a) at slot creation when the conditions already hold, (b) after the queue replay in
  `on_initialized`, and (c) after `on_publish` stores a fresh tagged `last_raw` for that
  buffer — the moments the conditions can newly become true. A slot that never becomes
  sendable is expired by its own deadline (no infinite hold; the deadline has been live since
  slot creation, §above).
- **Change-invalidation rule (the other half of round-2 Important 2):** when `on_change`
  advances a buffer to a version NEWER than a pending slot's `version`, the slot is resolved
  IMMEDIATELY with its empty terminal (and its `sent_id` de-registered) — the fix target
  belongs to a superseded snapshot, and the overlay's own `opened_version` guard would refuse
  any apply after that edit anyway (`diag_apply_selected`'s "document changed; re-open"). This
  kills the mismatched-snapshot class at its source instead of leaning on downstream guards,
  and it means any still-pending slot satisfies `our_version == version` by construction.
- The response is tracked via `PendingKind::FixRequest` on `pending_requests` (replacing the
  deleted `CodeAction` variant), carrying `{ token, buffer_id, generation, version, range }` —
  the token IS in the variant (round-3 Minor 5: the payload list is the implementer's
  contract; correlation must never route back through mutable state).
- **Document-close leg (gate round-3 Important 2; didClose de-dup per round-5 Important 2):**
  `on_close(buffer_id)` — reached from `workspace::close_buffer_now` via
  `ProviderSet::notify_close_all` — additionally resolves a matching `pending_fix` (same
  buffer) with its empty token terminal, de-registers its `sent_id`, and clears the buffer's
  `last_raw`. Without this, a response landing after the close would consume its `PendingKind`
  entry, be dropped as stale-generation, and leave the accepted request with NO terminal —
  the one leg the round-2 matrix missed. **And `on_close` sends `textDocument/didClose` ONLY
  when the removed `DocState` is actually OPEN** (`d.open`) — the shipped arm sends it
  unconditionally on `docs.remove`, which was correct while every closed doc was open, but a
  §3.3 timeout-retired doc (present, `open = false`, its URI already didClosed and removed
  from `uri_owner` at retirement) would otherwise get a SECOND didClose for the retired URI.
  State removal and request/raw bookkeeping still run unconditionally; only the wire frame is
  gated on `d.open`.

Constant: `FIX_REQUEST_TIMEOUT_MS: u64 = 10_000` (in `lsp_client.rs` — engine-agnostic; a flat
leash covering held+sent time; during a long JVM warm the writer gets an honest "no fixes
available" within 10 s and can reopen to retry — chosen over a warm-aware deadline so the overlay
never sits in "fetching" for minutes).

### 3.4 The response path + marshaling

New message: `Msg::DiagFixesReady { token: u64, buffer_id: BufferId, version: u64,
source: DiagSource, range: std::ops::Range<usize>, suggestions: Vec<Suggestion> }` — `token`
is the delivery key (round-2 Critical 1); the identity fields ride along for debug
asserts/logging, never for correlation.

- `on_server_response` routes `PendingKind::FixRequest` to a new
  `on_fix_response`: **drop if the token is not the live slot's (a displaced/expired request's
  late response — its terminal was already emitted); a stale-generation response FOR the live
  slot resolves it empty immediately** (T4-review amendment: the response consumed the
  request's `pending_requests` entry, so nothing can ever answer that request again — waiting
  out the deadline would be a knowingly-futile "fetching…", a no-silent-UI violation; the
  immediate empty resolution IS the token's one terminal). Else map every action via the §4
  mapping against the anchor range, and emit `DiagFixesReady` with ALL matched suggestions
  (possibly empty). Clear the slot.
  **Guarantee: every `Accepted::Yes` `RequestFixes` produces exactly one `DiagFixesReady`
  carrying its token** — from exactly one of: the server response (fix-bearing, empty, or the
  stale-generation immediate resolution above); the deadline expiry (empty);
  replacement by a newer request (empty, emitted at replacement time, §3.3); the
  change-invalidation resolution (empty, §3.3); the no-triple-match resolution (empty, §3.3);
  the DOCUMENT-CLOSE resolution (empty, at `on_close`, §3.3); `on_server_gone`'s flush
  (`pending_fix` joins `flush_outstanding`'s coverage); or `FlushGuard` drop (both the
  in-machine slot and any UNREAD `Cmd::RequestFixes` still in the channel — the drain arm is
  extended, §3.2). An `Accepted::No` produces nothing and the CALLER
  resolves the overlay state synchronously (§3.2). A wedged Fetching overlay is thereby
  impossible in every leg: accepted requests always terminate, and unaccepted ones never enter
  Fetching.
- Reduce side (guard 2 — TOKEN-keyed CONSUMPTION, version-gated DISPLAY — the round-5
  Important-1 correction): a new thin `Msg::DiagFixesReady` arm. On
  `overlay.fix_token == Some(msg.token)` the terminal is ALWAYS consumed for the overlay —
  version mismatch may suppress suggestion DELIVERY, never terminal CONSUMPTION (a token's
  exactly-once terminal is the only one that will ever come; dropping it strands `Fetching`
  forever — the exact hazard the round-4 draft created, reachable because non-key messages
  pass through the modal intercept, so a background mutation can bump `document.version`,
  trigger the §3.3 change-invalidation terminal, and the round-4 guard would then discard
  precisely that terminal). Concretely: token match + `opened_version ==
  document.version` → deliver the suggestions; token match + version MISMATCH → close the
  overlay with the shipped "document changed; re-open" sticky status (the same semantics
  `diag_apply_selected` applies to a stale overlay — every other row is equally stale, so a
  zombie NoFixes overlay would be worse than closure). Any OTHER terminal (a displaced
  request's, an
  expired one's, one for a closed overlay) is dropped silently — token equality is the SOLE
  correlation, because reopening the same diagnostic unedited reproduces every identity field
  (round-2 Critical 1's failure sequence) while tokens are unique by construction. On
  delivery: `anchor.suggestions = suggestions`, the fetch state clears (§5.2), `selected`
  moves per the §5.2 post-delivery selection policy (row identity; deliberate reset for the
  vanished fetching row — never a bare clamp). The STORE is never written — suggestions are
  overlay-lifetime data.

### 3.5 The ltex idle-suspend interaction (grounded in D2; restated as the invariant the tests pin)

Out of Review nothing paints and `quick_fix` refuses (`active_lens_diags` → `None`), so a parked
engine's steady state cannot receive a fix request. The only live window is post-re-entry warm:
stale-but-version-valid underlines paint while the JVM respawns; a `RequestFixes` arriving there
**materializes the pending slot immediately (deadline live from that moment) — it is NEVER
enqueued; only the `Change` queues and replays** (§3.3's first-class routing). The slot persists
in the machine across the resume, its send-attempt fires after the replayed `Change`'s didOpen
and the fresh publish re-tags `last_raw` (attempt sites (b)/(c)), and the wire request is
materialized against the fresh URI — or the slot expires at 10 s into an honest "no fixes
available." State-machine tests drive exactly this interleaving (suspend → change → unpark →
request-slot → replay → publish → send → response) — the E10 lesson that ORDERINGS of
concurrent inputs are where the real bugs live.

### 3.6 Overlay open flow (recap)

`quick_fix` → `Editor::open_diag(d)` (unchanged XOR/close-all/`opened_version` semantics) → fire
`request_fixes` at the active source → the overlay opens IMMEDIATELY with client-side rows +
the fetch state set from the `Accepted` result (§3.2, §5.2). No path blocks; Esc always works.

### 3.7 ⚠ This deliberately rewrites behavior that harper's pinned tests encode

E10's T1 pin protected a behavior-PRESERVING extraction: harper's inline tests
(`harper_ls.rs::tests`) encode the publish→park→batched-codeAction→attach pipeline.
**E11 is a deliberate behavior CHANGE, and those tests change with it** — rewritten to pin the
NEW contract (publish emits immediately; fixes are on-demand; the flush covers `pending_fix`).
The complete pinned-behavior blast radius (gate finding 7 — the inventory the plan carries):
- `harper_ls.rs::tests`: `publish_teh` and every consumer of it; the assembly/watchdog/
  stale-response set (`nonempty_publish_then_codeaction_attaches_replace_with_and_drops_command_only`,
  `assembly_superseded_generation_is_discarded_not_emitted_with_new_ranges`,
  `stale_codeaction_response_does_not_consume_the_newer_assembly`,
  `codeaction_watchdog_emits_converted_suggestionless`,
  `assembly_result_then_eof_does_not_re_emit_empty_for_the_same_version`,
  `codeaction_watchdog_then_eof_no_duplicate_terminal`);
  `grammar_gate_drops_grammar_diagnostics_when_disabled` (asserts a non-empty publish sends
  `textDocument/codeAction` and populates `assembling` — the grammar GATE survives, the
  assembly assertions do not); `flush_outstanding_covers_all_three_tracks_and_is_idempotent`
  (three tracks become awaiting + queued + `pending_fix`).
- `lsp_rpc.rs::tests`: the `quickfix_*` set pins exact-kind `"quickfix"` and `edit.changes`-only
  behavior — superseded by the §4 mapping's tests.
- `diag_overlay.rs::tests::diag_window_follows_selection` (via `tall_diag`) pins
  `row_count == suggestions + 2` — superseded by the `DiagRow` model's tests.
- `lsp_client.rs::tests`: the T2/T3-era tests referencing assembly/`CODEACTION_TIMEOUT_MS`
  behavior, ported to the slot model.
This is stated here so no reviewer meets it as a surprise: the pin was scoped to the extraction,
not granted eternity. Harper's fixes also become on-demand (unify all three engines; per-
diagnostic `codeAction` is harper's native shape — it is where the parser's `"quickfix"` +
`edit.changes` expectations came from).

---

## 4. The suggestion mapping repair (probe-settled; D1 item 1)

`lsp_rpc::quickfix_suggestion` is replaced by an engine-parameterized mapping (home:
`lsp_rpc.rs`, engine knowledge injected — exact seam to the plan):

- **Per-engine fix-kind acceptance** via a new `LspEngine` hook:
  ```rust
    /// Is this CodeAction kind a FIX this engine delivers as an edit? (E11 §4 —
    /// probe-grounded per engine; command-only kinds are excluded by knowledge, not luck.)
    fn is_fix_kind(kind: &str) -> bool;
  ```
  `HarperEngine`/`ValeEngine`: `kind == "quickfix"`. `LtexEngine`:
  `kind == "quickfix.ltex.acceptSuggestions"` (its `addToDictionary`/`hideFalsePositives`/
  `disableRules` kinds are command-only and deliberately excluded — the client-handled set D3's
  rows own natively).
- **Both edit shapes:** `edit.changes[uri]` (harper/vale, probe-verified) AND
  `edit.documentChanges[]` (ltex, probe-verified exclusive) — the latter iterating
  `documentChanges[].edits[]`, matching `textDocument.uri` against ours, same
  newText/range→`Suggestion` rules as today (`ReplaceWith`/`Remove`/`InsertAfter` mapping
  unchanged).
- **Attach ALL matching actions** (the `break` dies with `on_codeaction_response`): §3.4's
  `on_fix_response` collects every action whose kind passes `E::is_fix_kind` and whose edit
  matches the anchor range, in response order (probe: vale returns candidates best-first;
  ltex returned `receive` before `relieve`). Dedupe identical `Suggestion` values.

---

## 5. The overlay: row model, kind-aware rows, session dismiss, learn-more (D3, D5)

### 5.1 The row-model refactor (forced; index arithmetic cannot express conditional rows)

`DiagOverlay` drops `is_ignore`/`is_add_dict` index arithmetic for a computed row list:

```rust
#[derive(Clone, PartialEq, Eq, Debug)]
// `pub`, not `pub(crate)`: `diag_overlay` is a `pub mod` and `rows()` is a `pub fn`, so a
// crate-private return type trips `private_interfaces` and breaks the warning-free build gate.
// Its neighbours `DiagOverlay` and `FixState` are `pub` for the same reason.
pub enum DiagRow {
    Suggestion(usize),   // index into anchor.suggestions
    FetchingFixes,       // §5.2 — present only while a fetch is live
    NoFixes,             // §5.2 — terminal, only when a fetch came back empty/expired
    LearnMore,           // only when anchor.href.is_some()
    IgnoreOnce,          // Spelling only
    AddToDictionary,     // Spelling only
    DismissSession,      // non-Spelling only
}
impl DiagOverlay {
    pub fn rows(&self) -> Vec<DiagRow> { /* pure fn of anchor + fetch state */ }
}
```

`row_count`/`up`/`down`/`selected` keep their shapes over `rows().len()`;
`diag_apply_selected` matches on `rows()[selected]` instead of index comparisons; paint
(`paint_diag`) and mouse (`mouse_diag`, `chrome_geom::diag_row_at`) read the same `rows()` —
one source of truth, no drifting arithmetic. Row labels: existing strings for the carried rows;
`"Learn more (copy link)"`, `"Dismiss for this session"`, `"fetching fixes…"`,
`"(no fixes available)"` (the two fetch rows are non-activatable — Enter on them is a no-op).

### 5.2 The fetch state (D2 owed item 4 — no silent wait)

`DiagOverlay` gains `fix_state: FixState` — `enum FixState { Fetching, Done }`
(`Fetching` iff the open-time `request_fixes` returned `Accepted::Yes`; `Done` at
delivery/expiry, or immediately at open on `Accepted::No` — §3.2).
While `Fetching` and `anchor.suggestions` is empty, the `FetchingFixes` row shows; when
`DiagFixesReady` lands, suggestion rows appear; an empty delivery shows `NoFixes`. The overlay
never blocks and never silently waits — the states are visible rows.

**Post-delivery selection policy (gate round-3 Important 4 — an async message must not
silently re-aim Enter):** naive index clamping would leave `selected == 0` pointing first at
the non-activatable `FetchingFixes` row and then, after delivery, at `Suggestion(0)` —
converting a documented no-op into a document edit with no navigation input. The policy,
with tests:
- Across ANY `rows()` change, selection is preserved **by ROW IDENTITY** (the `DiagRow` value
  the writer had selected, re-located in the new list) — a writer parked on `IgnoreOnce`
  stays on `IgnoreOnce` when suggestion rows appear above it.
- When the selected row VANISHED (`FetchingFixes` on delivery, replaced by the results), the
  selection is a **deliberate reset to the first row of the new list** (`Suggestion(0)` on a
  non-empty delivery; `NoFixes` on an empty one) — deterministic and test-pinned, not a
  side effect of clamping.
- `FetchingFixes`/`NoFixes` remain no-op rows under Enter at ALL times (activation tests).
  **Ordering scope note (T6-review regression fix):** inertness governs EXECUTION — an inert
  row never performs an edit, at any time — while the stale-overlay `opened_version` guard
  runs FIRST in `diag_apply_selected` and closes with the shipped "document changed;
  re-open" warning regardless of which row is selected. The guard shipped ahead of row
  handling; hoisting an inert-row early-return above it is a behavior change, not a free
  refactor.
- Named residual: an Enter already in flight when delivery lands executes against the reset
  row — the overlay's primary action, applied as a normal UNDOABLE edit with the caret moved
  to it (the standard async-popup trade; bounded by undo and by the visible list change).
  Tests pin the deterministic halves: Enter-while-Fetching is a no-op; the post-delivery
  selection lands exactly per this policy; identity-preservation for user-moved selections.
`scroll_top` continues to follow via the existing `keep_overlay_visible` two-layer invariant.

### 5.3 Kind-aware rows + the session dismiss (D3)

- `IgnoreOnce`/`AddToDictionary` rows: `anchor.kind == DiagnosticKind::Spelling` only. Their
  handlers are unchanged (surface-word semantics are CORRECT for spelling).
- `DismissSession` row: non-Spelling only. Handler inserts into a new
  `Editor.session_dismissals: HashSet<(DiagSource, String, DismissKey)>` — **the D9 key: (engine
  source, rule `code` or `""`, ENCLOSING SENTENCE text)**. This is the identity ltex itself
  uses for `hideFalsePositives` (probe-captured `{"rule": …, "sentence": "^\\Q…\\E$"}`); it is
  stable across republishes and edits elsewhere, distinguishes identical wording in different
  sentences, and carries D9's named limit (two identical flags inside ONE sentence are not
  separated).
- **The key is the PAIR of parse-free units — a conjunctive refinement of D9∧D10 (round-4
  Important 3):** `struct DismissKey { sentence: String, line: String }` — BOTH the enclosing
  sentence-unit (the shared parse-free derivation, §below) AND the enclosing line-unit
  (`buffer.byte_to_line(anchor.range.start)` → `lines::line_text(buffer, line)`,
  newline-stripped; both symbols verified), computed identically at dismiss time and filter
  time, on any buffer, with no classification step at all. Why the pair: a kind-TAGGED key
  classifies the STORED unit but not the CANDIDATE — on a non-active buffer no parse-free
  candidate classification exists, so a stored prose sentence could suppress a marker-free
  non-prose match (a setext heading `"Title"`) and vice versa. **The precise claim (round-5
  wording correction — the pair carries no domain information and this spec claims no
  categorical domain separation):** under the pair, cross-domain suppression requires equality
  of BOTH parse-free units — so any cross-domain match falls inside the named
  IDENTICAL-TEXT collision class (two positions textually indistinguishable in both their
  line and their sentence-window shape), the same limit class D9 already accepts for
  identical sentences. Context-sensitive Markdown can give identical source text different
  block roles; when it does, the pair treats them as the same text — documented behavior, not
  separation. The pair is STRICTLY NARROWER than either decided key alone (every pair-match
  is both a D9-sentence match and a D10-line match — mathematically, not rhetorically), so it
  refines the locked decisions rather than amending them: sentence discrimination for prose
  (D9), line discrimination for non-prose (D10), each enforced simultaneously. Named limits: two identical flags inside one
  sentence-and-line are not separated (D9's limit, unchanged); and REWRAPPING the containing
  paragraph (an edit to the dismissed text's own lines) changes the line-unit and drops the
  dismissal — acceptable for session-scoped state (the flag honestly returns after a rewrite
  of its surroundings). Never the flagged surface (D10's rejection stands); never
  no-row-at-all (a heading false positive keeps its escape hatch).
  `commands::prose_sentence_at` is NO LONGER needed anywhere in this feature — both units are
  rope+`textobj` derivations, valid on any buffer.
- **Filter-time matching is PARSE-FREE *unit EQUALITY*, never containment (gate round-3
  Important 3):** containment only proves the stored string occurs somewhere near the range —
  a dismissed heading line `"Title"` would also suppress a body diagnostic inside
  `"The Title is tentative."`, a dismissed sentence would match as a substring of a longer
  one, and an empty stored key would match everything. D9/D10 demand equality with the
  ENCLOSING UNIT, so the filter derives the candidate diagnostic's OWN enclosing unit and
  requires string equality:
  - A candidate matches a stored `DismissKey` iff its (source, code-or-empty) matches AND its
    line-unit — `lines::line_text(buf, buf.byte_to_line(d.range.start))`, rope-only — EQUALS
    `key.line` AND its sentence-unit EQUALS `key.sentence` (the PAIR rule, §above; the cheap
    line equality short-circuits first). No kind tag, no candidate classification — any
    cross-domain match requires both-unit textual identity, the named collision class
    (round-4 Important 3, wording per round-5 Minor 3).
  - The sentence-unit is derived by a shared parse-free helper (plan-named, in
    `diagnostics_run`): expand from the diagnostic's line to the nearest
    blank-line/document boundaries by rope line iteration (the SOURCE-level paragraph — no
    block tree), then `wordcartel_core::textobj::sentence_bounds(window, rel)` (the pure core
    segmenter — the same machinery the lens uses, minus the lens's window). **Dismiss time
    uses this SAME helper (and the same `line_text` call) for the stored pair** — both sides
    of every equality are computed by one derivation, so prefixed-block edge cases
    (blockquotes, list items, where the source-level window and the lens window differ) shift
    BOTH sides identically instead of silently never matching.
  - A key whose line-unit is EMPTY is REFUSED at store time (a diagnostic cannot sit on an
    empty line; belt against the empty-key hole — and under the pair rule an empty
    sentence-unit alone can never over-match, since the line-unit must also be equal).
  - Why parse-free is load-bearing: `apply_diagnostics_done` can land on a NON-active buffer
    (a result arriving after a buffer switch), and non-active buffers' block trees are
    deliberately stale — the standing lazy-reparse invariant forbids running the lens
    classifier there. Rope line iteration + the pure segmenter touch no tree. Cost:
    O(paragraph) per dismissal per candidate, on the cold apply/refilter path only.
  This keeps ltex-precedent alignment (its `hideFalsePositives` regex `^\Q<sentence>\E$` is
  whole-unit EQUALITY too — anchored, not substring), and preserves the T7 discriminators:
  Codex's `"Title"`-in-body counterexample now survives (the body diagnostic's enclosing
  line/sentence ≠ `"Title"`).
- Filtering call sites: `diagnostics_run::retain_over_union` generalizes to one pass over both
  sets — unchanged word-union behavior for Spelling, plus the PAIR-EQUALITY rule above for the
  dismissals. Same call sites (`retain_unignored`, `apply_diagnostics_done`); the
  dismissal takes effect immediately (in-place refilter, no server round trip) and re-applies
  to every future publish this session. Not persisted (dies with the process, like
  `session_ignores`).
- **Considered decisions (T7 review — deliberate, not overlooked; a later reader must not
  "fix" these by accident):**
  1. **The empty-line refusal CLOSES the overlay** — consistent with `diag_apply_selected`'s
     close-regardless-of-outcome contract and its shipped failure precedents (add-to-dict IO
     error; "no dictionary path configured"). The one principled exception is Learn-more,
     which stays open because a copy is not an OUTCOME of the overlay's purpose — a refusal
     is.
  2. **The refusal belt's asymmetry is designed:** `.line` is the true "nothing to key on"
     guard; an empty `.sentence` cannot over-match because pair EQUALITY still requires the
     line-unit to match exactly, bounding it inside the documented collision class.
  3. **The filter's `O(diagnostics × paragraph)` cost is ACCEPTED:** it runs per debounced
     publish, never per keystroke, and the (source, code) pre-filter prunes before any unit
     derivation. Honest worst case: a blank-line-free document with many same-rule
     diagnostics (each derivation scans to document bounds). Both halves of the escape hatch,
     recorded: splitting `dismissal_units_at` to short-circuit stays FORBIDDEN (the
     identical-derivation rule exists because a split lets the two sides disagree); the
     sanctioned fix, if live use ever shows the worst case, is a byte-cap on the window scan
     INSIDE the single shared function — both sides inherit the same bound by construction.
     Deliberately not implemented now.

### 5.4 Learn more (D5)

Row present iff `anchor.href.is_some()` — **ltex-only in practice** (probe: every ltex
diagnostic carries `codeDescription.href`; vale built-ins send none; harper never) — the row is
conditional, so this is self-correcting if a future vale style supplies links. Handler:
`editor.clipboard_sync_request = Some(href.clone())` (the shipped copy-out intent, drained by
`clipboard::drain_clipboard_intents` — Layer-1 backend + OSC 52, so it works over SSH/tmux) +
the MANDATORY ack `editor.set_status(StatusKind::Info, "link copied")` (D5: copy is invisible by
nature; without the ack the row reads as dead — the exact failure class this effort exists to
fix). The overlay stays open (copying is not a dismissal).

---

## 6. The detail box (D4)

A bottom-anchored, multi-line box shown while (and only while) the quick-fix overlay is open,
carrying: the WRAPPED full `anchor.message`, an attribution line `{source.label()} · {code}`
(code omitted when `None`), and — courtesy only — the `href` when present. Mechanism mirrors the
prompt precedent exactly:

- Geometry: a `chrome_geom::diag_detail_rect(area, status_row, overlay: Rect, lines) ->
  Option<Rect>` modeled on `prompt_detail_rect` (same width ladder/centering; bottom-anchored
  directly above the status row; `None` when fewer than 3 rows are free) **plus one rule the
  prompt helper does not have (gate finding 4): the box's TOP edge is capped strictly below
  `overlay.y + overlay.height`, and the helper returns `None` when no legal row remains under
  that cap.** The two rects are NOT disjoint by nature — `palette_overlay_rect` sits at the
  top-quarter bias but `prompt_detail_rect` may otherwise consume every row above the status
  row (its own test permits rows 0..6 on a 7-row screen), and paint order makes overlap a
  CLOBBER, not a blend (`paint_status`, which paints the box, runs BEFORE the frame-overlay
  walk that paints `paint_diag` last). The caller passes
  `palette_overlay_rect(area, diag.row_count())` — the same helper the overlay's paint and
  hit-test already share, so the cap tracks the live overlay geometry by construction. The box
  vanishing on small terminals is WHY nothing load-bearing lives in it: the action rows,
  including Learn-more, stay in the centered overlay.
- Paint: a `render_overlays::paint_diag_detail(frame, diag, area, status_row, cs)` called from
  `render.rs::paint_status` beside the existing `paint_prompt_detail` call, guarded on
  `editor.diag.is_some()` — the shipped "painted *for* that one overlay, not a second render
  site" pattern (`RenderSite` keeps its single-valued axis; no `OVERLAYS` table change).
- Content rules: message wrapped to the box width (plain wrapping — prose, not path-shaped;
  the prompt's left-elision rule is for paths and is NOT inherited), attribution + href as
  single truncated lines, and the prompt precedent's last-row `…and N more` summary when the
  wrapped message exceeds the available rows.
- Overlap: disjointness is ENFORCED by the overlay-rect cap above, not assumed from the two
  anchors — the plan pins it with a property test sweeping terminal sizes × row counts ×
  message lengths asserting `diag_detail_rect ∩ overlay == ∅` or `None`.

---

## 7. Command-surface-contract conformance (explicit statement, grounded)

**E11 adds NO command, NO user-settable option, NO menu row, NO keybinding, and touches no
palette/hint surface.** Conformance reasoning per `docs/design/command-surface-contract.md`
(not asserted — grounded):

- The new overlay rows (Learn-more, Dismiss, the fetch states) follow the SHIPPED precedent of
  the existing quick-fix rows: "Ignore once"/"Add to dictionary" are overlay-internal list rows,
  not registry commands — they have never appeared in the palette, and the overlay's keys
  (`Up`/`Down`/`Esc`/`Enter`) are hardcoded in `diag_overlay::intercept`, not routed through the
  `KeyTrie` (verified; the palette-completeness invariant tests have never enumerated overlay
  rows). The contract's laws govern commands, options, palette, menu, and hints; its
  dynamic-section exemption note governs MENU rows — neither creates an obligation for rows
  inside an input overlay, and the shipped rows are the standing interpretation.
- Law 2 (every user-settable option is a command) is not triggered: `session_dismissals` is
  session-transient working state like `session_ignores` (not persisted, not configurable), and
  no config key is added.
- Law 6 (shared setters) is not implicated: the rows mutate via the same single handlers the
  shipped rows use (`diag_apply_selected` and its new arms), and no profile/second caller exists.
- The commands that exist stay untouched: `quick_fix` gains the fire-the-fetch line (same id,
  label, binding `ctrl-.`); `diag_next`/`diag_prev`/`recheck_diagnostics` unchanged.
**No contract amendment.**

---

## 8. Cuts and deferrals (so a reader of the filed hook is not surprised)

1. **The executeCommand relay is CUT (D8) — a human-blessed edit of E11's own filed hook** (the
   hook names it; this spec is the recorded scope edit, not a silent divergence). Killed by
   observed capability: ltex advertises only `_ltex.checkDocument`/`_ltex.getServerStatus`
   (probe Q5) and its dictionary/hide/disable operations are command-only, CLIENT-handled by the
   server's own design — there is nothing to relay them to; vale-ls advertises
   `cli.sync`/`cli.compile` (style-package chores, probe §3); harper's server commands duplicate
   the shipped client-side mechanism. A seam with no caller. Re-file if a concrete command ever
   earns it. At merge, E11's `backlog.toml` hook text is updated accordingly (drift gate).
2. **Severity stays OUT of the core `Diagnostic` type.** Probe-settled: a per-engine constant
   that CONTRADICTS across engines (ltex `3`/Warning on everything including spelling; vale
   `1`/Error on everything including style nits) — displaying it would rank a vale repetition
   nit above an ltex spelling error. The cross-engine-honest category remains our
   `DiagnosticKind`.
3. **ltex dictionary settings-push → E13** (filed, size S). Probe-confirmed viable; cut on scope
   discipline (invisible to the writer — the client-side union already suppresses before paint).
4. **Rule-level disable → E14** (filed, size M). Needs a persistence-home design (the app has
   never written config back) and owes a command under law 2 — a real design fork of its own.
   D3's session dismiss covers the immediate escape-hatch need.

---

## 9. What E11 does NOT touch

- The dispatch/store spine: `arm_enabled`/`dispatch_diagnostics`/`dispatch_one`/`DiagStore`/
  `apply_diagnostics_done`'s routing, the debounce/latch machinery, `active_lens_diags` — all
  unchanged (the retain filter gains the dismissal set inside the existing pass; the store never
  holds suggestions).
- The underline PAINT path: `render.rs::gather_row_ctx`/`row_spans_placed` and everything S6/S8
  — untouched (diagnostics still enter as byte-ranged values through `active_lens_diags`).
  `render.rs`'s only edit is the one guarded `paint_diag_detail` call in `paint_status` (§6).
- The engine lifecycle: warm deadlines, suspend/resume, availability, the E10 lifecycle
  predicates (`should_run_diagnostics` — never a `RenderMode::Review` literal in new code).
- The E11-adjacent status quirk (all enabled engines arm regardless of active lens; degraded
  hints race the one status slot) — observed, out of scope, recorded in the grounding.

---

## 10. Task decomposition (ordered; each task ends green)

**T1 — the ltex per-diagnostic wire probe (NO app code).** Re-run the ltex probe with the §3.3
raw-echo request shape + variants (exact-range vs cursor-position; single-raw vs all-raw
context) to pin what elicits `acceptSuggestions` per-diagnostic (§2.3). Output: a results file
in `scratchpad/e11/probe/`; its findings BIND the plan's request-construction task (and select
the §2.3 fallback if needed). Advisory-style verdict quoted in the pre-merge report.

**T2 — the mapping repair (TDD, `lsp_rpc.rs` + `LspEngine::is_fix_kind`).** Both edit shapes,
per-engine kinds, attach-all + dedupe — unit-tested against verbatim probe JSON (the raw
transcripts are on disk; use their exact action objects as fixtures).

**T3 — delete the parking; publish emits immediately (TDD, `lsp_client.rs`).** Remove
`Assembly`/watchdog-arm/`raw_envelope`/`PendingKind::CodeAction`; `on_publish` emits on
conversion. **`last_raw` ownership (round-4 Minor 5, stated so the split cannot be read two
ways): T3 adds ONLY the `DocState.last_raw` field and its clearing sites (close; the §3.3
retirement) — it stores NOTHING (the field stays `None`; no fix requests exist yet, so this
is inert and green). ALL storage — the await-attribution rule — lands in T4 with the rest of
the attribution machinery; an interim ambient-version store would recreate the round-3
Critical.** Includes the deliberate rewrite of the affected harper pinned tests (§3.7) — the
task's description says so in its commit message.

**T4 — the `pending_fix` slot + request/response/deadline/flush (TDD).** §3.3/§3.4 complete:
`Cmd::RequestFixes` (token-carrying) with first-class `on_inbound` materialization (slot +
deadline live in EVERY phase, never queued), the §3.3 ATTRIBUTION rules — await-attribution
(`last_raw` stored only when a publish answers a live await, tagged with the await's
`our_version`; unsolicited publishes never stored) + generation retirement on watchdog expiry
(`on_deadline` removes the retired `uri_owner` entry, sends `didClose`, clears `last_raw`,
marks the doc closed; the round-3 Critical-1 regression: a late publish to the retired URI is
dropped whole, never tagged; the round-4 Important-2 no-leak regression: after N retirement
cycles `uri_owner` holds exactly the live URIs, one didClose sent per retired; the round-5
Important-2 sequence: timeout retirement → buffer close BEFORE reopen → exactly ONE didClose
total for that URI, state fully removed) — the one-snapshot send
condition, the change-invalidation rule, wire-materialization-at-send with the triple-match
raw selection, the three attempt sites, the replacement-emits-terminal + de-registers-sent-id
rule, the DOCUMENT-CLOSE resolution (`on_close` resolves a matching slot empty + de-registers
— the round-3 Important-2 regression: close then deliver the response, assert the token
terminal was already emitted and nothing double-fires), `PendingKind::FixRequest` (token in
the variant), `FIX_REQUEST_TIMEOUT_MS`, the extended `FlushGuard` drain + `flush_outstanding`
slot coverage, the full exactly-once matrix (§3.4 — response / expiry / replacement /
change-invalidation / no-match / CLOSE / server-gone / guard-drop), and the
suspend→change→unpark→request-slot→replay→publish→send ordering test (§3.5) — including the
deadline-fires-DURING-warm case (round-1 finding-2 regression) and the
stale-raw-after-didChange case (round-2 finding-2 regression).

**T5 — provider seam + reduce arm (TDD).** `DiagnosticsProvider::request_fixes -> Accepted`
(token-first signature, defaulted `No`) + the `LspProvider` impl (disconnected-send →
`Unavailable` + `No`, the `notify_change` mirror) + `ProviderSet` delegation +
`ProviderCall::RequestFixes`/recorder with settable `Accepted`; `Editor.next_fix_token` +
`DiagOverlay.fix_token`; the `Msg::DiagFixesReady` reduce arm — token-keyed CONSUMPTION,
version-gated DISPLAY (§3.4) — with the round-2 Critical-1 regression test (close-and-reopen
the same diagnostic unedited, deliver request A's displaced terminal, assert it does NOT
clear request B's Fetching state) AND the round-5 Important-1 strand regression (a background
non-key mutation bumps `document.version` while Fetching; the change-invalidation terminal
arrives token-matched but version-mismatched; assert the overlay is CLOSED with the shipped
"document changed; re-open" status — the terminal is CONSUMED, never dropped, no eternal
Fetching); `quick_fix` mints the token, fires the fetch, and sets `FixState` FROM the
`Accepted` result (No → immediate `NoFixes`, no phantom Fetching).

**T6 — the row-model refactor (TDD, `diag_overlay.rs` + `search_ui.rs` + paint/mouse).**
`DiagRow` + `rows()`; `diag_apply_selected` re-keyed; kind-aware row visibility;
`FetchingFixes`/`NoFixes` states; paint/mouse read `rows()`; the §5.2 post-delivery selection
policy (row-identity preservation; deliberate reset for the vanished fetching row) with the
round-3 Important-4 tests: Enter-while-Fetching is a no-op, delivery-under-a-parked-selection
preserves the row by identity, delivery-while-on-FetchingFixes resets deterministically.

**T7 — session dismiss (TDD).** `Editor.session_dismissals: HashSet<(DiagSource, String,
DismissKey)>` with `DismissKey { sentence, line }` + the `DismissSession` handler (both units
via the shared parse-free derivations, §5.3 — no classification step, no
`prose_sentence_at`) + the PAIR-EQUALITY filter in the generalized retain pass
(non-active-buffer apply covered WITHOUT touching any tree — the lazy-reparse invariant
test) + immediate-refilter + reapplies-on-republish + the counterexample tests: a dismissed
heading line `"Title"` leaves a body diagnostic inside `"The Title is tentative."` flagged
(round-3); a dismissed sentence does not suppress a longer sentence containing it verbatim
(round-3); an empty line-unit key is refused at store time (round-3); a dismissed PROSE
sentence does not suppress a setext heading whose LINE unit differs, and a dismissed heading
line does not suppress prose on a textually different line (round-4 cross-domain,
non-identical pairs); an ACTUALLY-IDENTICAL pair across roles IS suppressed —
pinned as a documented-behavior test of the named identical-text collision class, not claimed
as separation (round-5 Minor 3). The constructible cross-role fixture (plan-gate round 3 —
the earlier setext example fails because its underline joins the blank-line window, but
Markdown LAZY CONTINUATION does not: an unmarked `Title` line directly under `> Quote.` is
blockquote content, role-non-prose, yet derives sentence unit `Title` — `Quote.` terminates
the preceding sentence — and line unit `Title`, byte-identical to an isolated one-line
`Title` paragraph elsewhere); plus the D9 discriminator (identical wording in a DIFFERENT
sentence survives) and the D10 discriminator (a heading dismissal stays scoped to that line)
and the named rewrap limit pinned as a documented-behavior test.

**T8 — learn-more row (TDD).** Conditional row, `clipboard_sync_request` set, the "link copied"
ack, overlay-stays-open.

**T9 — the detail box (TDD).** `diag_detail_rect(area, status_row, overlay, lines)` (geometry
tests: the `None`-on-tiny inheritance + the overlay-cap disjointness property sweep of §6) +
`paint_diag_detail` + the `paint_status` call — rendered-screen tests (the C5 lesson: render
the screen, don't assert the struct).

**T10 — live probe (mandatory-run, advisory-pass).** Drive the real app against real
harper/ltex/vale-ls: open fixes on each engine (multi-candidate visible on vale spelling and on
ltex `recieve`-class diagnostics), fetch state visible, dismiss works on a grammar diagnostic,
learn-more copies on an ltex diagnostic, detail box appears/vanishes with terminal height,
suspend→re-enter→fix-during-warm degrades honestly. Verbatim summary line in the pre-merge
report, plus `scripts/smoke/run.sh` quoted per the standing convention.

Dependencies: T1 → T4 (request construction); T2 → T4; T3 → T4 → T5 → T6 → {T7, T8}; T9 after
T6; T10 last.

---

## 11. Risks + honest flags

1. **The ltex per-diagnostic question (§2.3) — RESOLVED, Outcome A.** T1 falsified the
   original anomaly premise (a fixture artifact: a nonword with zero LanguageTool
   replacements; the pre-spec write-up contradicted its own raw capture) and established that
   every request variant elicits the fixes identically. The default single-raw echo stands;
   the Outcome-B seam ships as tested, dead-by-default insurance.
2. **The harper pinned-test rewrite (§3.7)** — deliberate, stated, scoped to the tests that
   encode the parked pipeline.
3. **The D9∧D10 pair key (§5.3)** — every dismissal stores BOTH parse-free units (enclosing
   sentence AND enclosing line), and a match requires EQUALITY of both — never containment,
   never a surface key (D10's rejection stands). Strictly narrower than either decided key
   alone; the named limits are D9's identical-text class plus rewrap-of-the-containing-lines
   dropping a session dismissal.
4. **`FIX_REQUEST_TIMEOUT_MS = 10_000`** — a named constant with a stated rationale (§3.3); the
   value is a judgment call the gate may tune.
5. **Module budgets:** `lsp_client.rs` net-shrinks (Assembly machinery out, slot in);
   `diag_overlay.rs`/`search_ui.rs` grow modestly; `render_overlays.rs` gains one painter. No
   hub gains dispatch bulk; new behavior enters via the row enum + one reduce arm + one trait
   method (registration-seam discipline).

## 12. Merge gates (per CLAUDE.md)

`cargo test` green across all suites; `cargo build`/`test --no-run` warning-free; workspace
clippy clean (`too_many_lines`/module budgets); backlog drift gate green (E11 status/hook edit at
merge; E13/E14 already filed); PTY smoke suite run + one-line summary quoted verbatim
(mandatory-run, advisory-pass); T1/T10 probe verdicts quoted (advisory). Spec/plan honor the
command-surface contract per §7 (no amendment).
