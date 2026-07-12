# Multi-engine prose linting + one-shot `wc.async` — design-space exploration

**Status:** DESIGN-SPACE (pre-brainstorm, 2026-07-12). Grounding + fork map for a prospective effort:
multi-engine prose linting (harper + ltex-ls-plus + vale/vale-ls + future engines, each its OWN
namespaced view) and the deferred one-shot `wc.async` subprocess primitive. Sibling of
`docs/design/effort-p-plugin-system-design-space.md`. **Not a committed design.** Every architectural
claim is anchored to a real symbol; human-only product decisions are flagged inline and listed at the end.

Grounding inputs: the live tool-capability scan (`docs/design/prose-linters-scan.md`, treated as ground
truth on harper-ls / ltex-ls-plus / vale / vale-ls), the real code at HEAD (P1–P3 plugin system shipped),
and the recorded intentions (`CLAUDE.md`, `command-surface-contract.md`, the E7/H2 archive, the Fresh
windowing notes in `docs/ux-backlog.md` S1).

---

## 0. Two reframings the code forces on the brief (challenge-the-framing first)

Before answering the questions, two places where the real code pushes back on the brief's assumptions —
both de-risk the effort:

1. **The "biggest net-new" client action layer LARGELY EXISTS.** The brief (echoing the scan's §D) says
   `DiagnosticsProvider` "has NO code-action / executeCommand / apply-edit / add-to-dictionary path." That
   is **true of the trait**, but **false of the client UX**: a full quick-fix surface already ships —
   `DiagOverlay` (`diag_overlay.rs:12`), opened by the `quick_fix` command (`registry.rs:462`), with rows =
   `suggestions + "ignore once" + "add to dictionary"`, applied by `diag_apply_selected` (`search_ui.rs:131`):
   apply-suggestion → `commands::build_range_replace` (undoable edit), ignore-once → `session_ignores`,
   **add-to-dictionary → `editor.dictionary.insert` + `append_word_to_dict` (`diagnostics_run.rs:140`) +
   `diag_provider.reload_dictionary()`**. So the *client-handled dictionary mutation* — the exact pattern
   ltex-ls-plus deliberately pushes onto the client (scan §B/§D-iii) — is **already implemented for harper**.
   A multi-suggestion selector also mostly exists: `DiagOverlay` already lists `anchor.suggestions` as rows
   (`diag_overlay.rs:59-65`); harper just rarely supplies >1. **Net-new is therefore narrower than framed:**
   per-diagnostic *doc-links* + a *detail region*, *per-engine* (non-harper) dict/rule writers, and an LSP
   *executeCommand relay* — not a from-scratch action layer.

2. **A reusable LSP transport already exists.** `lsp_rpc.rs` is *engine-agnostic* LSP plumbing —
   Content-Length framing (`write_frame`/`read_frame`), JSON-RPC over `serde_json::Value`,
   generation-stamped opaque URIs (`doc_uri`), UTF-16↔byte position conversion, and codeAction
   `TextEdit`→`Suggestion` mapping (its own module doc: *"No process IO lives here — see harper_ls.rs"*).
   Only the *process IO + protocol state machine* is harper-specific (`harper_ls.rs`). A 2nd/3rd LSP
   provider reuses `lsp_rpc.rs` wholesale and clones the `harper_ls.rs` lifecycle template — so
   "LSP-everywhere" is cheap, and the seam's *"Open-Closed insurance for provider #2"* claim
   (`diag_provider.rs:3-4`) is **verified accurate**: harper is the sole `Box<dyn DiagnosticsProvider>`
   today, and the trait is the extension point — **NOT the plugin system**.

These two facts recolor the whole decomposition: the spine is a *core-type + seam generalization*, the
viewing layer is an *extension of an existing overlay*, and the LSP transport is *already built*.

---

## 1. Multi-provider generalization — what must change, and where the single-provider assumption lives

**Where "one provider" is baked in (all real anchors):**
- `Editor.diag_provider: Box<dyn DiagnosticsProvider>` — a **single** boxed provider (installed in
  `app::run`; `NullProvider` default, `HarperLs` real).
- `DiagStore` (`diagnostics_run.rs:7`) — **one** per-buffer store: `{ diagnostics: Vec<Diagnostic>,
  computed_version, recheck_due_at, in_flight_version }` on `Buffer.diagnostics` (`editor.rs:154`). One Vec,
  one in-flight latch, one debounce — no room for N engines' results side-by-side.
- `Msg::DiagnosticsDone { buffer_id, version, diagnostics: Vec<Diagnostic> }` (`app.rs:60`) — **no
  provider/source id**; `apply_diagnostics_done` (`diagnostics_run.rs:150`) blindly overwrites
  `b.diagnostics.diagnostics`.
- **`wordcartel_core::diagnostics::Diagnostic` (`wordcartel-core/src/diagnostics.rs:40`) = `{ range, kind:
  DiagnosticKind{Spelling,Grammar}, message, suggestions }` — NO `source`, `code`, `severity`, or `href`.**
  This is the load-bearing gap: a namespaced-per-engine view *needs* a source tag; jump-to-rule-docs needs
  `href` (scan §B: ltex/vale populate `codeDescription.href`, harper never does); rule-level disable needs
  `code` (LanguageTool `ruleId` / vale `alert.check`).
- `dispatch_diagnostics` (`diagnostics_run.rs:72`) — snapshots one `(buffer_id, version, path, text)`, calls
  ONE `provider.notify_change`, latches ONE `in_flight_version`.
- `should_run_diagnostics` (`diagnostics_run.rs:34`) gates the *whole* subsystem on
  `RenderMode::Review` — one on/off, not per-engine.
- Status bar shows a single `editor.diag_provider.name()` (`render_status.rs:27`, "REVIEW · Harper").

**Minimal multi-provider shape (design space, not committed):**
- **Core type (the enabler):** add to `Diagnostic` a `source: DiagSource` (a small tag —
  `enum DiagSource { Harper, LTeX, Vale, Plugin(&'static str) }` or an interned `&'static str`), plus
  `code: Option<String>` (rule id) and `href: Option<String>` (codeDescription). Keep `kind`
  (Spelling/Grammar) or fold it into a `severity`/category. This is a `wordcartel-core` change — the one
  place a design review will push hardest, because `Diagnostic` is the shared vocabulary. Recommend the
  **enum tag** (exhaustive-match discipline, no free-form strings — mirrors the `SemanticElement` /
  `MenuRowAction` exhaustive-literal house style) with a `Plugin(&'static str)` arm for future engines.
- **Provider set:** `Editor.diag_provider: Box<dyn>` → `Editor.diag_providers: Vec<Box<dyn
  DiagnosticsProvider>>` (or a tiny name-keyed registry). harper stays index 0 / a bundled core provider.
  Each keeps its OWN `Availability` (they warm/degrade independently — critical for the JVM outlier, §2).
- **Store:** `DiagStore` becomes source-partitioned — either a `BTreeMap<DiagSource, Vec<Diagnostic>>` +
  per-source `in_flight_version`, or the single Vec keeps source-tagged items and the VIEW/nav filters by
  the active source. The LOCKED decision (separate namespaced views, never merged) means the render + nav
  layer shows **one engine's diagnostics per lens** (or per distinct style — see ⚠OPEN 3), so a
  source-keyed store is the natural fit.
- **Marshaling:** `Msg::DiagnosticsDone` gains a `source: DiagSource`; `apply_diagnostics_done` routes to
  the right sub-store and clears *that source's* in-flight latch. `FlushGuard`'s terminal-completion
  guarantee (`harper_ls.rs:660`) generalizes per-provider (each provider's guard flushes its own accepted
  `(buffer_id, version)`).
- **Dispatch fan-out:** `dispatch_diagnostics` iterates the Ready providers, hands each the same full-doc
  snapshot, latches each's in-flight independently. `should_run_diagnostics` stays the global Review gate;
  a per-engine enable (config, §3) filters *which* providers fan out.
- **View selection:** a new `editor.active_analysis_source: DiagSource` (which engine's view the writer is
  looking through) + a set-per-state command per engine + a cycle (`analysis_next` / `analysis_engine
  <name>` — contract law 8, §5). `diag_next`/`diag_prev`/`quick_fix` operate on the active source's store.

**Note on `DiagnosticsConfig.linters: Option<Vec<String>>` (`config.rs:82`):** a *forward-declared but
unused* multi-linter config hook already exists. It is the natural home for the enabled-engine list (§3) —
evidence the multi-engine future was anticipated at the config layer, matching the "provider #2 insurance"
at the seam layer.

---

## 2. One-shot vs LSP — with the harper precedent as governing input

**What the recorded record actually says about the harper decision (grounding, not recollection).** The
E7/H2 archive (`docs/backlog-archive.md`) shows harper went to `harper-ls` (resident LSP subprocess, an
`optdepends` like pandoc/xclip) for **two written reasons**: (a) the **dependency-weight shed** — dropping
the ~389-crate `harper-core` tensor stack from the binary (H2 → option D); and (b) **latency lands where
it's fine** — confining grammar to the deliberately-entered `Review` view (E7) relocates the checker's cost
off the drafting hot path. The one-shot `harper-cli` existed but lost; the archive frames the win as
dep-shed + latency-in-view, *not* primarily "one-shot vs LSP stability" (that is the brief's recollection —
worth noting the written rationale is narrower). harper-ls is *resident and cheap* (`Availability::Idle`
at rest, `ensure_running` lazy-spawns the `"wcartel-harper-client"` thread only when Review first needs it,
`harper_ls.rs:609`).

**Does that precedent govern the three new runtimes? It splits per tool:**

- **ltex-ls-plus — LSP is FORCED (no CLI exists), and it is the heavy outlier.** 300 MB JVM, 30 s–2 min
  first check (scan §A/§E). It *must* be an LSP `DiagnosticsProvider` (reusing `lsp_rpc.rs` + the
  `harper_ls.rs` template), and its lifecycle must exploit exactly the mechanisms the seam already has:
  - **Lazy start on Review-entry, never on a keystroke.** `ensure_running` is already lazy and
    non-blocking (all trait methods just `cmd_tx.send`; the blocking `recv_timeout` lives entirely in the
    worker, `harper_ls.rs:716`). The JVM boot happens on the worker thread; the hot path never blocks.
  - **`Availability::Starting` as the honest "warming up" state.** The seam already has
    `{Idle, Starting, Ready, Unavailable}` (`diag_provider.rs:14`). ltex sits in `Starting` for 30 s–2 min;
    the status line shows "warming ltex…" (no-silent-UI); the writer keeps drafting; when it flips to
    `Ready` an accepted check lands via the normal `DiagnosticsDone` path. No new machinery — the state
    already exists, unused by the light harper.
  - **Idle-shutdown for free-at-rest** — the JVM must not sit resident forever. This is the one genuinely
    new lifecycle knob (harper is cheap enough to leave running). Options in ⚠OPEN 6 (idle-timeout vs
    keep-warm-within-session). Edge-triggered by leaving Review / N minutes of no-Review, never wall-clock
    polled — consistent with the swap-thrash-fix / idle-is-free conventions (`CLAUDE.md`).
  - It never violates free-at-rest *as long as it is not spawned until Review is entered and is shut down
    when the writer leaves the analysis lens* — the E7 "cost lands in the summoned view" principle applied
    to a much heavier tool.

- **vale — the precedent does NOT cleanly govern; a genuine fork.** Vale ships a **mature one-shot Go CLI**
  (run → JSON → exit, near-zero residency, scan §A/§E) *and* **vale-ls** (a Rust LSP wrapper that itself
  shells out to the `vale` CLI per check — two processes in the chain — but adds `installVale:true`
  self-install and a uniform LSP interface). Harper chose LSP because it needed *incremental sync* + the
  *dep-shed*; **vale is a full-document linter that needs neither** — so vale-CLI-one-shot is the near-free
  profile that only lost for harper for reasons that don't apply here. This is a real product/resource fork
  (⚠OPEN 1): vale-ls (interface uniformity via `lsp_rpc.rs`, best absent-binary UX via self-install) vs.
  vale-CLI-one-shot (fewer moving parts, zero residency, but a second lifecycle model in the seam OR a
  `wc.async` driver instead of a provider).

**Recommendation on the LSP-vs-one-shot axis:** **LSP-everywhere for the first-class bundled/core
providers** — harper-ls (resident), ltex-ls-plus (lazy JVM), and vale-ls — all through the *one* seam,
`lsp_rpc.rs`, the *one* staleness model (generation-stamped URIs), and the *one* client action layer
(`DiagOverlay`). Reasons: (a) the seam, transport, staleness guard, and action UX are all LSP-shaped
already; (b) adding a one-shot lifecycle to the *provider* seam means a second lifecycle model
(spawn-per-check vs resident) for marginal gain; (c) vale-ls's self-install is the best graceful-absence
UX. **Reserve `wc.async` (one-shot) for genuinely non-LSP tools** — formatters (`prettier`, `fmt`), and
**vale-the-CLI as the canonical `wc.async` driver** (a plugin running vale directly, the exact
"formatter/linter plugin" driver P3 scoped `wc.async` for, `effort-p3-grounding.md:166`). So the harper LSP
precedent is *upheld* for the provider seam, with the honest caveat that vale-CLI-one-shot is competitive
and is the natural proof-case for the separate `wc.async` effort. **`wc.async` belongs in a SEPARATE
effort** (§6-d): it has zero dependency on the multi-provider spine, and P3 already deferred it with its own
driver.

**Where `wc.async` fits mechanically (the `!Send` constraint governs the shape).** The `jobs.rs` substrate
(`Job { run: Box<dyn FnOnce() -> JobResult + Send> }`, `filter.rs`'s ad-hoc spawn) is the transport for the
*shell-out* — the worker runs the subprocess off-thread. BUT a plugin's completion callback is `mlua` Lua,
which is `!Send` and main-thread-confined (P1–P3's load-bearing invariant), so the completion **cannot** be
the `Job.merge` closure (which would have to touch Lua on/across the Send boundary). It must marshal back
exactly as harper's results do: a new `Msg::AsyncDone { plugin, token, output }` (Send: owned bytes) drained
on the main thread by the plugin **pump**, which then invokes the Lua `on_done` with the output. This is
P3's F1-option-A ("a CLOSED menu of Rust primitives via `wc.async{op, args, on_done}`; Lua completion on the
main thread") — the closed-primitive shape is forced by `!Send`, not chosen. So `wc.async` = a small Rust
op-menu (shell-out first) + a `Msg` + a pump drain + a resource/security posture (arg/output caps, a
timeout, no arbitrary-exec-without-consent question — ⚠OPEN 7).

---

## 3. Plugins' role — config surface, not process owner

The seam has **zero plugin coupling** today (`diag_provider.rs` imports nothing from `plugin/`; the
provider is a Rust `Box<dyn>` owned by `run()`). The answer to *"does Lua give more control over settings"*
is **where the config surface lives, not who spawns the process:**

- **The LSP client stays Rust.** A Lua-side LSP client is a non-starter: the mlua VM is `!Send` and
  main-thread-confined (P3), while an LSP client is fundamentally a *worker thread holding Send protocol
  state* (`harper_ls.rs`'s `"wcartel-harper-client"` + `"wcartel-harper-read"` threads). Lua cannot own
  that thread, and `wc.async`'s completion-marshaling (§2) exists precisely because Lua can't cross the
  Send boundary. Spawning/protocol/staleness = Rust, always.
- **The three core engines' config lives NATIVELY** in `[diagnostics.<engine>]` (extending
  `DiagnosticsConfig`), because harper/ltex/vale are *first-class bundled/core providers*, not plugins.
  ltex's ~30 keys and vale's `.vale.ini`/StylesPath ecosystem (scan §C) are large — but they are the
  engines' own config files (`.vale.ini` is read by vale directly, scan §B), so wordcartel's job is
  mostly *enable + point-at-config + a few knobs*, not re-expressing 30 keys.
- **Plugins' legitimate roles (later, optional):** (a) DECLARE/CONFIGURE a *4th, non-core* engine via
  `[plugins.config.<name>]` (a plugin bundling a linter it spawns via `wc.async`, or curating a friendlier
  subset of an engine's knobs), and (b) SURFACE actions — register commands (appearing in palette/menu)
  that `wc.command` the built-in diag commands. This is a *future extensibility layer* that couples to
  `wc.async` (§6-d) + the deferred dynamic-menu effort (§5), NOT part of the core multi-provider spine.

So: native config for the 3 core engines now; plugin-declares-a-server as a separate, later, wc.async-
dependent effort. (⚠OPEN 8: is any plugin-declared-server in the initial scope, or purely native?)

---

## 4. The viewing / action layer — the delta over the existing `DiagOverlay`

Given §0's reframe (the apply/ignore/add-dict spine + a suggestion-list overlay already ship), the net-new
is bounded:

**(i) Explanation / detail + doc-link.** `DiagOverlay` shows the `message` only as its title
(`render_overlays.rs:419`); harper is message-only (no link, ever — scan §B). For ltex/vale, add
`Diagnostic.href` (§1) and a **"Learn more" affordance**. Since the app owns the terminal (no browser),
"learn more" resolves to: copy the href to the clipboard (the existing clipboard seam) **or** show it in a
detail region. **Fork (⚠OPEN 2):** the detail region is a *net-new surface* — there is **no dock/split/
side-panel layer** anywhere (all list overlays route through the single centered
`chrome_geom::palette_overlay_rect`; "panel" in render only means an overlay's bg fill). Two options: **(a)
extend the existing centered `DiagOverlay`** with a scrollable message/detail region + a "learn more" row
(cheap, ships in this effort); **(b) build the Fresh `SplitRole::UtilityDock` content-agnostic leaf**
(`ux-backlog.md` S1: render-into-`&mut Buffer`, a tagged dock leaf, *not* a new `RenderMode`) — the recorded
"right" long-term model, but a whole *windowing* effort of its own. Recommend (a) for this effort; note (b)
is the S1 backlog item and the principled end-state.

**(ii) Multi-suggestion selector.** Mostly exists — `DiagOverlay` already renders `anchor.suggestions` as
rows with a `selected` index (`diag_overlay.rs:59-65`, applied at `search_ui.rs:182`). ltex/vale populate N
`suggestions` (via `lsp_rpc.rs`'s codeAction→`Suggestion` mapping); the overlay handles N unchanged. This
is a **populate-more-suggestions** task, not a new surface.

**(iii) Dictionary / add-word.** The *client-handled* mechanism exists for harper (`append_word_to_dict` +
`editor.dictionary` + `reload_dictionary`, `search_ui.rs:165-181`). Net-new = **per-engine writers**: ltex's
add-to-dictionary is client-handled to a *different* per-language file/format (scan §D-iii); vale's is
vocab-file editing. So each non-harper engine needs its own `append_word` target + a `reload`/re-sync
nudge. The *seam* addition is small (a per-provider `add_to_dictionary(word)` or an executeCommand relay);
the *writers* are per-engine.

**(iv) Rule/category disable.** harper = settings-level (flat bool table, no runtime command — scan §D-iv);
ltex = per-diagnostic **client-handled `_ltex.disableRules`** (write the rule id into the engine's config);
vale = `.vale.ini` editing. Net-new = a `disable_rule` action on `DiagOverlay` (visible only when
`Diagnostic.code` is present) that writes to the active engine's config + re-syncs. This needs
`Diagnostic.code` (§1).

**(v) executeCommand relay.** harper offers server commands (`HarperAddToUserDict`, `IgnoreLint`,
`RecordLint`); vale-ls offers `cli.sync`/`cli.compile` (scan §B). A generic LSP `workspace/executeCommand`
send (built on `lsp_rpc.rs`) lets the client relay these where the server *does* handle them — the
complement to the client-handled ones. Small seam addition (`provider.execute_command(name, args)`).

**Trap to honor (scan §B/§D-4):** **vale-ls hover/completion fire ONLY inside `.vale.ini`/style YAML, NOT
on prose.** Do NOT wire hover/completion for prose diagnostics — there is nothing to hover. Prose
diagnostics arrive via `publishDiagnostics` only; explanation comes from `message` + `href`, never a hover
round-trip. (wordcartel has no hover concept anyway — reinforces "don't add one for this.")

Every surface honors *no-silent-UI* (a `Starting`/`Degraded` engine shows a status hint, never a silent
empty view) + *instant-typing* (all provider methods are non-blocking `cmd_tx.send`; the overlay is
Review-only, off the drafting hot path).

---

## 5. Menus — per-engine management, and the dynamic-menu coupling question

Per-engine **enable/disable, "select analysis engine", "explain", "apply fix", "add to dictionary", "run
vale now"** should all be **commands** — the command-surface-contract requires it (every option is a
command; palette-exhaustive; menu ⊆ palette). Concretely:
- **Set-per-state + a cycle (contract law 8):** `analysis_engine_harper` / `_ltex` / `_vale`
  (palette-only set primitives) + an `analysis_next` cycle carried in the menu with state-in-label — exactly
  the `keymap_next` / `toggle_chrome` precedent. Per-engine `diag_enable_<engine>` toggles pair with the
  same shape.
- **A per-engine menu section.** The engines + their live state (Ready / warming / off / not-installed) are
  live editor state → a **dynamic menu section** fits (`DynamicSection { category, rows: fn(&Editor) ->
  Vec<(String, MenuRowAction)> }`, `menu.rs:46`). **Coupling answer:** a **builtin** dynamic section (a Rust
  `fn` pointer in `DYNAMIC_SECTIONS`, exactly like Documents → `workspace::documents_menu_rows`,
  `menu.rs:53`) works **TODAY, with zero coupling to the deferred plugin-dynamic-menu effort** — because
  `DYNAMIC_SECTIONS` is a `fn`-pointer table and a *builtin* engine-rows fn is just another `fn`, no closure
  needed. The coupling only appears if **plugins** contribute engine rows (which needs the boxed-closure /
  `MenuRowAction::Plugin` widening the P3 grounding scoped as a separate effort). So: **builtin engine
  management via a new `DYNAMIC_SECTIONS` row now (no coupling); plugin-contributed engine rows later
  (coupled).** New category candidate: reuse `View` (where the Review-mode + palette rows live) or add an
  `Analysis` `MenuCategory` — a small enum + `MENU_ORDER` addition (⚠OPEN, minor).
- Each new command derives its palette entry + live keymap hint for free (`reg.commands()` →
  `palette::rebuild_rows` / `menu::grouped_commands`); menu ⊆ palette holds by construction.

---

## 6. Everything else + decomposition

**Install / dependency posture.** harper stays a **bundled core provider** (an `optdepends` binary via
`$PATH`, like pandoc/xclip; `spawn_session` runs bare `Command::new("harper-ls")`, `harper_ls.rs:686`;
absence → `Availability::Unavailable` + `INSTALL_HINT`, `harper_ls.rs:719`). **ltex-ls-plus (300 MB JVM) and
vale/vale-ls are user-installed, never bundled** — the same graceful-absence path (spawn-Err →
`Unavailable` + a per-engine install hint). vale-ls's `installVale:true` self-install softens the vale
binary-absent case (scan §A/§E) — the best absent-binary UX, an argument for the vale-ls path (§2). Each
engine's `INSTALL_HINT` is per-engine ("install ltex-ls-plus (needs Java 21+)"). **⚠OPEN 4/5** cover the
support posture + the default engine.

**Staleness / version across N async providers.** The existing model generalizes cleanly: harper already
generation-stamps URIs (`doc_uri(buffer_id, generation)`) and version-guards every publish
(`on_publish` drops on `generation` / `version` mismatch, `harper_ls.rs:356`), and the apply-side
`apply_diagnostics_done` stores only if `b.document.version == version` (`diagnostics_run.rs:160`). With N
providers, each latches its OWN `in_flight_version` and each result is version-checked independently — a
slow ltex result for an old version is dropped by its own guard while a fast harper result for the current
version applies. `DiagStore.valid_for(version)` (`diagnostics_run.rs:18`) gates painting per source. No new
staleness *mechanism* — just per-source instances of the existing one.

**Is this ONE effort or several? Several, with a clear spine.** Candidates + dependencies + rough sizes:

- **(a) SPINE — generalize the provider seam single→multi.** `Diagnostic` gains `source`/`code`/`href`
  (wordcartel-core); `diag_provider` → `Vec`/registry; `DiagStore` source-partitioned; `DiagnosticsDone`
  gains `source`; `dispatch_diagnostics` fan-out; per-source view/nav + the `active_analysis_source` +
  its set-per-state/cycle commands; native `[diagnostics.<engine>]` config. **Depends on nothing.** Ships
  the "provider #2 insurance" realized — and can land with a single cheap 2nd engine (vale-ls) to prove it.
  **Size: medium-large** (the core-type change is the review-magnet; the seam/store/render generalization is
  mechanical but broad). This is the effort that must go first and could ship alone.
- **(b) The ltex-ls-plus provider + its JVM lifecycle** (lazy-on-Review, `Starting`/warming, idle-shutdown,
  never-block) + (optionally) the vale-ls provider. Each = a new `DiagnosticsProvider` impl reusing
  `lsp_rpc.rs` + the `harper_ls.rs` template. **Depends on (a).** **Size: medium each; ltex's JVM lifecycle
  (warming/idle-shutdown) is the only genuinely new risk.**
- **(c) The viewing/action delta** — `href`/"learn more" + detail region on `DiagOverlay`; per-engine
  client-handled dict/rule writers; the executeCommand relay; more-suggestions population. **Depends on (a)**
  (needs the new `Diagnostic` fields). **Size: medium.** Parallelizable with (b).
- **(d) `wc.async` one-shot primitive** — a closed Rust op-menu (shell-out first) + `Msg::AsyncDone` + pump
  drain + resource/security caps, with a formatter or vale-CLI driver. **Depends only on the shipped plugin
  system — NOT on (a)/(b)/(c).** Fully independent; can ship any time. **Size: small-medium.**
- **(e) plugin-declares-a-server + plugin-contributed dynamic menu rows.** **Depends on (a) + (d) + the
  deferred dynamic-menu-section effort.** **Size: medium.** Last, optional.

**Recommended ordering:** **(a) alone first** (the generalization, proven with vale-ls as the cheap 2nd
provider). Then **(b-ltex) + (c) as a second effort** (the heavy JVM outlier + the richer viewing/action
layer, parallelizable). **(d) `wc.async` independently**, whenever a one-shot-tool driver (formatter, or
vale-CLI-as-a-plugin) motivates it. **(e) last**, if plugin-authored engines ever materialize. This keeps
each effort's blast radius bounded, lands the user-visible multi-engine value at the end of (a)+(b), and
never blocks the spine on the deferred dynamic-menu or plugin-server work.

---

## ⚠ Human-only product decisions (surfaced, not decided)

1. **vale: vale-ls provider vs. vale-CLI-one-shot.** vale-ls (uniform LSP via `lsp_rpc.rs`, self-install
   for best absent-binary UX, but 2 processes + residency) vs. vale-CLI (near-free one-shot, fewer moving
   parts, but a 2nd lifecycle model in the seam OR a `wc.async` driver instead of a provider). The harper
   LSP precedent does not cleanly govern here (vale needs no incremental sync + no dep-shed). *Recommend
   vale-ls as the provider; vale-CLI as the `wc.async` driver.* — a genuine resource/product fork.
2. **Detail/explanation panel: extend the centered `DiagOverlay` vs. build the Fresh `UtilityDock`
   side-panel.** Extend-overlay ships in-effort and is cheap; the dock is the recorded principled end-state
   (`ux-backlog.md` S1) but is a separate *windowing* effort. *Recommend extend-overlay now; dock later.*
3. **The "separate per-engine views" INTERACTION model.** The locked decision (never merge) leaves *how the
   writer sees/switches engines* open: (a) one active engine's view at a time (a switchable lens, like
   `RenderMode`); (b) all engines' diagnostics rendered simultaneously with *per-engine distinct underline
   styles* (distinct notations, not merged); (c) a stacked list grouped by engine. The Neovim pain the user
   lived was *merged competing notations* — (b) is close to that line and needs a product call on whether
   distinct-simultaneous-styles is acceptable or is the same pain. *Recommend (a) — a switchable lens — as
   the cleanest "separate views."*
4. **Install/support posture for ltex (300 MB JVM) + vale.** Documented `optdepends` we support + hint the
   user to install, vs. fully BYO with a bare "engine unavailable" degrade. Affects the graceful-absence
   copy and the support surface.
5. **Default Analysis engine when multiple are enabled.** harper (bundled, grammar-first identity —
   `CLAUDE.md`) is the natural default; but a writer may want ltex-for-a-language-pass as their default.
   Which engine the Review lens shows first is a product call.
6. **JVM idle-shutdown policy for ltex.** Shut down after N minutes idle (strict free-at-rest, pays the
   30 s–2 min re-warm on the next Review) vs. keep warm within a session (fast re-entry, a resident JVM at
   rest). A resource-vs-latency tradeoff unique to the heavy outlier.
7. **`wc.async` exec posture.** A closed op-menu (shell-out only, capped args/output, a timeout) under the
   trusted-plugin posture — but shelling out to an arbitrary subprocess is a broader capability than any P1–
   P3 primitive. Is arbitrary shell-out allowed for a trusted plugin without an additional consent gate, and
   what are the arg/output/time caps? (Resource-bound + no-silent-UI apply; the security posture is the call.)
8. **Is any plugin-declared-server in the initial scope, or purely native config for the 3 core engines?**
   *Recommend native for the core three; plugin-declared-server deferred to effort (e).*
