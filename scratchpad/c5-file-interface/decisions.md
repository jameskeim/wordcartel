# C5 — resolved decisions (human-ratified)

## Forks 1–4 (resolved during brainstorming, unchanged)

- **F1 — browsing UI model: C.** Picker for C5's transactional surfaces; the collapsible tree is a
  binder problem and lands in **S2 (directory-as-binder)**. Do not build a tree in C5.
- **F2 — chokepoint width: C.** Full unification of all *in-process* filesystem access.
  Permanent out-of-scope set, stated explicitly in the spec: the `!` shell filter
  (`filter::run_subprocess`, arbitrary user commands, vi/Emacs semantics by design), harper-ls's own
  `userDictPath` access (out of process), and pandoc's own `-o` writes (subprocess owns its output).
- **F3 — filter model: A**, two orthogonal toggles (clutter / file type), with the disclosure
  principle: **the filter never silently lies** — a footer count whenever entries are withheld.
  AMENDED by decision 2 below (clutter loses gitignore) and by Fable's mode-aware documents
  definition (the "pandoc-ingestible" rationale was an error — there is no import path;
  `file::open` refuses `.docx` as `OpenError::Binary`).
- **F4 — extension policy: A**, default-and-redirect. Missing extension → append `.md`; recognized
  OUTPUT extension (`.docx`/`.pdf`/`.html`/`.tex`) → refuse, explain, offer Export (which now has a
  destination); any other extension → honored silently.

## The six post-report decisions — human took Fable's position on ALL SIX (2026-07-18)

1. **F5 — contract + mint-and-stamp rider.** ADOPTED IN FULL.
   - Swap file naming stays **path-derived, permanently**; the spec records why (recovery answers a
     path-shaped question, so a path-derived key is semantically correct, not a compromise). The
     path-aware latch in `dispatch_swap_write`'s merge is untouched;
     `stale_path_swap_does_not_relatch_after_rekey` stays a merge gate.
   - **Identity contract (binding):** any NEW per-document persistent state — S3 snapshots foremost —
     keys on a `DocumentId`, never on a path. Paths in persistent records are display/forensic hints.
   - **Rider:** mint a `DocumentId` (128-bit random, hex) at first durable association; carry it on
     `Document`; stamp it into the session `StateEntry` (defaulted serde field) and the swap header
     (`id:` line — `swap::parse` ignores unknown keys, verified, so this is forward- AND
     backward-compatible). **Nothing reads it yet.**
   - **Semantics:** the id is a **lineage hint, not a uniqueness invariant** (mirroring "path is not
     a uniqueness invariant" — the workspace permits the same path in multiple buffers). C5's
     position: the id follows the buffer through Save-As; a stay-behind buffer at the old path
     re-mints on next touch. Divergence lineage beyond that is **S3's to answer**, recorded as such.
   - Rationale correcting the controller: the controller argued a stable identity implies a path→id
     index on disk, hence a SPOF for crash recovery. That leap only follows if the swap *filename*
     is rekeyed on the id, which nobody needs. And the SPOF logic **inverts** for snapshots:
     path-keyed snapshots fail in ordinary operation (every Save-As orphans the history), while
     id-keyed snapshots degrade gracefully (listable and reassociable via per-snapshot
     id/realpath/content-hash headers).

2. **Drop gitignore matching and the `ignore` crate from C5.** ADOPTED.
   - Clutter toggle = **dotfiles + VCS/system directory names only** (a dozen lines over `read_dir`
     for one-level listing).
   - **C5 adds ZERO new dependencies.** Fuzzy filtering uses the existing `palette::fuzzy_filter`
     (nucleo-matcher, already a direct dep).
   - Gitignore-aware recursive walking moves to **S2**, where recursion makes `ignore` earn its place.
   - This deliberately amends the already-accepted F3-A definition; the human called it.

3. **Recents IN, favorites DEFERRED.** ADOPTED. `open_recent` command sourced from
   `SessionState.entries` ranked by `seq` (already an LRU-ranked, canonical-path-keyed map).
   Favorites/pinned dirs (the backlog item's "Layer 2") are **explicitly deferred** — the spec must
   say so, because the C5 backlog item names both.

4. **Export Enter-through.** ADOPTED. The export destination picker opens **pre-seeded** with
   `derived_export_path` (field) and the source directory (dir), so plain Enter reproduces today's
   zero-decision behavior exactly. Destination *choice* is new capability; destination *obligation*
   would be a regression. `pending_export` overwrite + TOCTOU re-check wiring unchanged.

5. **Cut destination-mode "New".** ADOPTED. `new` stays purely in-memory; first-save-names-it via
   the destination picker covers the scenario (the GUI-editor model).

6. **Diverged orphans — visibility only.** ADOPTED. Do **NOT** extend the `clean_recovery` sweep to
   diverged orphans: `swap_is_cleanable` fails closed on `RecoveryDecision::Prompt` precisely because
   a diverged swap holds content not on disk at its recorded realpath, and
   `swap_is_cleanable_only_for_valueless_dead_pid_swaps` asserts it. Surface the
   kept-because-recoverable count/listing in the modal instead; let the human extract or explicitly
   discard. Severable to a follow-up under size pressure.

## Corrections to the grounding packet that the spec must NOT repeat

- **`Buffer.pending_swap_path` is NOT dormant.** It is populated in `app.rs` startup recovery when
  `find_orphan_scratch_swap()` stages an orphan scratch swap, and consumed by `prompts::resolve_prompt`
  in the `Recover` / `DiscardSwap` / `OpenOriginal` arms (test:
  `recover_loads_body_and_deletes_orphan_swap_file`). It is the orphan-scratch recovery carrier.
  Removing or repurposing it breaks Recover.
- **Session-entry stranding is hygiene, not a durability gap.** `persist_session` is not exit-only —
  the main loop persists whenever `saved_version` advances (`sv != last_persisted_saved` in
  `app::run`), so the new path gets an entry one loop tick after the Save-As merge. The stranded old
  entry still matches the old file's mtime+size identity, so restoring it on reopening that file is
  arguably correct. Fix it (the merge already captures `prior_key`, so it is nearly free) but
  describe it honestly.
- **`SessionState.entries` is a `BTreeMap<String, StateEntry>`, not a `HashMap`.**
- **Listing must NOT ride the `jobs.rs` FIFO.** `ThreadExecutor` is one worker shared with
  `JobKind::Save` and `JobKind::SwapWrite`; a listing blocked on a hung mount would queue AHEAD of
  the user's saves and swap writes, converting a browsing hiccup into a durability outage. Use a
  dedicated short-lived thread (the `export::do_export` precedent: `std::thread::spawn` + typed
  `Msg` + epoch/version check at merge). The overlay must stay closable while a listing is in flight.
- **`rebuild_entries` re-runs `read_dir` on every query keystroke**, not just on open. Caching the
  listing per directory and filtering in memory is most of the responsiveness fix, independent of
  threading.
- **tokio is a stale `Cargo.lock` entry** — `cargo tree -i tokio --target all` finds no path on any
  target; the release binary has 0 tokio symbols. It does NOT ride in under `harper-brill`/`burn`.

## Design commitments from Fable's IMAGINE pass (carry into the spec)

- **The resolved absolute target is always visible before commit** — the picker footer renders, live,
  `→ /home/km/drafts/chapter-one.md` (post extension policy) on every keystroke. On completion the
  status **names the full path** (fixes the verified gap where Save-As reports a bare "Saved").
- **Destination mode = one text field, dual duty** — its content is simultaneously the
  filename-to-be AND a live filter on the listing (typing `chap` reveals existing chapter files:
  overwrite awareness for free). Nav keys move the selection highlight only. **Enter with a
  directory highlighted descends; Enter otherwise commits `dir + field`** (through extension policy,
  then the existing overwrite-confirm). Selecting an existing **file copies its name into the
  field** — explicit overwrite intent, two keystrokes, never accidental. Field text that exactly
  names a directory **resolves toward descend**.
- **Documents filter is mode-aware:** open mode lists what `file::open` can actually open (text
  formats); destination mode also shows output-format siblings (overwrite awareness).
- **Listing responsiveness, three layers, simplest first:** (1) per-directory listing cache + in-memory
  filtering; (2) entry cap with visible disclosure ("showing 5,000 of 38,412"); (3) `read_dir` on
  open/descend on a dedicated thread with epoch-checked merge and a "Listing…" status if slow.
  Helix's 30ms-inline-then-thread refinement is nice-to-have on top; the epoch check and the
  not-on-the-durability-queue rule are load-bearing.
- **`Fs` grows a small, CLOSED set of primitives** — bounded read, capped list, stat, plus actually
  using the declared `rename`/`remove_file` — not a sprawling god-trait.
- **Quit-drain coupling must survive the Save-As migration** — `dispatch_save_then` arms
  `pending_save_as` by checking `minibuffer.kind == SaveAs`; the drain-abort behavior in
  `save_as_submit`'s empty-path arm must be preserved. Guard tests:
  `save_and_quit_on_unnamed_buffer_does_not_arm_pending_after_save` and the Effort-6 Codex-C2
  empty-path arm.

## Post-spec decisions (human-ratified 2026-07-18, round 2)

7. **Navigation depth: ONE LEVEL.** Confirmed — closes the spec's §16 open fork. The picker
   navigates one directory level at a time with fuzzy filtering within the level. Recursive
   project-wide find is NOT in C5: it would want the `ignore` crate that decision 2 removed, S2 owns
   traversal, and `open_recent` covers the realistic lost-file scenario. The acknowledged cost,
   recorded honestly: a writer with a deep hierarchy looking for a file never opened in wcartel has
   no fast path until S2.
8. **Filter toggles ARE persisted settings.** Ratifies the spec's §17 deviation. They join
   `SettingsSnapshot` and the overrides serde mirror — which is what puts them under the
   compile-time reachability gate (`every_persisted_setting_has_a_command`, contract law 2). A
   toggle that reset on every launch would read as a bug.
9. **Click divergence between modes KEPT.** Ratifies the spec's §17 deviation. In select mode a
   click selects AND commits (today's behavior, unchanged). In destination mode a click does NOT
   commit. This is a deliberate inconsistency within one overlay, justified by stakes: in a
   destination context click-to-commit means one stray click can overwrite a file — the exact harm
   class C5 exists to eliminate.

## Symlink handling (human-ratified 2026-07-18, round 3)

10. **Symlinked DIRECTORIES resolve and behave as directories.** Fixes a verified defect:
    `file_browser::rebuild_entries` classifies via `DirEntry::file_type()`, which does NOT follow
    symlinks, so a symlink-to-directory got `is_dir == false` — sorted as a file, rendered without
    its trailing `/`, and Enter routed it to `file::open` → `OpenError::IsDir`. THREE consumers of
    `FileEntry::is_dir` were affected (sort, Enter dispatch, and `render_overlays::paint_file_browser`'s
    label). Broken symlinks are listed and visibly marked, never silently hidden, and refuse to
    open with a clear status. A symlink marker renders in the listing. NO loop protection (one-level
    navigation makes cycles a deliberate act and `..` escapes) — but S2 MUST revisit if it adds
    recursive traversal. Syscall economy is load-bearing: `d_type` already carries the symlink bit
    free, so only actual symlinks cost a `metadata()` — a counting-`Fs` test guards against a later
    "simplification" to stat-everything.

11. **Symlinked FILES: MIDDLE B — `Document.path` stays AS-OPENED; resolve only at the write
    boundaries.** (Human initially chose Middle A; Fable objected with evidence, the controller
    agreed the objection was correct, and the human switched.)
    - The verified defect: `file::save_atomic` refuses to write through a symlink
      (`SaveError::Symlink`), so symlinked files are **openable but unsaveable**. The refusal is
      CORRECT and stays unmodified — `atomic_replace` renames a temp over the target, which would
      replace the symlink with a regular file and destroy the link. Resolution happens BEFORE the
      refusal, so `save_through_symlink_refused` stays green unmodified.
    - **Why Middle B beat Middle A** (record this reasoning; it is the general rule):
      (a) `derived_export_path` off a canonical path would silently drop exports into a directory
      the writer has never opened — the exact harm class C5 exists to eliminate.
      (b) The consistency Middle A claimed to gain ALREADY EXISTS: `swap::swap_path`,
      `swap::build_header`, and `session_restore` each call `canonicalize` at their own point of
      use. They never needed `Document.path` canonical. The controller's premise was false.
      (c) Middle B is strictly less machinery — the write-boundary resolver is needed under EITHER
      option (destinations are not `Document.path`), so Middle A = that resolver PLUS a display
      field PLUS seven consumer changes.
      (d) It IS the rule the spec already establishes and tests — **navigation and display are
      logical; durability is resolved; each at its point of use.** Middle A carved an exception.
    - The seven `Document.path` consumers that keep today's correct behavior under Middle B:
      `workspace::buffer_display_name`, `prompts::open_save_as` prefill, `blocks_marked::block_write`
      prefill, the `"open"` command's dir seed, `export::run_export` → `derived_export_path`,
      `plugin::api` `wc.path()` + 4 event payloads, and `diagnostics_run`'s LSP URI.
    - **Destination-mode targets resolve too** — a destination that is itself a symlink to an
      existing file must resolve before dispatch, and the overwrite-confirm names the RESOLVED
      target. Broken-symlink destinations are refused before dispatch, never falling through to a
      confusing `SaveError`.
    - The full-path completion status names the **resolved write target** (same answer under either
      option).

## Plugin discovery (human-ratified 2026-07-18, round 4)

12. **Plugin discovery FOLLOWS symlinks (Option A).** A symlinked `.lua` file or plugin directory in
    the plugins directory is discovered and loaded.
    - **The fact that decided it:** `plugin::load::discover` is ALREADY internally inconsistent.
      It classifies with `entry.file_type()` (does NOT follow), so a symlinked `.lua` or plugin dir
      is silently ignored — neither `is_file` nor `is_dir`, so it falls through both arms without
      even reaching the `skipped` report. But the nested `init.is_file()` probe is `Path::is_file()`,
      which DOES follow. So today a real directory whose `init.lua` is a symlink loads, while a
      symlinked directory containing a real `init.lua` does not. The trust boundary is not intact
      now; it is inconsistently enforced.
    - **The security argument, stated fairly and rejected:** a plugin is executable Lua running
      in-process, so following means the executed bytes live outside the curated directory and could
      change without the user touching it. Rejected because the boundary already leaks via
      `init.lua`, and because creating a symlink into one's own plugins directory is itself a
      deliberate act of installation.
    - **Two riders, both required:** (a) a broken symlink in the plugins directory lands in
      `discover`'s existing `skipped` report — named, never silently dropped, matching that
      function's stated contract; (b) the `.lua` arm gates on `is_file`, NOT `!is_dir`, so a fifo
      named `x.lua` cannot become a candidate.
    - This is a deliberate behavior change, not a refactor consequence. Recorded as such.

## Discovered during spec authoring (adds work, no decision needed)

- **The seam's callers hardcode `&crate::fsx::RealFs`** at the call site rather than accepting
  `&dyn Fs`, so `file::save_atomic` and its siblings are NOT themselves fault-injectable — only
  `atomic_replace` is. F2-C requires a **signature convention**, not merely new trait methods: the
  `*_with_fs` split, following the house pattern already used by `find_orphan_scratch_swap`/`_in`
  and `state::load`/`load_in`.
- **`FaultFs` is private to `fsx.rs`'s test module** and must be promoted to `test_support.rs`
  before any other module's call sites can be fault-tested. This is a **prerequisite task**, not a
  side effect.

## Size

Medium-plus, ~15 tasks, A17-class — clearly smaller than H1. Highest-risk: destination-mode commit
semantics and its quit-drain integration (the only places a design error produces the exact harm
class C5 exists to eliminate — silent overwrite, save-to-nowhere, hung quit).
