# C5 — Unified file interface (one chokepoint, one picker) — Design spec

Status: DRAFT (awaiting Codex spec gate)
Author: Fable (independent grounding + authoring)
Date: 2026-07-18
Backlog item: **C5** — "File interface — unify save/write onto the picker + favorites/recent"
Decisions of record: `scratchpad/c5-file-interface/decisions.md` (human-ratified 2026-07-18;
decisions 1–6 pre-spec, decisions 7–9 ratified against this spec's §16/§17)
Relates forward to: **S2** (directory-as-binder — owns the tree + recursive/gitignore-aware walking),
**S3** (snapshots — inherits the identity contract in §12).

---

## 1. Summary

C5 does two things that are usually described as one effort and are in fact two:

1. **One chokepoint — for four named operation classes.** Every in-process **file-content read,
   directory listing, stat/existence probe, and durable write or app-artifact rename/delete** routes
   through the `fsx::Fs` seam, which today covers exactly one operation (`atomic_replace`) and is
   write-only. `Fs` grows a small, closed set of primitives (bounded read, capped listing, stat) plus
   the two it already declares but that call sites bypass (`rename`, `remove_file`). The payoff is
   that the fault-injection story extends to reads and listings, and that config-class reads come
   under cap discipline.

   **It is deliberately not "all filesystem access."** Directory provisioning, `canonicalize`,
   `RealFs`'s own primitives, subprocess-owned IO, and the `/proc` liveness probe stay raw. §2.3
   states this as a **rule with exemption clauses** — a predicate any call site can be classified
   against without consulting a list — and backs it with a guard test
   (`wordcartel/tests/fs_chokepoint.rs`) that fails the build on an ordinarily-written new raw call.
   The scope claim rests on a rule plus a high-coverage detector, not on a curated inventory — earlier
   drafts tried the inventory and it drifted three times before implementation had even started. §2.3
   states the detector's known gaps rather than claiming it is airtight.

2. **One picker.** The `FileBrowser` overlay — today reachable only from `open` — gains a
   **destination mode** and becomes the single UI for choosing a file path: Open, Save-As,
   Write-Block, and (for the first time) Export. The typed-path minibuffer for save destinations
   retires. A `open_recent` command gives a writer who cannot find their file a way back to it.

It also fixes **two live, user-reported, source-verified defects** that fall squarely in this
territory, both hitting a writer who structures their directories with symlinks:

- **Symlinked directories are unusable** (§4.9) — classified as files, so they cannot be entered, and
  the error contradicts what the UI showed. Fixed at the seam (§5.2) so the picker and every future
  consumer inherit one correct answer; the listing gains symlink and broken-link markers (§7.5).
- **Symlinked files are openable but unsaveable** (§4.10) — they open fine, then every save is
  refused, so the writer cannot save at all. Fixed by resolving write destinations before they reach
  the (correct, unchanged) symlink guard (§7.6).

The writer-facing thesis: a person who is not confident at a terminal should never have to *type* a
path, and should always **see the resolved absolute target before they commit** and **be told the
full path afterward**. Every design choice below serves that.

Non-negotiables preserved: swap file naming stays path-derived; the path-aware swap latch is
untouched; the `clean_recovery` enumerator's fail-closed law is untouched; listing never rides the
durability job queue; C5 adds **zero new dependencies**.

---

## 2. Scope

### 2.1 In scope

- `Fs` seam extension (bounded read, capped list, stat) + `RealFs` impls + a shared fault-injection
  test seam (§5).
- Migration of in-process FS call sites onto the seam, including the three known bypasses (§5.3).
- Config-class read caps (§5.4).
- Listing pipeline: per-directory cache, entry cap with disclosure, off-thread `read_dir` (§6).
- `FileBrowser` select mode / destination mode, filters, and commit semantics (§7).
- **Bug fix: symlinked-directory classification** (§4.9 → §7.5) — symlinked directories are currently
  unusable in the picker. Fixed at the seam so both the picker and any future consumer inherit it.
- **Bug fix: symlinked files are unsaveable** (§4.10 → §7.6) — write destinations resolve through
  symlinks, so a symlinked document can be saved and the link is preserved.
- Extension policy for save destinations (§8).
- Export destination picker, Enter-through preserving today's behavior (§9).
- `open_recent` (§10).
- Save-As durability epilogue: session-entry migration, full-path completion status (§11.1).
- Diverged-orphan **visibility** in the `clean_recovery` modal (§11.3).
- F5: identity contract + `DocumentId` mint-and-stamp rider (§12).
- Command-surface conformance (§13).

### 2.2 Out of scope for C5, deferred to a named home

- **Collapsible tree / binder view** → **S2** (fork F1-C).
- **Gitignore-aware and recursive walking, and the `ignore` crate** → **S2**. C5's clutter filter is
  dotfiles + VCS/system directory names only, which one-level listing can do over `read_dir` in a
  dozen lines (decision 2).
- **Favorites / pinned directories** — the C5 backlog item's "Layer 2". Explicitly deferred
  (decision 3). Recents ship; favorites do not. This spec records the deferral because the backlog
  item names both and a reader will otherwise think it was missed.
- **Destination-mode "New"** — `new` stays purely in-memory (decision 5); first-save-names-it via the
  destination picker covers the scenario.
- **Import of non-text formats.** There is no import path in wcartel and C5 does not add one;
  `file::open` refuses binary/invalid-UTF-8 via `OpenError::Binary`.

### 2.3 Scope of the chokepoint — a rule, exemption clauses, and a guard test

The claim in §1 is scoped to **four operation classes**. This section defines that scope as a
**predicate plus exemption clauses**, enforced by a guard test. It is not an inventory of call sites,
and nothing in this document is — see the note immediately below for why.

**Why this section is a rule and not a list.** Earlier drafts of this spec defined scope by
enumerating call sites. That failed three times in review — once as an overbroad claim, once as two
enumerations contradicting each other, once as a census missing an entire operation class — and it
would have failed again during implementation, because a hand-curated inventory of call sites decays
the moment anyone adds one. So scope is defined by a **predicate any reader can apply to any call
site without consulting a list**, and enforced by a test rather than by prose.

#### The rule

> **In scope:** a call in **production code** (not `#[cfg(test)]`, not `tests/`, not `e2e.rs`
> fixtures) in the **`wordcartel` shell crate** that performs one of these **four operation classes**
> against a filesystem path:
>
> 1. **read file content**
> 2. **enumerate a directory**
> 3. **probe metadata** — existence, type, size, mtime, mode. The `Path` methods in this class are
>    `metadata`, `symlink_metadata`, `canonicalize`, `read_link`, `read_dir`, `exists`, `try_exists`,
>    `is_file`, `is_dir`, `is_symlink` — the same closed std-defined set layer 3 enumerates, listed
>    identically in both places on purpose so they cannot drift apart. (`canonicalize` and `read_dir`
>    appear here as *operations of this shape*; `canonicalize` is separately exempt under clause (c)
>    and `read_dir` belongs to class 2.)
> 4. **mutate durably** — write, rename, or delete
>
> …routes through the `fsx::Fs` seam.

`wordcartel-core` and `wordcartel-nlp` are out by construction: neither contains any `std::fs` usage,
which is what makes the shell crate the whole surface.

#### Exemption clauses

A call matching the rule is nevertheless out of scope if and only if it falls under one of these.
Each clause is a category, not a site, so new code is classified by reading them:

- **(a) Subprocess-owned IO.** Anything a child process does to its own files. Covers the `!` shell
  filter (`filter::run_subprocess` — the user's arbitrary `sh -c` command, where unconstrained file
  access is the *feature*, per vi `!` semantics), harper-ls's own `userDictPath` handling, and
  pandoc's `-o` writes. We control what we ask for, not what the child does. C5 does bring the
  *subsequent* rename of pandoc's output under the seam (§5.3) — that rename is ours.
- **(b) Directory provisioning.** `create_dir_all`, `set_permissions`, and the existence probe that
  guards them, when they exist to bring a directory into being rather than to inspect user data.
  Directory creation is not one of the four classes. This also avoids inverting a dependency:
  `swap::state_dir` provisions the directory the seam's own artifacts live in.
- **(c) Pure path resolution.** `canonicalize` — a name computation that consults the filesystem, not
  an operation on content. §5.5 explains why routing it would perturb swap naming for no testable
  gain.
- **(d) The seam's own implementation.** `fsx::RealFs` method bodies. Routing them through themselves
  is circular.
- **(e) Path syntax used for something that is not a file.** `swap::pid_is_live`'s `/proc/{pid}`
  probe is a process-liveness question wearing a path.
- **(f) Test code.** Excluded by the rule itself; restated here because the enforcement test must
  strip it.

Anything matching the rule and matching **no clause** is in scope. That is the whole definition. A
call site added next year is classified by reading the above — no list to consult, nothing to keep in
sync.

#### Enforcement — a guard test, not a promise

Prose scope claims decay; this one decayed three times in three rounds. So the rule gets a test in the
spirit of `wordcartel/tests/module_budgets.rs`, which is the project's existing answer to exactly this
problem (an invariant that erodes unless something fails the build when it does):

`wordcartel/tests/fs_chokepoint.rs` scans production sources under `wordcartel/src/` and fails on any
raw filesystem access not in an allow-list. **Each allow-list entry cites the exemption clause it
claims**, so the file reads as "here is each known raw FS call and why it is legitimate."

**Gate the import, not just the call — a call-token scan alone would be unsound.** The tree already
contains `use std::fs;` in both `file.rs` and `fsx.rs`, and `fsx.rs` already calls `fs::rename`,
`fs::remove_file`, and `fs::metadata` through that alias. A scanner looking for `std::fs::` would not
see any of them, so a new production `fs::write` or `fs::read_to_string` would sail past and the
claim "a missed site fails the moment it is introduced" would be false. The scan therefore has three
layers:

1. **Import gate (for `std::fs` reached through an import).** No production module may bring `std::fs`
   into scope unless allow-listed.

   **Caught** — these spellings are detected:
   - `use std::fs;` — the module.
   - `use std::fs::File;` / `use std::fs::OpenOptions;` — a **type** rather than the module. Caught by
     the `use std::fs` prefix, so a later bare `File::open(…)` or `OpenOptions::new()` is gated by the
     import even though the call site names neither `fs` nor `std`. Called out so the implementer does
     not anchor the pattern on `use std::fs;` *with a terminating semicolon*, which would miss every
     type import.
   - `use std::fs as alias;` — same prefix.
   - `use std::{fs, io};` — flat grouped. **Needs its own pattern**: the literal `use std::fs` never
     appears, so prefix matching alone misses it.

   **Not caught — known gaps, stated plainly:**
   - **Nested grouped imports** — `use std::{fs::{self as filesystem, OpenOptions as OO}};`
   - **Renamed imports inside a group** — `use std::{fs::File as StdFile};`
   - **Leading-root paths** — `use ::std::fs;`

   Closing these properly requires parsing the `use` tree, which means a dev-dependency and a small
   Rust parser. That was weighed and declined (see the forward note below), so the layer is specified
   with its limits rather than with a claim it cannot meet. **What this layer is for:** catching the
   realistic regression — someone mid-effort reaches for `fs::read_to_string` and writes an ordinary
   `use std::fs;`. It does **not** stop deliberate circumvention, and the gap spellings are ones
   nobody writes by accident.
2. **Fully-qualified call tokens.** `std::fs::…` and fully-qualified `OpenOptions` — the routes that
   need no import.
3. **Inherent `Path` methods — a closed, std-defined set.** This layer must be an enumeration, and
   that is acceptable *here* for a reason that does not apply to our own call sites: these methods
   need no import (nothing to gate) and emit no `std::fs::` token, but **the set is defined by the
   standard library, not by this codebase, so it does not drift as the tree changes.** Enumerated
   completely, the layer is sound permanently.

   The complete set of filesystem-touching inherent methods on `std::path::Path` as of current std:

   `metadata`, `symlink_metadata`, `canonicalize`, `read_link`, `read_dir`, `exists`, `try_exists`,
   `is_file`, `is_dir`, `is_symlink`

   **Both call syntaxes are matched, not just the dot-call.** Each of these can be written UFCS —
   `Path::metadata(p)` or `std::path::Path::exists(p)` — which a `.method(` scan misses entirely. Since
   the method set is already an explicit enumeration, covering UFCS costs one extra pattern per name:
   match `Path::<method>(` alongside `.<method>(` over the same closed set. That is cheap enough that
   disclosing UFCS as a gap would be the wrong trade, so it is **caught**, not listed under the
   not-caught set.

   `PathBuf` derefs to `Path`, so the same tokens cover it.

   #### Plugin discovery follows symlinks (decision 12) — and the silent-drop audit

**What it does today**, verified: `plugin::load::discover` classifies with `entry.file_type()`, which
does **not** follow symlinks — `file_type.is_file()` gates single-file `<name>.lua` plugins and
`file_type.is_dir()` gates `<name>/init.lua` plugins. A symlink to a `.lua` file, or to a plugin
directory, is neither, so it **falls off the end of the loop body entirely** — not loaded, and not
reported in `skipped`.

**Why following is defensible**, and this is the load-bearing part for a future reader: the trust
boundary is **already porous**. The nested `init.is_file()` probe is `Path::is_file()`, which *does*
follow. So today a real directory whose `init.lua` is a symlink loads, while a symlinked directory
containing a real `init.lua` does not — an inconsistency inside one function. Option B would have been
defending a boundary that is not intact. The security argument (executable Lua entering from outside
the curated directory) was weighed and outweighed: the user must deliberately create the link, and
that is itself an act of installation. **Recorded as a deliberate behavior change so nobody later
reads it as a refactor consequence.**

**Rider 1 — broken symlinks are reported.** A broken symlink in the plugins directory lands in
`discover`'s existing `skipped` report, named. This also fixes a small pre-existing bug in its own
right: today such an entry vanishes without appearing anywhere, contradicting the function's stated
contract.

**Rider 2 — the `.lua` arm matches `kind == File`, never "not a directory"**, so a fifo, socket, or
device named `x.lua` (`kind == Other`) cannot become a candidate. `EntryKind` exists for exactly this
distinction.

**Rider 3 — the silent-drop audit, because following does NOT close it by itself.** `discover`'s
contract says a found candidate is "named, never silently dropped." Enumerating every path out of the
loop shows following symlinks closes the biggest class but leaves others:

| Entry | Today | After decision 12 |
|---|---|---|
| Symlink → `.lua` file, or → plugin dir | silently dropped | **loaded** (resolved `kind == File` / `Dir`) |
| Broken symlink | silently dropped | **reported** (rider 1) |
| Fifo/socket/device named `x.lua` | silently dropped | `kind = Other` → **reported** under rider 3 |
| Entry whose type cannot be determined (`file_type()` error) | silently `continue`d | `kind = Unknown`, **named** in `entries` → **reported** under rider 3 |
| `.lua` name that is not valid UTF-8 (`file_stem().to_str()` is `None`) | silently dropped | **still dropped** unless rider 3 applies |
| `README.md`, `.gitignore`, an ordinary subdirectory | silently ignored | **unchanged — correctly silent** |

So rider 3 states the rule that makes the contract true without making the report noise:

> **An entry is reported to `skipped` if it is *plausibly a plugin* but cannot be loaded** — its name
> ends in `.lua`, or it is a directory containing `init.lua`, or it is a broken symlink (which we
> cannot classify and which is always actionable in a plugins directory). Everything else in the
> directory is legitimately ignored in silence.

The "plausibly a plugin" qualifier is what keeps this from degenerating: reporting every non-plugin
file would flood the report and make it useless, so a `README.md` stays silent and that is correct,
not a gap. Non-UTF-8 names are rendered lossily for the report — `DirEntryInfo::name` is already a
`String`, so the seam has done that conversion by the time `discover` sees it.

**One case where "named" degrades honestly.** An `Err` from the directory iterator itself has no name
to report — the entry could not be read at all. `list_dir` counts these in its dedicated `unreadable`
field (§5.2) rather than folding them into the cap arithmetic, so they surface as a **count** rather
than a row: the picker discloses them on their own line, and `discover` reports "*n* entries could not
be read" straight from `unreadable`. This is the one place the contract reads a count instead of
naming, and it is a limit of the filesystem, not of the design.

**Closure of the handle types, stated correctly — it splits.** `fs::DirEntry`, `fs::ReadDir`, and
   `fs::Metadata` genuinely need no separate rule: a value of those types can only be obtained by
   calling something layers 1–3 already gate, so their methods (`.metadata()`, `.file_type()`) are
   reachable only downstream of a gated call.

   `fs::File` is **not** closed that way, and an earlier draft claimed it was. Std provides
   `From<OwnedFd> for File` and `FromRawFd for File`, so a `File` can exist without any gated call
   having happened. The resolution is not to hedge the claim but to scope the rule: **§2.3 governs
   reaching the filesystem *by path*.** A `File` constructed from an existing file descriptor is not a
   path operation — it never names a path, cannot be routed through a path-taking seam, and is outside
   the rule's subject matter rather than an evasion of it.

   **Revisit condition for this boundary:** the codebase does no fd-based filesystem work today. If a
   future effort starts passing descriptors around (fd inheritance, `openat`-style work, a sandboxing
   layer), the path-scoped framing stops covering the surface and the rule needs widening — not just
   the scanner.

   **This is a live route, not a hypothetical** — production `file::save_atomic` calls
   `path.symlink_metadata()` today, which layers 1 and 2 both miss.

   **Revisit condition:** this list needs updating only if the standard library adds a
   filesystem-touching inherent method to `Path` — never because our own code changed. A future reader
   should check it against std, not against the tree.

**What the three layers together do and do not give.** Layer 3 is sound within its subject (a closed,
std-defined set). Layers 1 and 2 cover the ordinary spellings of `std::fs` access — an import in any
of the four common forms, or a fully-qualified path — and miss the nested/renamed/leading-root import
spellings listed above. All three are required, since each covers a route the others structurally
cannot see, but the combination is a **high-coverage detector, not a proof**.

The allow-list, not this document, is the census. That still inverts the failure mode where it
matters: an ordinarily-written raw call becomes a **failing test at the moment it is introduced**,
rather than a spec defect found in review or never. It fails with a message directing the author to
route the call through `Fs` or add an allow-list entry naming its clause. What it does not do is
guarantee that no raw access exists — an author who writes `use std::{fs::{self as f}};` will not be
stopped, and the spec does not pretend otherwise.

**Forward note — the sound version, and why it is not here.** Full soundness on layer 1 requires
parsing the `use` tree rather than matching text: resolving nested groups, `self as` renames, and
leading-root paths to the module they actually bind. That means a dev-dependency (a Rust parser) and a
small amount of real machinery. It was weighed for C5 and declined as disproportionate to the risk —
the uncaught spellings are deliberate-circumvention shapes, not accident shapes. **This is a
considered tradeoff with a known upgrade path, not an oversight**, and a future effort that wants the
stronger guarantee knows exactly what it costs.

**The self-check must exercise every detection route — one planted evasion each.** A self-check that
only plants `std::fs::read` proves layer 2 and nothing about layers 1 or 3; that is the
vacuous-guardrail failure this project has hit before. Layer 3 has **two** routes (dot-call and UFCS),
so the fixture plants **four** samples and asserts each is detected:

| Layer | Planted sample | Which layer must catch it |
|---|---|---|
| 2 | `std::fs::read(p)` — fully qualified | Call-token match |
| 1 | a module containing `use std::fs;` plus a short-form call `fs::write(p, b)` | Import gate |
| 3 (dot-call) | `p.symlink_metadata()` — inherent, no import, no `std::fs` token | Inherent-method enumeration |
| 3 (UFCS) | `Path::metadata(p)` — inherent, called UFCS, invisible to a `.method(` scan | Inherent-method enumeration, UFCS pattern |

A scanner that regresses to token-only matching fails row 2; one that omits the inherent-method set
fails row 3; one that matches only dot-calls fails row 4. Each row is invisible to the layers that do
not target it, which is why one sample per detection route is the minimum that proves anything — an
unexercised pattern is an unproven pattern.

Honest limits, stated so the test is not mistaken for a proof:

- It is **textual**, so it can flag a token in a comment or string literal. A false positive costs one
  allow-list line with a rationale — mildly annoying, never wrong-making.
- `#[cfg(test)]` stripping is heuristic (module-level attribute detection), the same approximation
  `module_budgets` already lives with.
- Import-gating covers the ordinary `std::fs` import spellings, **not all of them** — nested grouped,
  renamed-in-group, and leading-root `::std::fs` forms are uncaught and listed under layer 1.
- It does not attempt to catch filesystem access reached through a future third-party crate. That is a
  deliberate boundary — the rule here is about `std::fs`, and a new FS-touching dependency is a
  dependency review, not a scan.
- It is **path-scoped**: a `File` built from a raw or owned file descriptor is outside the rule's
  subject matter, not a hole in it (see the layer-3 closure note).
- It proves *routing*, not *correctness*: it cannot tell whether a seam call is used well. That is
  what §14's fault-injection tests are for.

It is a drift alarm, and a drift alarm is exactly what three rounds of findings showed was missing.

#### Verified illustration — NON-EXHAUSTIVE

The following were confirmed against the tree while writing this spec. They are examples showing the
rule applied, **not the definition of scope**, and a site absent from this list is not thereby out of
scope — the rule decides, and the guard test enforces. (This is the change that makes a missing row
harmless.)

| Class | Examples verified in the tree |
|---|---|
| Content reads | `file::open`, `file::bounded_read_opt`, `config::load`, `theme_resolve::resolve_theme`, `state::load_in`, `swap::read_swap_capped`, `swap::read_file_capped_bytes`, the `app.rs` startup override/mask reads, `plugin::load::discover`'s per-candidate read |
| Listings | `file_browser::rebuild_entries`, `file_browser_enter`'s readability probe, `swap::cleanable_recovery_files`, `swap::find_orphan_scratch_swap_in`, `plugin::load::discover`'s scan |
| Metadata probes | `save::fingerprint`, `state::file_identity`, `file::save_atomic`'s `symlink_metadata` refusal, the `target.exists()` checks in `prompts::save_as_submit` / `block_write_submit` / `export::run_export` / `jobs_apply::apply_export_done`, `export::run_pandoc`'s `tmp.exists()`, **`config::config_layer_paths`' `is_file()` probes**, **`plugin::load::discover`'s `init.is_file()`**, **the `app::run` startup probes** (override, `--config` mask, CLI path, and the `was_new_file` `!p.exists()`), **`clipboard::clip_env_from_process`'s PATH search** (`dir.join(bin).is_file()`) |
| Durable mutations | `fsx::atomic_replace` (already), `diagnostics_run::append_word_to_dict`, `jobs_apply::apply_export_done`'s rename, `prompts::resolve_prompt`'s `CleanRecovery` / `Recover` / `DiscardSwap` deletes, `swap::delete` |

The **bolded** metadata probes are the ones a previous draft's census omitted. They are in scope by the
rule — production shell code, class 3, no exemption clause — which is the point: the rule classified
them without anyone having to remember them.

C5 adds no `wc.fs` Lua API.

**One doc-comment correction.** `plugin::load::discover`'s doc comment currently states that it "does
not touch the `Fs` trait (write-only seam)." Once discovery's scan and read route through the seam,
that sentence is false and must be updated in the same change — a stale comment asserting an
invariant the code no longer has is worse than no comment.

---

## 3. Resolved decisions (human-ratified — do not re-litigate)

From brainstorming (forks F1–F4), the six post-report decisions (1–6), the three ratified against this
spec's §16/§17 (7–9), and three ratified during the spec-gate rounds (10–12) — all settled:

- **F1 = C.** Picker for C5's transactional moments; tree belongs to S2.
- **F2 = C.** Full unification of in-process FS access **within four named operation classes** —
  file-content reads, directory listings, stat/existence probes, and durable writes / app-artifact
  renames and deletes. §2.3 defines this as a **rule plus exemption clauses** (subprocess-owned IO,
  directory provisioning, `canonicalize`, the seam's own implementation, path-syntax-for-non-files,
  test code), backed by the `fs_chokepoint` guard test rather than by an enumeration — a
  high-coverage detector whose known gaps §2.3 states rather than papers over. The scope claim is
  deliberately narrower than "all filesystem access," which would be false.
- **F3 = A**, two orthogonal toggles (clutter / file type) + the **disclosure principle: the filter
  never silently lies** — a footer count whenever entries are withheld. Amended by decision 2
  (clutter loses gitignore) and by the mode-aware documents definition in §7.4.
- **F4 = A**, default-and-redirect extension policy (§8).
- **Decision 1 — F5:** path-derived swap naming permanent + binding identity contract + mint-and-stamp
  rider with lineage-hint semantics (§12).
- **Decision 2:** gitignore and the `ignore` crate dropped; **zero new dependencies**.
- **Decision 3:** recents in, favorites deferred.
- **Decision 4:** export Enter-through — pre-seeded destination picker reproduces today's
  zero-decision behavior on a bare Enter.
- **Decision 5:** destination-mode "New" cut.
- **Decision 6:** diverged orphans get visibility, never sweeping.
- **Decision 7 — one-level navigation.** The picker navigates one directory level at a time;
  recursive project-wide find belongs to **S2**. The accepted cost is stated honestly in §16.
- **Decision 8 — the two filter toggles are persisted settings.** `settings::SettingsSnapshot` plus
  the overrides serde mirror are in scope; the compile-time reachability gate is the reason (§13.2).
- **Decision 9 — the click divergence is deliberate.** Select mode: click selects **and** commits.
  Destination mode: click copies the name into the field and does **not** commit. Stakes-based
  justification recorded in §13.2 (law 5) so it is not "fixed" later.
- **Decision 10 — symlink classification is a bug C5 fixes.** A user-reported, source-verified defect
  (§4.9): symlinked directories are currently classified as files and are **unusable**, not merely
  mis-sorted. The human structures both their working directory and home directory with symlinks, so
  this is a primary workflow. Four resolutions, all adopted (§7.5): symlinked directories behave as
  directories; broken symlinks are listed and visibly marked and refuse to open, never silently
  hidden; entries carry a symlink marker in the render; and **no cycle detection** — with the reasoning
  stated rather than left for a reviewer to ask about.
- **Decision 11 — symlinked files must become saveable** (§4.10 → §7.6). A second verified defect:
  symlinked files open fine but every save is refused, so the writer cannot save at all.
  `file::save_atomic`'s symlink refusal is **correct and stays**; resolution happens before a path
  reaches it, at the four write-destination boundaries (§7.6.1). **`Document.path` keeps the
  as-opened path** and no display-path field is added. An earlier form of this decision canonicalized
  `Document.path`; it was **reversed after a consumer sweep** found seven behavior changes — one of
  which silently relocates exports — against three durability subsystems that already canonicalize at
  their own point of use and therefore gained nothing. The full argument, and the seven-consumer table
  as a standing map of what breaks if anyone canonicalizes it later, is §7.6.2.

- **Decision 12 — plugin discovery follows symlinks (a deliberate behavior change).** Symlinked
  `.lua` files and symlinked plugin directories are discovered and loaded; today they are silently
  ignored. **This is a behavior change adopted on purpose, not a refactor consequence**, and the
  reason it is defensible is a finding rather than a preference: `plugin::load::discover`'s trust
  boundary is **already porous** — it classifies entries with the non-following `entry.file_type()`,
  but its nested `init.is_file()` probe *does* follow, so a real directory whose `init.lua` is a
  symlink already loads today. Option B would have defended a boundary that is not intact. The
  security-shaped argument (a plugin is executable Lua entering from outside the curated directory)
  was examined and found outweighed: the user must deliberately create the link, which is itself an
  act of installation. Two riders and a silent-drop audit are in §5.2.

**Discovered scope, recorded during spec authoring** (not decisions — findings that shape the
migration):

- Of the three `atomic_replace` callers, **two** (`file::save_atomic` / `save_atomic_bytes` and
  `swap::write_atomic`) hardcode `&crate::fsx::RealFs` internally and so are not themselves
  fault-injectable. **`settings::save_overrides` / `write_overrides` already take
  `fs: &dyn crate::fsx::Fs`**, with `RealFs` injected at the composition root in `app.rs` and a
  `FailFs` driven through it in tests. An earlier draft of this spec asserted all three hardcode it;
  that was wrong, and the correction matters twice over — it removes work that is already done, and
  it means the migration's target shape is an **existing in-crate precedent** rather than a convention
  this spec invents (§5.2).
- `FaultFs` / `FaultHandle` / `FaultAt` are private to `fsx.rs`'s `#[cfg(test)] mod tests`, so
  promoting them into `test_support` is prerequisite work (§5.2).

---

## 4. Grounding — the code as it actually is

Every claim below was read against the tree at this spec's date and is anchored on **symbol names**.

### 4.1 The `Fs` seam is narrower than its name suggests

`fsx.rs` declares two `pub(crate)` traits: `WriteSync` (`write_all`, `flush`, `set_mode`, `sync_all`)
and `Fs` (`create_excl`, `existing_mode`, `rename`, `sync_dir`, `remove_file`). `RealFs` is the sole
production impl. The seam covers **exactly one operation**: `atomic_replace(fs, path, bytes,
WriteOpts { mode, dir_fsync })` — create-temp(O_EXCL) → write → `set_mode` → flush → `sync_all` →
`rename` → optional dir-fsync, with `TempGuard` RAII cleanup on the failure path. `ModePolicy` is
`Fixed(u32)` or `PreserveExistingOr(u32)`.

Three callers route through it, and they split **two-to-one** on how they obtain the `Fs`:

**Two hardcode `RealFs` internally**, so the caller itself cannot be fault-injected:

- `file::save_atomic` / `save_atomic_bytes` — `PreserveExistingOr(0o600)` / `Fixed(0o600)`, both
  `dir_fsync: true`; `save_atomic` additionally does a symlink refusal (`symlink_metadata` →
  `SaveError::Symlink`) and a skip-unchanged compare. Each passes `&crate::fsx::RealFs` from inside
  its own body.
- `swap::write_atomic` — `Fixed(0o600)`, `dir_fsync: false`, likewise passing `&crate::fsx::RealFs`
  internally.

**One already takes the seam as a parameter — and it is the precedent this effort follows:**

- `settings::save_overrides` and `settings::write_overrides` are declared
  `(fs: &dyn crate::fsx::Fs, path: &Path, …)`. The concrete `RealFs` is injected at the
  **composition root** in `app.rs`, not baked into the function. Their tests exercise that seam with a
  local `FailFs` implementing `crate::fsx::Fs` — see
  `settings::tests::save_overrides_surfaces_io_failure`, which asserts an injected IO failure reaches
  the caller.

**Two consequences the spec must design around.** First, `Fs` is write-only: reads, listing, stat,
`canonicalize`, and out-of-temp deletes are all raw `std::fs`. Second, the two `RealFs`-hardcoding
callers are not themselves fault-injectable — only `atomic_replace` is — so making `file::open` (or a
listing) fault-testable requires a signature change at those call sites, not merely a new trait
method. §5.2 specifies it, and specifies it as *matching what `settings` already does* rather than as
a new convention.

Additionally: `FaultFs`, `FaultHandle`, and `FaultAt` are **private to `fsx.rs`'s `#[cfg(test)] mod
tests`**. Extending fault coverage to call sites in other modules requires promoting them to a shared
test seam first. This is real work and is called out as its own task in §14.

### 4.2 Bounded reads exist but are ad hoc

`file::bounded_read_opt(path, limit) -> Option<Vec<u8>>` (`.take(limit + 1)`, `None` on over-cap or
any error) is the public helper. `swap.rs` has two private twins, `read_swap_capped` (String) and
`read_file_capped_bytes` (bytes). `state::load_in` open-codes the same `.take(cap + 1)` shape.
`file::open` open-codes it again with a metadata pre-check. Four implementations of one idea.

Caps live in `limits.rs`: `MAX_OPEN_BYTES` (64 MiB), `MAX_SESSION_BYTES` (8 MiB),
`PLUGIN_MAX_SOURCE_BYTES` (1 MiB).

### 4.3 Uncapped config-class reads

`config::load` and `theme_resolve::resolve_theme` use plain `std::fs::read_to_string`, as do the two
startup override/`--config` mask reads in `app.rs`. These are deliberately config-class (small files),
so this is hygiene rather than a live hazard — but they are the only remaining unbounded reads.

### 4.4 What bypasses the seam today

The seam is write-only and covers one operation, so **every** read, listing, and stat in the crate is
raw `std::fs` — §2.3's rule is what determines which of them come under the seam. Within that, three
sites are distinguished
because they perform operations the `Fs` trait **already declares** (`rename`, `remove_file`) or
perform a *durable write* outside `atomic_replace` — i.e. they bypass capability the seam already has,
rather than merely predating it:

- `jobs_apply::apply_export_done` — `std::fs::rename(&tmp, &target)` for the `WritesOutput` sinks.
- `prompts::resolve_prompt`'s `PromptAction::CleanRecovery` arm — `std::fs::remove_file` per
  snapshotted path.
- `diagnostics_run::append_word_to_dict` — `OpenOptions::new().create(true).append(true)` +
  `writeln!`. **Non-atomic, uncapped, no symlink guard, outside the seam.** It is the sole writer of
  the personal dictionary and the only durable write in the app not behind `atomic_replace`.

### 4.5 The file browser

```rust
pub struct FileEntry { pub name: String, pub is_dir: bool }
pub struct FileBrowser { pub dir: PathBuf, pub query: String,
                         pub entries: Vec<FileEntry>, pub selected: usize, pub scroll_top: usize }
```

`file_browser::rebuild_entries` calls `std::fs::read_dir` synchronously on the UI thread, sorts
dirs-then-files alphabetically, prepends a synthetic `".."` when `dir.parent().is_some()`, and filters
by **case-insensitive substring** (`name.to_ascii_lowercase().contains(&q)`) — not fuzzy. No
hidden-file filtering, no extension filtering, no entry cap.

**It re-runs `read_dir` on every query keystroke.** `intercept`'s `Char`, `Backspace`, and
`Event::Paste` arms each mutate `fb.query` and call `rebuild_entries`. This is the dominant
responsiveness defect and it is independent of threading — a per-directory cache with in-memory
filtering removes it.

`file_browser_enter` is the shared Enter path for keyboard and mouse: a directory is probed with
`read_dir(&target).is_ok()` before `fb.dir` is mutated (unreadable → Sticky Error, no mutation —
guarded by `enter_on_unreadable_dir_stays_put_and_sets_status`); a file closes the browser and calls
`workspace::open_as_new_buffer`.

Supporting infrastructure that C5 reuses rather than reinvents: `list_window::{list_h_for,
list_nav_key, apply_list_nav, wheel_list, WHEEL_STEP}`, `app::keep_overlay_visible`,
`chrome_geom::file_browser_row_at` (single-sourced geometry shared by `mouse::mouse_file_browser` and
`render_overlays::paint_file_browser`), and `palette::fuzzy_filter<T: Clone>(items, query, key)` — the
existing nucleo-matcher ranker used by the palette and outline, which the browser does **not**
currently use.

### 4.6 The overlay registration seam (H21)

`overlays.rs` holds `OverlayId` (11 variants, `Splash` pinned at index 0), `OverlayId::ALL`
(intercept-chain order), `OverlayId::row()` (exhaustive match into `OVERLAYS` — a new variant fails to
compile until a row exists), `OverlayRow { name, id, is_active, intercept, close, mouse, render }`,
`RenderSite::{Frame, StatusRow}`, `RENDER_ORDER` (a distinct permutation containing exactly the
`Frame`-site overlays), and `DispatchCtx { reg, keymap, ex, clock, msg_tx }` — which deliberately
excludes `&mut Editor` for aliasing reasons.

**C5 adds no new `OverlayId` variant.** Destination mode is a mode *within* `FileBrowser`, not a
second overlay. This is deliberate: two overlays would duplicate the intercept, painter, mouse fn, and
geometry, and would have to be kept in lockstep by hand — precisely the hand-parallel pathology H21
removed. See §7.1.

### 4.7 Save, Save-As, and the durability invariants that must survive

`save::do_save_to(ctx, target, mode)` dispatches `JobKind::Save`; the merge, on
`Ok(SaveOutcome::Saved | Unchanged)`, sets `document.path` (SaveAs only), `saved_version`,
`stored_fp` (from `new_fp`, fingerprinted against the **written** path), clears `swapped_version`,
deletes the current swap when the buffer landed clean, and on `SaveMode::SaveAs` also deletes the
`prior_key` swap — with the edited-during-write branch setting `last_swap_at = None` to expedite a
fresh swap under the new path. On `Err` it leaves `path`/`saved_version`/`stored_fp` untouched.

`swap::dispatch_swap_write`'s merge carries the **path-aware latch**:

```rust
if ok && swap_path(b.document.path.as_deref()).ok().as_ref() == Some(&path) {
    b.last_swap_at = Some(ts);
    b.swapped_version = Some(version);
}
```

guarded by `swap::tests::stale_path_swap_does_not_relatch_after_rekey`. The stale swap under the old
path is deliberately **not** deleted (a co-open buffer may legitimately own that `swap_path`).
`swap::pending(dirty, version, swapped_version)` is version-keyed, which is why a stale relatch would
suppress a fresh swap. **C5 touches none of this.**

### 4.8 Facts the grounding packet got wrong — recorded so they are not re-introduced

These were verified against source and are stated here because a reader of the packet would otherwise
carry the errors forward:

- **`Buffer.pending_swap_path` is NOT a dormant field.** It is written in `app.rs`'s startup recovery
  when `swap::find_orphan_scratch_swap()` stages an orphan scratch swap, and read by
  `prompts::resolve_prompt` in the `Recover`, `DiscardSwap`, and `OpenOriginal` arms. Test:
  `recover_loads_body_and_deletes_orphan_swap_file`. It is the orphan-scratch recovery carrier;
  removing or repurposing it breaks Recover.
- **Session-entry stranding is hygiene, not a durability gap.** `persist_session` is not exit-only —
  `app::run` persists whenever `saved_version` advances (the `sv != last_persisted_saved` branch), so
  the new path gets an entry one loop tick after the Save-As merge. The stranded old entry still
  matches the old file's mtime+size identity via `state::file_identity`, so restoring it when that
  file is reopened is arguably correct. Fix it (§11.1) but describe it honestly.
- **`SessionState.entries` is a `BTreeMap<String, StateEntry>`**, not a `HashMap`.
- **tokio is a stale `Cargo.lock` entry** — no dependency path on any target, zero symbols in the
  release binary. It does not ride in under `harper-brill`/`burn`.

### 4.9 VERIFIED DEFECT — symlinked directories are classified as files and are unusable

User-reported and confirmed against source. `file_browser::rebuild_entries` classifies each entry as:

```rust
let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
```

`std::fs::DirEntry::file_type()` **does not follow symlinks** — on Unix it is `d_type` from
`readdir`, falling back to `lstat` when the filesystem reports `DT_UNKNOWN`. A symlink pointing at a
directory therefore reports `is_dir == false`.

`FileEntry::is_dir` has exactly three consumers, and the misclassification corrupts all three:

1. **Sorting** (`rebuild_entries`) — the symlinked directory is pushed onto `files`, so it sorts into
   the files group instead of with the directories.
2. **The Enter dispatch** (`file_browser_enter`) — the `is_dir` branch is not taken, so the entry
   routes to `workspace::open_as_new_buffer` → `editor::Buffer::from_file` → `file::open`, which
   returns `OpenError::IsDir` ("…: is a directory"). **The entry is unusable, not merely mis-sorted** —
   the writer cannot enter the directory at all, and the error they get names a condition the UI just
   told them was false.
3. **The render label** (`render_overlays::paint_file_browser`) — the label is
   `if e.is_dir { format!("{}/", e.name) } else { e.name.clone() }`, so the symlinked directory is
   also **missing its trailing `/`**. The writer has no visual cue that the entry is a directory, so
   the failure is not even diagnosable from the screen.

Note what is **already correct** and must not be "fixed": `file_browser_enter`'s pre-descend
readability probe is `std::fs::read_dir(&target).is_ok()`, and `read_dir` *does* follow symlinks — so
the descend path itself works fine for a symlinked directory. The defect is purely in classification;
once `is_dir` is right, the existing descend logic needs no change.

This is not an edge case for this user: the human structures both their working directory and home
directory with symlinks (decision 10).

### 4.10 VERIFIED DEFECT — symlinked files are openable but unsaveable

The sibling of §4.9, and independently confirmed. `file::save_atomic` opens with:

```rust
match path.symlink_metadata() {
    Ok(meta) if meta.file_type().is_symlink() => return Err(SaveError::Symlink),
    _ => {}
}
```

**This refusal is correct and must stay.** `fsx::atomic_replace` renames a temp over the target; done
to a symlink it would replace the *link* with a regular file and destroy the link. Tests
`file::tests::save_through_symlink_refused`, `save::tests::background_save_failure_keeps_dirty_and_status`,
and `save::tests::background_save_failure_is_a_sticky_error_that_survives_a_later_info` pin it.

But a symlink-to-file sorts as a file and **opens fine** — `file::open` uses `fs::File::open`, which
follows links. So the buffer ends up holding the *symlink* path, and every subsequent save is refused.
**Symlinked files are openable but unsaveable.** It is not data loss (the buffer stays dirty and
`swap::pending` keeps the swap covering it), but the writer simply cannot save their document, and the
error names a mechanism rather than a remedy.

Same primary-workflow status as §4.9: the human structures their home and working directories with
symlinks.

---

## 5. The chokepoint

### 5.1 Shape: extend `fsx`, do not replace it

`fsx.rs` keeps its identity — "the fault-injectable filesystem seam" — and grows. The alternative
(a new module owning all FS policy) would orphan `atomic_replace`'s hard-won failure semantics and
its fault tests. `Fs` stays object-safe (`&dyn Fs`) and `pub(crate)`.

### 5.2 The closed primitive set

`Fs` grows exactly three methods. This set is **closed**: it is the minimum that lets every in-process
call site be expressed and fault-injected. Resist widening it into a god-trait.

| Method | Contract |
|---|---|
| `read_capped(&self, path: &Path, limit: u64) -> std::io::Result<Option<Vec<u8>>>` | Reads at most `limit + 1` bytes; `Ok(None)` when the file exceeds `limit`; `Err` on IO failure. Distinguishing over-cap from IO error is why this returns `Result<Option<_>>` and not `Option<_>` — the existing `bounded_read_opt` conflates them, which is acceptable for its degrade-silently callers but is the wrong primitive for a seam. |
| `list_dir(&self, path: &Path, cap: Option<usize>) -> std::io::Result<DirListing>` | **Enumerates fully; retains at most `cap` when `Some`, everything when `None`.** `DirListing { entries: Vec<DirEntryInfo>, total_seen: usize, unreadable: usize }`. **`DirEntryInfo` carries the resolved classification — see below.** |
| `stat(&self, path: &Path) -> std::io::Result<FileStat>` | `FileStat { len: u64, mtime: Option<SystemTime>, is_file: bool, is_dir: bool, is_symlink: bool, broken: bool }`. **The file-type, symlink, and broken-link semantics are all load-bearing — see below.** |

**Why `list_dir` enumerates past the cap.** An earlier form of this spec capped *enumeration* at `cap`
while promising a "showing 5,000 of 38,412" disclosure — self-contradictory, because stopping at 5,000
means the total is never learned. Since §7.4's disclosure law requires that shown + withheld account
for what is really there, the count has to be real. So the iteration runs to the end of the directory
and only *retention* is capped. The cost is honestly **bounded memory, off-UI `O(total)` time**:

- **Memory stays bounded** at `cap` entries — the property that actually matters.
- **Time is `O(total)`, and that is real work.** Enumerating a huge or network-mounted directory can
  take a while. What the cap buys is that the per-entry cost past it is only a `readdir` step — no
  retained allocation, and **no `metadata` call**, since symlink resolution (below) runs on retained
  entries only. So it is markedly cheaper per entry than the naive alternative, not free.
- Because it is real work, it runs on the listing thread (§6.3), never the UI thread — which is
  exactly the hung-mount case that thread exists for. A pathological directory delays the listing and
  shows `Listing…`; it cannot stall input, and the picker stays closable throughout.

#### The cap is opt-in, and only the interactive lister opts in

**`cap` is `Option<usize>`, not `usize`** — the caller states intent in the signature rather than in
prose, so nobody can route a new caller through the capped path by accident.

| Caller | `cap` | Why |
|---|---|---|
| The picker's directory listing (§6.3) | `Some(MAX_DIR_ENTRIES)` | Interactive. A writer is waiting on a redraw; a 38,000-entry directory must not stall the list. |
| `plugin::load::discover` | **`None`** | See below. |
| `swap::cleanable_recovery_files`, `swap::find_orphan_scratch_swap_in` | **`None`** | Same reasoning: startup/command-time scans of our own state dir, off the interactive path, and capping them would silently shrink what `clean_recovery` can find. |

**Why discovery must be uncapped — it is a regression question, not a preference.** Verified against
the tree: `discover` today collects candidates from `read_dir` with **no cap at all**, and both
`swap` scans are likewise uncapped. So capping them would not be "declining to add a protection" —
it would be **introducing a new restriction the current code does not have**, by refactor, silently.
That is the same accident class as decision 12, and it directly contradicts decision 12's rider 3:
a plausible-plugin entry past the cap would be neither loaded nor named, which is precisely the
silent drop that rule exists to eliminate.

The cap earns its place on the interactive path and nowhere else. A plugins directory is small, read
once at startup, and off the redraw path — the cap buys nothing there and costs the contract. Stated
in the signature, and stated here, so a later "let's unify the listing callers" change cannot
reintroduce it without confronting this paragraph.

#### `total_seen` and `unreadable` are separate counters

An earlier form of this spec had one counter and let `total_seen - entries.len()` mean two different
things at once: entries omitted by the cap, and entries the directory iterator could not read at all
(today swallowed by `.flatten()` in both `discover` and the browser). Those are different facts, and
a writer would act on them differently — **"showing 5,000 of 38,412" is normal; "3 entries could not
be read" means something is wrong with their filesystem.** Conflating them also made the cap conflict
above hard to see, which is reason enough to split them.

- `total_seen` — every entry the iterator yielded, `Ok` or `Err`.
- **`unreadable` — entries that could not even be NAMED**, i.e. the iterator itself yielded `Err`.
  Nothing else belongs here. This field's meaning has been narrowed twice during review, so it is
  stated exactly: it is **not** "entries we could not classify." A named entry whose *type* probe
  failed is a perfectly good row with `kind = Unknown` and lives in `entries` (case 2 above), because
  a name is more useful than a tally and rider 3 needs it. `unreadable` is a count only because there
  is genuinely nothing to name.
- `entries` — **named** entries that were retained, whether or not their type resolved.

Invariant, asserted by test: `total_seen == entries.len() + unreadable + capped_out`, where
`capped_out` is `0` whenever `cap` is `None`. With that, §7.4's disclosure law holds precisely and each
number means one thing: the picker discloses the cap and the unreadable count as **separate** lines,
and `plugin::load::discover` reports "*n* entries could not be read" from `unreadable` alone
(§5.2, rider 3) rather than inferring it from a subtraction that also contains cap effects.

**`list_dir` resolves symlinks once, at the seam.** This is the fix for §4.9, and it belongs here
rather than in the picker so that the follow/don't-follow subtlety is stated in exactly one place and
both consumers inherit it:

```rust
/// What the entry resolved to. An ENUM, not a pair of bools, so `Unknown` cannot be
/// silently absorbed into a false branch — the house rule on exhaustive matches
/// (avoid a catch-all that swallows a new variant) applied to the exact failure mode
/// this review kept surfacing.
pub(crate) enum EntryKind {
    /// RESOLVED regular file (follows symlinks).
    File,
    /// RESOLVED directory (follows symlinks).
    Dir,
    /// RESOLVED to something that is neither — fifo, socket, block/char device.
    Other,
    /// NOT classified. Either the `file_type()` probe itself failed, or this is a
    /// symlink whose target could not be resolved (`broken`). We have a name but no type.
    Unknown,
}

pub(crate) struct DirEntryInfo {
    pub name: String,
    pub kind: EntryKind,
    /// True when the entry itself is a symlink, whatever it points at.
    pub is_symlink: bool,
    /// A symlink whose target could not be RESOLVED — dangling, permission-denied along the
    /// chain, or a resolution loop. Identical meaning to `FileStat::broken`.
    /// INVARIANT: `broken` implies `is_symlink` and `kind == Unknown`.
    pub broken: bool,
}
```

Invariants, asserted by test: `broken` implies `is_symlink`; `broken` implies `kind == Unknown`.
(`FileStat` keeps its bool fields and states the parallel invariant in its own terms — see below.)

**Resolution algorithm, and why it costs almost nothing.** A naive fix — `metadata()` on every entry —
would add one stat syscall per entry, 5,000 of them in a capped listing. That is unnecessary, because
`DirEntry::file_type()` already yields the symlink bit for free:

- `entry.file_type()` — free (`d_type` from `readdir`; std falls back to `lstat` only on
  `DT_UNKNOWN` filesystems). Authoritative for `is_symlink` when it succeeds.
- **Probe fails** (`Err`) → `kind = Unknown`, `is_symlink = false`, `broken = false`. **The entry is
  still emitted, with its name.** This must NOT use `?` — propagating would abort the whole listing
  because one entry was unclassifiable, turning a single bad entry into a failed directory.
- **Not a symlink** → `kind = File | Dir | Other` from `ft.is_file()` / `ft.is_dir()`,
  `is_symlink = false`, `broken = false`. **Zero extra syscalls — exactly today's cost.**
- **Is a symlink** → one `metadata()` call (which follows) to resolve: `Ok(m)` →
  `kind = File | Dir | Other` from `m.is_file()` / `m.is_dir()`, `broken = false`;
  `Err(_)` → `kind = Unknown`, `broken = true`.

**`kind` is assigned from the file type, never inferred, and `Unknown` is a real state.** The three
classified variants come straight from `ft`/`m`; nothing is derived as "not a directory." Two reasons
this is an enum rather than `is_file`/`is_dir` bools:

1. **`Other` and `Unknown` are different facts that bools cannot separate.** With two bools, a
   legitimately-classified fifo (`Other`) and an entry we simply could not classify (`Unknown`) are
   both `false, false` — indistinguishable, which is precisely what let case 2 below go unnoticed.
2. **`Unknown` must be impossible to ignore.** An exhaustive `match` on `kind` forces every consumer
   to say what it does with it. A bool flag can be skipped silently, and silent drops are the defect
   class this spec has had to close repeatedly.

Consumers **decide by matching on `kind`**, never by testing a convenience accessor, so `Unknown`
cannot be swept into a false branch. Decision 12's rider 2 depends on this: a fifo named `x.lua` is
`Other`, not `File`, so it cannot become a plugin candidate.

**Three entry categories, three destinations.** `DirEntry` enumeration has exactly three outcomes, and
each now has an accounting path:

| # | Case | Name? | Type? | Goes to |
|---|---|---|---|---|
| 1 | Iterator yields `Err` | **no** | no | `unreadable` count |
| 2 | Named entry, `file_type()` probe fails | **yes** | no | `entries`, `kind = Unknown` |
| 3 | Named entry, type resolved | yes | yes | `entries`, `kind = File`/`Dir`/`Other` |

Case 2 is the one an earlier draft had no home for: `unreadable` was justified specifically as "cannot
be named," so a named-but-unclassified entry does not belong there — a name is strictly more useful
than an anonymous tally, and rider 3 needs it to test "plausibly a plugin" from the `.lua` suffix
alone. Today `discover` silently `continue`s on this case, which is the drop decision 12 exists to
prevent.

**What each consumer does with `Unknown`:**

- **The picker** shows it, marked, and refuses both actions — it cannot be descended into (we do not
  know it is a directory) and must not be opened blind (opening a fifo would block). Same treatment as
  a broken symlink, which is consistent because `broken` is a subset of `Unknown`.
- **`plugin::load::discover`** reports it in `skipped` when the name is plausibly a plugin
  (§5.2 rider 3), by name.
- **The swap scans** ignore `kind` entirely. Verified: `swap::cleanable_recovery_files` and
  `find_orphan_scratch_swap_in` both classify purely from `entry.file_name()` string patterns
  (`*.swp`, `recovered-*.md`, `*.tmp`, `scratch-{pid}.swp`) and then read the file; neither calls
  `file_type()`. So an `Unknown` entry flows through their existing filename logic unchanged, and no
  behavior changes for them.

So the added cost is exactly one stat **per symlink**, which is the minimum possible, and it lands on
the listing thread (§6.3), never the UI thread. An ordinary directory of regular files pays nothing.

**`stat` must follow symlinks for `len`/`mtime`/`is_dir`, and report `is_symlink` separately.** This
is not a stylistic choice; getting it backwards is a silent durability regression. Every existing stat
caller uses `std::fs::metadata`, which **follows** symlinks — `file::open`'s size pre-check,
`save::fingerprint` (whose `mtime`/`size` feed the external-modification guard),
`state::file_identity` (the session-resume staleness guard), and `RealFs::existing_mode`. Exactly one
site uses `symlink_metadata`, and it wants only the type bit: `file::save_atomic`'s symlink refusal
(`SaveError::Symlink`). A `FileStat` built solely from `symlink_metadata` would therefore report the
*link's* size and mtime to the fingerprint and resume guards, silently breaking external-mod detection
for any symlinked document. `RealFs::stat` accordingly performs `metadata` for `len`/`mtime`/`is_dir`
and `symlink_metadata` for `is_symlink` — two syscalls, one method, both existing behaviors preserved
exactly. A test asserts that a symlink to a larger file reports the **target's** length while
`is_symlink` is `true`.

`stat` and `list_dir` therefore make the **same** follow/don't-follow distinction, expressed once
each: follow for "what is it," don't-follow for "is it a link." Any future FS primitive that needs the
distinction states it in these terms rather than re-deriving it.

**`FileStat` carries `broken`, mirroring `DirEntryInfo`.** Without it nothing can satisfy §7.6.1's
requirement to refuse a broken-symlink destination before dispatch: `canonicalize` fails identically
for "does not exist yet" (the *normal* Save-As case) and "symlink whose target is gone," and a
`FileStat` built only from `metadata` cannot be produced at all for a broken link because `metadata`
fails outright. This is the same distinction `list_dir` already draws, so it gets the same name rather
than a second spelling of one idea.

The two-syscall contract already specified for `stat` extends naturally:

- `symlink_metadata(path)` **Err** → the path does not exist in any form → `stat` returns `Err`. This
  is the ordinary "new file" answer, and it stays distinguishable from a broken link.
- `symlink_metadata` **Ok** → the entry exists. `is_symlink = lm.file_type().is_symlink()`.
- then `metadata(path)` **Ok** → `is_file` / `is_dir` / `len` / `mtime` from it; `broken = false`.
- `metadata` **Err** and `is_symlink` → **`broken = true`**, with `is_file = false`, `is_dir = false`,
  `len = 0`, `mtime = None`.
- `metadata` **Err** and not a symlink → a genuine IO/permission error, not a broken link → propagate
  `Err`. `broken` is never used to paper over an unreadable regular file.

**`broken` means UNRESOLVABLE, not "the target is gone" — one definition, shared with
`DirEntryInfo`.** Both seam methods use it identically: *the entry is a symlink whose target could not
be resolved.* That covers a dangling target, a permission denial anywhere along the chain, and a
resolution loop (`ELOOP`), because `metadata` reports all three as `Err` and the seam does not
distinguish them. An earlier draft defined it as "the link exists; its target does not" in `FileStat`
and "dangling, or an unreadable chain" in `DirEntryInfo` — two definitions that disagree on exactly the
permission case, which would have produced diagnostics claiming a file was missing when it was merely
unreadable.

**Consequence for user-facing wording:** because a permission failure reports `broken == true`,
messages must not assert the target is absent. §7.6.1's destination refusal therefore reads
"destination symlink cannot be resolved" rather than "target is gone," and the listing marker
(§7.5) means the same. If a future effort needs to tell dangling from permission-denied — to offer a
different remedy — the seam would carry the `io::ErrorKind` alongside the flag; C5 does not, because
no caller currently acts differently on the distinction.

Invariants, holding for both types: `broken` implies `is_symlink`; `broken` implies
`!is_file && !is_dir`.

**Migrated callers must preserve today's behavior exactly, and one needs an explicit mapping.**
`save::fingerprint` currently begins `std::fs::metadata(path).ok()?` — so a broken symlink yields
`None` today. Under the seam it must map `Ok(s) if s.broken => None` to keep that, otherwise a broken
link would produce a `Some` fingerprint with zeroed fields and silently corrupt the external-mod
comparison. `file::open`'s size pre-check likewise treats `broken` as "skip the pre-check and let the
open fail," which is what its `if let Ok(meta)` does today. A test covers a broken-symlink path
through `fingerprint` returning `None`.

**`file::save_atomic_bytes` gains a symlink guard.** It has none today — unlike `save_atomic`, it goes
straight to `atomic_replace` — and it is the **export write path** (`jobs_apply::apply_export_done`'s
`Bytes` arm) as well as the session-state write path. Before C5 that was tolerable because export
targets were *derived*, never chosen; C5 makes them user-selectable for the first time (§9), so an
export target can now be a symlink the writer picked. Upstream resolution (§7.6.1) is the primary
protection, but it is not sufficient alone: the target can become a symlink between resolution and the
write, and `apply_export_done` already re-checks for a TOCTOU-appeared target for exactly this class of
reason. So the guard is added, making the last-resort invariant uniform across both durable-write
entry points rather than dependent on every caller having resolved first.

Consequence to note rather than discover: session-state writes (`state::SessionState::save_in` →
`save_atomic_bytes`) also acquire the guard, so a symlinked `session.toml` would now be refused where
it previously wrote through. That is the same protection `save_atomic` has always given user
documents, applied to an app artifact; it is a deliberate change, not a side effect.

**`is_file` is a distinct field, not `!is_dir`.** §2.3's rule puts `Path::is_file()` probes in scope —
`config::config_layer_paths`' walk-up search, `plugin::load::discover`'s `init.is_file()`, the
`app::run` startup probes, and `clipboard::clip_env_from_process`'s PATH search
(`dir.join(bin).is_file()`). A caller cannot reconstruct `is_file()` from the size/time/dir/link
fields alone without assuming `!is_dir` means "file," which is **wrong** for fifos, sockets, block and
character devices, and symlinks pointing at any of them. On a system where a config path is a fifo,
that assumption turns a correct "not a regular file, skip it" into an attempt to read, which blocks.

So `is_file` is carried explicitly, with exactly `std::fs::Metadata::is_file()` semantics — regular
file, following symlinks. The consequence to keep in mind: **`is_file` and `is_dir` can both be
`false`**, which is the entire reason the field exists. Each migrated probe site substitutes
`fs.stat(p).map(|s| s.is_file).unwrap_or(false)` for `p.is_file()`, preserving current behavior
exactly — including that an unreadable or missing path answers `false` rather than propagating an
error, which is what `Path::is_file()` does today at every one of those sites. A test covers a
non-regular-file path (a fifo where the platform allows creating one) reporting `is_file == false` and
`is_dir == false` together.

Plus: `rename` and `remove_file` are already declared and simply gain external callers (§5.3).

**Signature convention: match `settings`.** The target shape is not invented here — it already exists
in this crate. `settings::save_overrides` / `write_overrides` take `fs: &dyn crate::fsx::Fs` as their
first parameter, `app.rs` injects the concrete `RealFs` at the composition root, and
`settings::tests::save_overrides_surfaces_io_failure` drives a local `FailFs` through that parameter.
**That is direct evidence the pattern delivers the fault-injectability this section claims it will**,
in the same crate, on a durable write — not a projection.

So the convention is simply: **an in-scope operation takes `&dyn Fs`.** Where a function's public
signature must stay source-compatible for existing callers, it splits into an `&dyn Fs`-taking core
plus a thin `RealFs` wrapper:

```rust
pub fn open(path: &Path) -> Result<String, OpenError> { open_with_fs(&crate::fsx::RealFs, path) }
pub(crate) fn open_with_fs(fs: &dyn Fs, path: &Path) -> Result<String, OpenError> { … }
```

The split-with-wrapper form also has precedent for injection generally —
`swap::find_orphan_scratch_swap` / `find_orphan_scratch_swap_in` and `state::load` / `state::load_in`
do it for directories. Keeping the wrapper leaves every existing call site source-compatible, so the
migration is additive and green at each step.

#### Ownership: `&dyn Fs` synchronously, `Arc<dyn Fs + Send + Sync>` across a thread

The convention above is **not sufficient on its own**, and taking it literally everywhere produces
code that does not compile. `jobs::Job` declares `run: Box<dyn FnOnce() -> JobResult + Send>`, and
§6.3 spawns the listing on a bare `std::thread` — both require `'static + Send` state, which a
borrowed `&dyn Fs` cannot provide. An implementer hitting that would do the natural thing and hardcode
`RealFs` inside the spawned closure, **silently destroying the read/list fault-injectability that
justifies extending the seam at all.** That is exactly what today's code does (`file::save_atomic`
constructs `&crate::fsx::RealFs` inside the worker closure), and it is why `save_atomic` is not
injectable today (§4.1).

So there are two forms, and which applies is determined by one question — *does this call cross a
thread boundary?*

| Context | Form | Precedent |
|---|---|---|
| Synchronous, on the main thread | `&dyn Fs` parameter | `settings::save_overrides`, injected at `app.rs`'s `perform_settings_save(…, &crate::fsx::RealFs)` call under `editor.borrow_mut()` — the precedent is specifically a **synchronous** one |
| Inside a `jobs::Job` closure, or a spawned listing thread | **owned** `Arc<dyn Fs + Send + Sync>`, cloned into the closure | new in C5 |

**Bounds go on the async call sites, not on the trait.** `Fs` does **not** gain `Send + Sync`
supertraits; the async sites spell `dyn Fs + Send + Sync` instead. This keeps the synchronous
convention and the `settings` precedent untouched, and leaves room for a future single-threaded test
double (one using `Rc`/`RefCell` for recording) that could not satisfy a trait-level `Sync`.

Both current impls already satisfy the bounds: `RealFs` is a unit struct with no fields, and `FaultFs`
holds `RealFs` plus a `Copy` enum. **`WriteSync` needs no `Send` bound** — `create_excl`'s
`Box<dyn WriteSync>` is created and consumed entirely inside `atomic_replace` on one thread, so it
never crosses a boundary; only the `Fs` *impl* does.

**Where the `Arc` comes from.** The composition root (`app::run`) builds one
`Arc<dyn Fs + Send + Sync> = Arc::new(RealFs)` and carries it as an ambient service on
`registry::Ctx` and `overlays::DispatchCtx` — both of which already carry exactly this kind of
injected dependency (`ex: &dyn Executor`, `clock: &dyn Clock`), so this follows the existing shape
rather than inventing a channel. Dispatch sites `clone()` the `Arc` into the closure they build. Tests
substitute an `Arc::new(FaultFs { … })` at the same point, which is what makes the worker-side read,
write, and listing paths injectable for the first time.

**Sweep — every background-thread or job-closure seam call in this spec:**

| Site | Thread context | Form |
|---|---|---|
| `save::do_save_to`'s worker (`save_atomic`, `fingerprint`) | `jobs::Job` closure | owned `Arc` |
| `swap::dispatch_swap_write`'s worker (`write_atomic`) | `jobs::Job` closure | owned `Arc` |
| The directory listing (§6.3) | bare `std::thread` | owned `Arc` |
| `export::run_pandoc`'s `tmp.exists()` verification probe | inside `export::do_export`'s spawned `std::thread` | owned `Arc` |
| `jobs_apply::apply_export_done` (rename, `save_atomic_bytes`) | **main thread** — it is a `Msg` arm | `&dyn Fs` |
| `prompts::resolve_prompt` deletes, `diagnostics_run::append_word_to_dict`, `config`/`theme` reads, `state` load/save, `settings` | main thread | `&dyn Fs` |
| `recovery::dump_on_panic` | panic hook, arbitrary thread | **stays `RealFs`** — see below |

**The export worker is split, not exempt as a whole.** `export::do_export` spawns a thread that runs
`run_pandoc`, and two different things happen inside it. Pandoc's own `-o` write is **exempt** under
clause (a) — the subprocess owns its output. But `run_pandoc`'s `if !tmp.exists()` verification probe
is **ours**: an in-scope class-3 metadata probe that happens to sit in the same function. It takes the
owned `Arc` like any other cross-thread seam call. Stated explicitly because the natural mistake is to
see "export worker → pandoc → exempt" and exempt the whole closure by association, leaving a raw
non-injectable probe behind.

`recovery::dump_on_panic` is a deliberate exception. It runs from a process-global panic hook with no
access to `Ctx`, uses `try_lock` specifically to never block, and exists to write when everything else
has already failed. Threading an injected `Fs` into it would add a global to the one path whose entire
value is having no dependencies. It keeps `RealFs` and is allow-listed accordingly.

**`settings` needs no migration.** It is already on the seam for its writes; its remaining raw calls
are the parent `create_dir_all` / `set_permissions` / `exists()` trio, which are exempt under clause
(b) (directory provisioning). C5 touches `settings` because the persisted filter toggles (§13.2, law
2) add fields to `SettingsSnapshot` and the overrides mirror — not because its FS access needs
changing.

**`FaultFs` must be promoted** out of `fsx.rs`'s private test module into the shared test seam
(`test_support.rs`, which already hosts `TestClock` and `install_enabled_harper`) and gain arms for
the three new methods. This is a prerequisite task, not a side effect. Note that `settings`' `FailFs`
is a *separate*, test-local minimal impl — promoting `FaultFs` gives the migrated read/list/stat sites
a shared fault harness with per-step injection, which a hand-rolled per-test `FailFs` does not.

### 5.3 The migration set

**The migration set is defined by §2.3's rule, not by any list in this document** — including the
illustration in §2.3 and the subtle-site notes below. An implementer determines whether a given call
site is in scope by applying the rule to it; the `fs_chokepoint` guard test then catches
ordinarily-written sites the sweep missed (within the coverage limits §2.3 states — it is a
high-coverage detector, not a completeness proof). Neither this section nor §2.3's table is an
inventory to work down.

Most in-scope sites are a direct substitution of a seam call for a `std::fs` call with no behavior
change. What follows is guidance for the minority whose behavior is subtle enough that a mechanical
port would get them wrong. §4.4 separately distinguishes three of these as *bypasses* — sites using
capability the trait already declares (`rename`, `remove_file`) or writing durably outside
`atomic_replace` — which is a statement about their history, not a scoping claim.

**Subtle sites, with their required behavior preservation:**

- **`jobs_apply::apply_export_done`** — the `TempReady` arm's `std::fs::rename(&tmp, &target)` and its
  failure-path `remove_file` route through `Fs`. The TOCTOU guard (`!overwrite_confirmed &&
  target.exists()` → refuse and clean up the temp) is behavior-preserved exactly; the existence probe
  becomes `fs.stat(...)`. Guard tests to keep green:
  `apply_export_done_rename_failure_is_a_sticky_error`,
  `apply_export_done_toctou_target_appeared_is_a_sticky_warning`.
- **`prompts::resolve_prompt`'s `CleanRecovery` arm** — the per-path `std::fs::remove_file` routes
  through `Fs`. **The bidirectional TOCTOU discipline is preserved verbatim**: the snapshot in
  `pending_clean` remains the ceiling (never a re-scan), and `swap::recovery_path_still_cleanable` is
  still re-run per path before each delete so the set can only ever narrow. Only the delete call
  changes.
- **`diagnostics_run::append_word_to_dict`** — becomes **read-capped → append in memory → atomic
  replace** through the seam, gaining the symlink refusal and cap that every other durable write has.
  This is a genuine behavior change (a torn append becomes impossible; a symlinked dictionary is now
  refused) and gets its own tests, including preservation of the existing
  `append_word_to_dict_creates_parent_dir` contract.
- **`prompts::resolve_prompt`'s `Recover` and `DiscardSwap` arms, and `swap::delete`** — raw
  `remove_file` calls that route through the seam. `Recover`'s delete must stay **after**
  `load_recovered` and must continue to consume `pending_swap_path` (§4.8: that field is the
  orphan-scratch recovery carrier, not dormant). `swap::delete` stays **best-effort** — its result is
  discarded today and must remain so, because a failed swap cleanup is never worth surfacing to the
  writer or failing a save over. Guard test:
  `prompts::tests::recover_loads_body_and_deletes_orphan_swap_file`.

### 5.4 Config-class caps

`config::load`, `theme_resolve::resolve_theme`'s `theme.file` read, and the two `app.rs` startup
override/mask reads move to `Fs::read_capped` with a new `limits::MAX_CONFIG_BYTES` (proposed **1
MiB** — generous for TOML, and consistent with `PLUGIN_MAX_SOURCE_BYTES`). Over-cap degrades exactly as
a parse failure does today (defaults + a status), never a panic and never a silent difference.

### 5.5 What the chokepoint deliberately does not do

It does not make FS operations *cancellable*, does not add a virtual filesystem, and does not attempt
to interpose on `std::fs::canonicalize` — canonicalization is a pure path query used by `swap_path`
and `persist_session`, and routing it through the seam would buy nothing testable while touching the
swap-naming path, which §12 puts under a stability guarantee.

---

## 6. The listing pipeline

Three layers, simplest first. Layers 1 and 2 are the responsiveness fix; layer 3 is the
hung-filesystem fix.

### 6.1 Layer 1 — cache the listing, filter in memory

`FileBrowser` gains a `listing: Vec<DirEntryInfo>` holding the **unfiltered** directory contents, and
`entries` becomes the *derived* filtered/ranked view. `read_dir` runs on open and on descend — never
on a query keystroke. The keystroke path becomes a pure function over `listing`.

This alone removes the per-keystroke syscall storm described in §4.5.

### 6.2 Layer 2 — cap with disclosure

Retention **in the picker** is capped at `limits::MAX_DIR_ENTRIES` (proposed **5,000**) — the picker
passes `Some(MAX_DIR_ENTRIES)`; enumeration is never capped, and non-interactive scans pass `None`
(§5.2). When entries were capped out, the footer discloses the real numbers: `showing 5,000 of 38,412`.
When `unreadable > 0` the footer carries that **separately** — `3 entries could not be read` — because
it means something is wrong with the filesystem rather than merely that the directory is large. The cap
is on the *listing*, not the filtered view; the filter's own withholding is disclosed separately
(§7.4). Both are instances of one law: **the picker never silently withholds**.

### 6.3 Layer 3 — `read_dir` off the UI thread, and NOT on the jobs queue

**This is a durability constraint, not a performance preference.** `jobs::ThreadExecutor` is a single
FIFO worker (`wcartel-jobs`) shared with `JobKind::Save` and `JobKind::SwapWrite`. A listing blocked on
a hung network mount would queue **ahead of the user's save and swap writes**, converting a browsing
hiccup into a durability outage. Directory listings therefore **must not** use `jobs.rs`.

They use the `export::do_export` precedent instead: a dedicated `std::thread::spawn` that sends a
typed message home.

- New message: `Msg::ListingDone { epoch: u64, dir: PathBuf, result: std::io::Result<DirListing> }`.
- **The epoch counter is process-global, NOT a `FileBrowser` field.** A `static LISTING_EPOCH:
  AtomicU64`; every open, descend, and close takes `fetch_add(1)`, and `FileBrowser` stores only the
  epoch value it is currently awaiting.

  This placement is load-bearing and the reason is an **ABA bug**: if the counter lived inside
  `FileBrowser`, closing the picker would *drop* it, and a freshly-opened picker would start counting
  from the same value — so a stale result from the previous picker's still-in-flight listing could
  carry an epoch that matches the new picker's, and be accepted. The listing would be for the old
  directory. A process-global counter never reissues a value, so the match is exact and unforgeable.
  (Fast listings hide this: the window only opens when a listing outlives the picker that started it,
  which is exactly the hung-mount case §6.3 exists for. The test below closes that gap deliberately.)
- The merge **discards any result whose `epoch` is not the value the active picker is awaiting, and
  discards unconditionally when there is no active picker.** Both halves are required: the
  no-active-picker case is what makes a result arriving after a close inert, and without it the first
  half would have nothing to compare against. `dir` is diagnostic and for the
  merge-targets-what-it-thinks assertion; it is **not** part of the discard condition, because the
  epoch alone is sufficient and two listings of the same directory must still be distinguishable.
- Required test (drives the ABA case directly): open the picker, start a listing, close the picker,
  reopen it, then deliver the first listing's result — it must be discarded, and the reopened
  picker's contents must be untouched.
- The overlay stays fully closable while a listing is in flight. Closing bumps the epoch; the
  detached thread's result is discarded on arrival. A stuck mount strands one short-lived thread,
  never the UI.
- If a listing has not returned within a short threshold, the status line shows `Listing…` — the
  no-silent-UI-wait rule. `dir` is carried in the message for diagnostics and for the assertion that
  the merge targets what it thinks it does.
- Descend keeps its readability pre-check semantics: `fb.dir` is only mutated when a listing for the
  target **succeeds** (`enter_on_unreadable_dir_stays_put_and_sets_status` stays green, with the
  status now arriving via the merge rather than inline).

Helix's refinement (attempt the walk inline for ~30 ms, spawn only if it overruns, avoiding
thread-spawn overhead for small directories) is a **nice-to-have**, explicitly optional for C5. The
load-bearing parts are the epoch check and the not-on-the-durability-queue rule.

**Idle-is-free check:** the listing thread is spawned by a user action, runs once, sends one message,
and exits. No polling, no timer, no work at rest.

---

## 7. The picker

### 7.1 One overlay, two modes

`FileBrowser` gains a mode:

```rust
pub enum BrowseMode {
    /// Choose an existing file to open.
    Select,
    /// Choose a destination path (dir + filename) for a write.
    Destination { purpose: DestinationPurpose, field: String, field_cursor: usize },
}

pub enum DestinationPurpose { SaveAs, WriteBlock, Export { ext: String } }
```

No new `OverlayId` variant (§4.6). The intercept, painter, mouse fn, and `chrome_geom` hit-testing stay
single-sourced and branch on mode where behavior genuinely differs. `DestinationPurpose` is what the
commit path dispatches on, so adding a future destination consumer is one variant plus one arm the
compiler demands — the registration-seam shape, not a new hub.

`field_cursor` exists because destination mode needs UTF-8-codepoint-safe text editing, which
`minibuffer.rs` already solves (`insert`/`backspace`/`left`/`right`). **Reuse that logic** rather than
re-implementing it; if it cannot be called directly, extract the cursor arithmetic into a shared
helper rather than writing a second copy. A second hand-written UTF-8 cursor is a defect generator.

### 7.2 Destination mode — commit semantics (the precision section)

This is the highest-risk design surface in C5: it is the only place where a wrong decision produces
the exact harms the effort exists to eliminate — **silent overwrite, save-to-nowhere, hung quit**. The
semantics are therefore specified exhaustively here, not left to the plan.

**The field is dual-duty.** Its content is simultaneously (a) the filename-to-be and (b) a live filter
over the listing. Typing `chap` narrows the list to existing chapter files — so the writer sees what
they might collide with *while* naming the new file. Overwrite awareness for free.

**Navigation keys move the selection highlight only.** The six shared nav keys route through
`list_window::list_nav_key` / `apply_list_nav` exactly as in select mode. Nav never edits the field.
Printable characters, `Backspace`, and `Event::Paste` edit the field (and therefore re-filter);
they never move the selection except to clamp it into the new filtered list.

**Enter — the decision table.** Evaluated top to bottom; first match wins.

| # | Condition | Action |
|---|---|---|
| 1 | The highlighted entry is a **directory** (including `..`) | **Descend.** Field is preserved across the descend (the writer keeps their filename while navigating). Selection resets, listing re-fetched, epoch bumped. |
| 2 | Field is empty and the highlighted entry is a **file** | **Commit to that file** (explicit overwrite of an existing file). Goes through the overwrite-confirm prompt, which is what makes this safe. |
| 3 | Field, resolved against `fb.dir`, **names an existing directory** | **Descend into it**, clearing the field. |
| 4 | Otherwise | **Commit `fb.dir + field`**, through extension policy (§8), then the existing overwrite-confirm if the resolved target exists. |

Row 3 is the one genuinely ambiguous case, and it resolves **toward descend**. Rationale: a directory
named `chapter-one` sitting visibly in the list while Enter silently creates a *file* named
`chapter-one.md` beside it is the worse surprise, and the writer who wanted the file can get it by
typing one more character (`chapter-one.` or a different name) or by descending and naming it there.
Descend is also recoverable in one keystroke (`..`); a misplaced file is not.

**Row 2 versus the click divergence (§13.2, law 5) — not a contradiction.** Row 2 lets a keyboard
`Enter` commit onto a highlighted existing file when the field is empty, while a *click* on that same
file never commits. The distinguishing property is not the target but the number of deliberate acts:
reaching row 2 requires navigating the highlight there and pressing `Enter` with a visibly empty
field, and it still raises the overwrite-confirm. A click is a single act that can land anywhere the
pointer happens to be, and in destination mode there is no second act between it and a write. The
design rule both follow is the same — **no single unconsidered action reaches an overwrite** — and
they differ only because a click is the one input that can be unconsidered.

**Selecting an existing file copies its name into the field.** A dedicated key (proposed `Tab`) on a
highlighted *file* replaces the field content with that file's name. This is the deliberate two-step
overwrite gesture: highlight, `Tab` (see the name land in the field and the footer show the resolved
target), `Enter` (see the overwrite-confirm). Overwrite is never one accidental keystroke, and is
never reachable without the target being visible first.

**Esc** cancels, clears the field, closes the overlay, and — critically — must run the same
cancellation cleanup the current minibuffer path does (§11.2).

### 7.3 The resolved-target footer

Destination mode renders, live on every keystroke, the **absolute resolved target after extension
policy**:

```
→ /home/km/drafts/chapter-one.md
```

This is the single highest-value writer-facing element in C5 and it is non-optional. It removes the
entire class of "I saved it but I don't know where." It shows the post-policy name, so the `.md` that
policy appends is visible *before* commit, not discovered afterward. When the resolved target already
exists, the footer says so inline (e.g. a trailing `(exists — will confirm)`), so overwrite is
telegraphed one step before the confirm prompt rather than sprung by it.

**Field resolution — deliberately NOT `prompts::expand_path`.** The picker needs its own rule, because
`expand_path` joins relative input onto `std::env::current_dir()`, which is invisible to a writer
looking at a directory listing. Resolution, in order:

1. `~/`-prefixed → home-relative (same as `expand_path`).
2. Absolute → as typed (same as `expand_path`).
3. **Otherwise → joined onto `fb.dir`, NOT onto cwd.**

Rule 3 is the divergence and it is the whole point: the writer is looking at `fb.dir`, so `chapter.md`
must mean "here." Joining onto cwd would put the file somewhere the picker never showed them — the
save-to-nowhere class §7.2 exists to prevent, and it would make the resolved-target footer the only
warning. It also keeps the Enter table coherent: §7.2 row 4 commits `fb.dir + field`, and rows 3 and 4
both resolve the field the same way.

Two consequences worth stating: a relative path with segments (`drafts/ch1.md`) resolves under
`fb.dir`, which is what a writer navigating a tree expects; and `prompts::expand_path` keeps its
current cwd-joining behavior for any caller that still uses it — this spec adds a picker rule, it does
not change `expand_path`. A test asserts the divergence directly, with `fb.dir` and cwd set to
different directories so a regression to `expand_path` cannot pass.

Geometry note: the footer and the existing windowed indicator both want the block's bottom edge.
`render_overlays::paint_file_browser` currently uses `block.title_bottom(...)` for
`windowed_indicator`. The two must be composed deliberately (the resolved target is more important
than the scroll indicator and should win the position if only one fits); whichever arrangement is
chosen, `chrome_geom::file_browser_row_at` must be updated in lockstep so mouse hit-testing and the
painter stay single-sourced — the property that module exists to preserve.

### 7.4 Filters — two orthogonal toggles, mode-aware, never silent

**Clutter toggle** (`show_clutter` on/off; default **hidden**): withholds dot-prefixed names and a
fixed set of VCS/system directory names (`.git`, `.hg`, `.svn`, `.jj`, `.pijul` — matched by name, and
withheld even though they are already dot-prefixed, so the list stays honest if the dotfile rule ever
changes). **No gitignore semantics** (decision 2): they carry near-zero value for this audience and a
real hazard — a manuscript under an aggressive ignore file would vanish.

**File-type toggle** — a two-state option `FileTypeFilter { Documents, All }` (default
**`Documents`**), not a bool, so it matches the `MenuMark::Value("Documents" | "All files")`
representative and the two set-per-state commands in §13.1. It is **mode-aware**:

- **Select mode** lists what `file::open` can actually open — text formats: `.md`, `.markdown`,
  `.txt`, `.rst`, `.text`, plus extensionless files. It deliberately does **not** list `.docx`/`.pdf`,
  because there is no import path and `file::open` refuses them as `OpenError::Binary`; listing them
  would build a select-then-error dead end. (The grounding packet's "pandoc-ingestible" rationale was
  an error and is not carried forward.)
- **Destination mode** additionally shows output-format siblings (`.docx`, `.pdf`, `.html`, `.tex`),
  because in a destination context those are exactly the files you need to see in order not to
  clobber them.

Directories are never withheld by the file-type filter in either mode — a filter that hides the path
to your file is a filter that lies. **Broken symlinks are likewise never withheld by either toggle**
(§7.5): hiding one would leave a writer unable to see why their file appears to be missing, which is
the same lie in a different costume.

**Disclosure.** Whenever either toggle withholds anything, the footer carries a count:
`3 hidden (clutter), 12 hidden (type)`. Combined with the listing-cap disclosure (§6.2), the invariant
is: **the sum of shown + disclosed-withheld always equals what is really there** (up to the listing
cap, which is itself disclosed). A test asserts this arithmetic rather than asserting the strings.

**Ranking.** With a non-empty query/field the filtered list is ranked by `palette::fuzzy_filter`
(nucleo-matcher, already a direct dependency) instead of today's substring match, bringing the browser
in line with the palette and outline. Directories continue to sort before files within equal-relevance
groups, and the synthetic `..` stays pinned first.

### 7.5 Symlinks in the listing — classification and display (decision 10 — fixes §4.9)

`FileEntry` grows to mirror `DirEntryInfo` (§5.2): `{ name, kind: EntryKind, is_symlink, broken }`.
The picker never calls `file_type()` itself; it consumes what the seam resolved, and it **matches
exhaustively on `kind`** rather than testing for "is it a directory," so `Other` and `Unknown` cannot
fall into a branch meant for files:

| `kind` | Sorts with | Enter |
|---|---|---|
| `Dir` | directories | descends |
| `File` | files | opens |
| `Other` (fifo/socket/device) | files, marked | **refused** — `file::open` on a fifo would block |
| `Unknown` (incl. every `broken` symlink) | files, marked | **refused** — we do not know what it is |

`Other` and `Unknown` share the refusal but not the reason, and the status says which: an unopenable
special file versus an unresolvable entry. Both are **shown**, never hidden — same disclosure law as
everything else in §7.4.

**Symlinked directories behave as directories — this is the fix.** They sort with the directories,
render with the directory affordance, and Enter descends. `file_browser_enter`'s existing `is_dir`
branch and its `read_dir(&target).is_ok()` pre-check both then work unmodified (§4.9): the descend
path was always correct and only the classification feeding it was wrong.

**Broken symlinks are shown, marked, and refused — never silently hidden.** An unresolvable link
(`broken == true` — dangling, permission-denied, or looping; §5.2) sorts with the files, renders with
an explicit broken marker, and on Enter is
**refused with a status naming the condition** rather than dispatched to `open_as_new_buffer`. Today
`unwrap_or(false)` renders such an entry as an ordinary file that fails on open with a confusing
error; a writer whose symlink target moved should be *told* that the link is broken. The refusal is a
**Sticky Warning** (recoverable, user-visible, must survive the next keystroke), matching the
`enter_on_unreadable_dir_stays_put_and_sets_status` precedent for the sibling "you cannot go there"
case. Broken links are never hidden by the clutter or file-type filters — hiding a broken link is
exactly the silent lie §7.4 forbids, and it would leave the writer unable to see why their file is
missing.

**Render markers.** The label suffix follows the `ls -F` convention, composed with the existing
trailing slash:

| Entry | Label |
|---|---|
| Directory | `name/` |
| Symlink to a directory | `name/@` |
| Regular file | `name` |
| Symlink to a file | `name@` |
| Broken symlink | `name@ (broken)` |

These are **text suffixes, not colors**, so they survive terminal-plain / no-color mode — the
project's standing constraint on every affordance. This also restores the trailing `/` that §4.9's
third consequence was silently dropping.

**No cycle detection, and why.** Symlink loops are a real hazard for *recursive* walkers; they are not
one here. C5 navigates **one level at a time** (decision 7): `read_dir` does not recurse, every
descend is a deliberate keystroke, each listing is capped (§6.2), and `..` always escapes — it walks
the *logical* parent, so a writer who descends into a loop leaves by exactly the path they came in on.
The worst outcome is a writer noticing they are somewhere familiar and pressing `..`. Cycle detection
would add persistent per-session state to defend against an inconvenience the interaction model
already bounds. **If S2 adds recursive traversal, it must revisit this** — that is where loop
protection genuinely earns its cost, and this paragraph is the note S2 should read.

**Two behaviors that look like bugs and are correct — do not "fix" either.**

1. **`fb.dir` is deliberately not canonicalized.** After descending through a symlink, `.parent()`
   returns the **logical** parent — where the writer actually came from — not the target's real
   parent. Canonicalizing `fb.dir` would teleport `..` somewhere the writer has never been. The
   current code's use of the raw `fb.dir` is right.
2. **Swap and session state deliberately DO canonicalize.** `swap::swap_path` derives its filename
   from `std::fs::canonicalize(p)`, and `session_restore::persist_session` / `restore_resume` key
   `SessionState.entries` on `canonicalize(path)`. So one document reached via a symlink path and via
   its real path shares **one** swap file and **one** session entry — which is the data-safety-correct
   answer (it is one document; two swaps for it would be a recovery hazard). The asymmetry with point
   1 is intentional: navigation is about where the *writer* is, durability is about which *file* this
   is. **This unification is a property of the session and swap keys only — it does NOT extend to
   `DocumentId`.** C5 mints and stamps the id and reads it nowhere (§12.1), so a document reopened by
   either route mints a fresh id; ids do not follow canonical identity across routes or across
   restarts. An earlier draft claimed the shared session entry gave both routes one `DocumentId` "for
   free," which cannot be true while nothing reads the stamped value. §12.6 records what S3 must
   specify to make it true.

---

### 7.6 Symlinked files — making saves work (fixes §4.10)

#### 7.6.1 Write-destination resolution — the whole fix

Every path that will be **written** is resolved through symlinks before it reaches `save_atomic` /
`save_atomic_bytes`. One shared helper, applied at all four write-destination boundaries — Save,
Save-As, Write-Block, and the Export destination — so a writer who navigates through symlinks cannot
pick a destination that fails at the end of a save they thought would work.

This is the **entire** fix for §4.10. `Document.path` is not touched; §7.6.2 records why.

Resolution rule, stated once:

- Target is **not** a symlink → unchanged.
- Target is a symlink that **resolves** → write to the resolved target. The link is preserved
  (`atomic_replace` renames over the *target*, never the link), and `save_atomic`'s refusal stays a
  flat unconditional invariant that simply never fires on this path — a genuine last-resort guard, not
  a branch.
- Target is a **broken** symlink → **refuse before dispatching any write**, with a Sticky Warning
  naming the condition ("destination symlink cannot be resolved" — not "target is gone", since
  `broken` also covers permission and loop failures, §5.2). It must not fall through to a
  `SaveError::Symlink`, which describes the mechanism rather than the problem, and must not be allowed
  to reach `atomic_replace` at all.

  **Detected via `FileStat::broken`** (§5.2) — `canonicalize` cannot serve here, because it fails
  identically for a broken link and for the ordinary case of a destination that does not exist yet.
  Distinguishing those two is the entire reason `stat` carries the field, and getting it wrong would
  either refuse every new-file Save-As or let a broken link reach the write.

**The overwrite-confirm prompt names the RESOLVED target**, because that is the file whose bytes will
be replaced. When resolved and typed paths differ, the prompt shows both — confirming an overwrite of
a file you were not shown is precisely the accident §7.2 exists to prevent. Existence checks feeding
the confirm (`target.exists()` in `prompts::save_as_submit` / `block_write_submit`, and
`export::run_export`) test the resolved target, since `exists()` follows links and would otherwise
disagree with what gets written.

The resolved-target footer (§7.3) likewise shows the resolved path when it differs from the typed one,
so resolution is visible *before* commit rather than discovered in a confirm dialog.

#### 7.6.2 `Document.path` stays as-opened — the rule, and the evidence for it

**Decision 11 (revised): `Document.path` holds the path the writer opened.** It is not canonicalized,
and no display-path field is added. All symlink resolution happens at the write boundary (§7.6.1).

An earlier version of this decision canonicalized `Document.path` and carried a separate as-opened
path for display. It was reversed on the evidence below. The reasoning is recorded in full — not just
the verdict — because the principle generalizes and a future effort will face the same question.

**The rule: resolve at the point of use, not on the buffer.**

> Navigation and display answer *"where is the writer?"* — the logical, as-opened path.
> Durability answers *"which file is this?"* — the resolved path.
> Each subsystem resolves for itself, at the moment it needs the answer.

This is the same rule §7.5 states for directories (`fb.dir` stays logical so `..` returns where the
writer came from, while swap and session canonicalize for themselves). Applying it to files as well
makes one rule, not two. Canonicalizing `Document.path` would have been an *exception* to it.

**Argument 1 — the buffer path has seven consumers, and canonicalizing breaks all seven.** Every
`Document.path` consumer in the shell crate was enumerated. This table is the evidence, and it doubles
as a **map of what would break if someone later canonicalizes `Document.path` "for consistency"**:

| Consumer | Today (as-opened) — correct | If canonicalized |
|---|---|---|
| `workspace::buffer_display_name` (title, buffer switcher) | `today.md` | `18.md` — wrong name in the title |
| `prompts::open_save_as` prefill | `~/notes/` | `/archive/2026/07/` — prefills a directory the writer never chose |
| `blocks_marked::block_write` prefill | `~/notes/` | same regression |
| `registry.rs` `"open"` command dir seed | `~/notes/` | opens the browser in the archive tree |
| `export::run_export` → `derived_export_path` | `~/notes/today.pdf` | `/archive/2026/07/18.pdf` — **the export silently lands somewhere the writer never asked for** |
| `plugin::api` `wc.path()` + `PluginEventKind::{Open, Save, Change, BufferClose}` payloads | as-opened | canonical — a behavior change to a shipped, documented plugin API |
| `diagnostics_run::dispatch_one` LSP URI | as-opened | canonical — protocol-visible change |

The export row is the sharpest: the writer exports a PDF and it appears in a directory they have never
opened. Nothing tells them where it went.

**Argument 2 — the subsystems it would have "brought into agreement" already canonicalize at their own
point of use, so they gain nothing.** `swap::swap_path` and `swap::build_header` call
`std::fs::canonicalize` internally; `session_restore::persist_session` and `restore_resume` key
`SessionState.entries` on `canonicalize(path)`. None of them consults `Document.path` for identity.
The premise that canonicalizing the buffer path would unify them was simply false — they were never
divided. (F5's `DocumentId` is likewise unaffected, but for a different reason: C5 reads the id
nowhere, so canonicalizing `Document.path` would not have changed id behavior either way. See §12.6 —
the id is not yet an identity anything relies on.)

**Argument 3 — strictly less machinery.** Write destinations are not `Document.path`, so §7.6.1's
helper is required either way. Canonicalizing the buffer path would have added a second mechanism on
top of it — a new `Document` field, a `display_path_or_path()` fallback, seven rerouted consumers, plus
rules for whether the display path persists and when it clears. Resolving at the write boundary needs
one mechanism and answers those questions by never raising them.

**Argument 4 — the failure mode is asymmetric.** Under the boundary approach, a missed write site
fails **loudly and locally**: the save is refused with `SaveError::Symlink`, which is the behavior we
have today and a bug report, not a silent loss. Under the canonical-buffer approach, a missed consumer
fails **silently**: a file quietly written or displayed in the wrong place, which is the §4.9 shape
repeating — a field with more consumers than anyone counted.

**The honest case against this choice**, recorded so the reversal is not read as one-sided: a single
canonical answer on the buffer is conceptually simpler than "resolve at each boundary," and a future
subsystem that forgets to resolve gets the safe answer by default. Argument 4 is the reply — forgetting
to resolve fails loudly here — but the point is real, and it is why §7.6.1 puts the resolution in one
shared helper rather than open-coding it at four sites.

**Two consumers are unaffected either way and need no change:** `save::fingerprint` (uses
`std::fs::metadata`, which follows links) and `app.rs`'s `was_new_file` check (`Path::exists`, likewise
follows). Both already describe the target rather than the link.

---

## 8. Extension policy (F4-A: default-and-redirect)

A pure classification function over the field text, applied at commit and — in its
name-transformation half — live in the footer:

| Input | Verdict | Behavior |
|---|---|---|
| No extension (`chapter one`) | **Default** | Append `.md`. Visible in the footer before commit. |
| A recognized **output** extension (`.docx`, `.pdf`, `.html`, `.tex`), case-insensitive | **Redirect** | Refuse the save. Explain that this is an export format, and offer Export — carrying the typed path into the export destination picker so the writer's intent is not thrown away. |
| Any other extension (`.txt`, `.rst`, `.org`, …) | **Honor** | Silently accepted. |

Redirect is only defensible *because* export now has a destination (§9); before C5, "use Export
instead" was advice with nowhere to go.

Edge cases the classifier must handle explicitly, each with a test:

- **Dotfile-shaped names** (`.gitignore`, `.wordcartel.toml`): the leading dot is not an extension.
  Treat as *honor*, never append `.md` to produce `.gitignore.md`.
- **Case-insensitivity**: `.DOCX` redirects exactly as `.docx` does.
- **Trailing dot** (`notes.`): treat as no extension → `notes.md`; do not produce `notes..md`.
- **Multi-dot names** (`chapter.one.md`): only the final component is the extension.
- **Write-Block** uses the same policy. Its destination is an ordinary file and the same confusions
  apply.

The policy applies to **save destinations only**. It never applies in select mode, and never to the
export destination (whose extension is fixed by the chosen format).

---

## 9. Export gains a destination — without losing its best property

Today `export::run_export` derives the target via `derived_export_path(source, ext)` =
`source.with_extension(ext)` and prompts for nothing. That is a genuinely good property: export is
**zero-decision**. Adding a mandatory dialog would be a regression dressed as a feature.

**Enter-through (decision 4).** The export destination picker opens **pre-seeded**: `fb.dir` = the
source file's parent, `field` = the derived file name. A bare `Enter` therefore reproduces today's
behavior byte-for-byte, and the derived target is visible in the footer while doing so. Destination
*choice* is new capability; destination *obligation* is not introduced.

Everything downstream is unchanged: the existing overwrite prompt at dispatch (`pending_export` +
`Prompt::export_overwrite`), `do_export`'s thread, and `apply_export_done`'s TOCTOU re-check
(`overwrite_confirmed`) all keep their current semantics. The picker replaces only the *derivation* of
`target`, and only by pre-filling it.

The `probe_pandoc()` gate and the "save the file first before exporting" refusal for unnamed buffers
both stay ahead of the picker — there is no point choosing a destination for an export that cannot
run. Test `export_refuses_scratch_buffer` stays green.

---

## 10. Recents

`open_recent` opens the picker over a synthesized list sourced from `state::SessionState.entries` —
already a canonical-path-keyed `BTreeMap` with an LRU `seq` per entry — ranked by `seq` descending.
This is the rescue path for "I can't find my file," and it is nearly free because the data already
exists and is already maintained.

Details:

- Entries whose path no longer resolves are shown **greyed and are not selectable**, rather than
  silently dropped — a writer whose file moved needs to see that it is gone, not to find a shorter
  list. (Existence is checked via the seam's `stat`, on the listing thread, not inline.)
- Selecting an entry routes through the same open path as the browser
  (`workspace::open_as_new_buffer`), inheriting the dirty-guard and resume behavior.
- Recents is a *source* for the picker, not a fourth mode: it presents a flat list with the same nav,
  filter, and rendering. Fuzzy ranking applies to the path strings.
- **Favorites are explicitly deferred** (decision 3, §2.2).

---

## 11. Durability interactions

### 11.1 Save-As epilogue

#### 11.1.0 PREREQUISITE — `do_save_to` must carry two paths, not one

Everything below depends on this, and getting it wrong reintroduces either §4.10's defect or all seven
of §7.6.2's regressions.

Today `do_save_to(ctx, target, mode)` takes **one** path, and that single value fans out to four
distinct consumers: `write_path` (a clone) feeds `file::save_atomic` **and** `save::fingerprint` on the
worker; `target` itself feeds the `fire_save` plugin-event payload **and** the `b.document.path` rekey
in the merge. Under Middle B those four no longer want the same answer, because a Save-As destination
may be a symlink:

- If the single path stays **logical**, `save_atomic` receives a symlink and returns
  `SaveError::Symlink` — precisely the defect §7.6 exists to fix.
- If it is made **resolved**, the merge rekeys `Document.path` to the resolved target — contradicting
  §7.6.2 and reintroducing every consumer regression Middle B was chosen to prevent.

Neither is acceptable, so the parameter splits. `do_save_to` takes a `SaveTarget`:

```rust
pub(crate) struct SaveTarget {
    /// What the writer selected — logical, possibly a symlink. Middle B's coordinate system.
    pub chosen: PathBuf,
    /// Where bytes actually go — §7.6.1 resolution applied. Never a symlink.
    pub resolved: PathBuf,
}
```

A struct rather than two positional `PathBuf`s **on purpose**: two same-typed positional parameters
are silently swappable, and this is exactly the distinction that must not be gettable-wrong at a call
site. For a non-symlink destination the two fields are equal, which is the common case and costs
nothing.

**Which consumer gets which — the complete assignment:**

| Consumer | Gets | Why |
|---|---|---|
| `file::save_atomic` (worker) | **resolved** | This is what makes a symlink destination work at all (§7.6.1); it also means `save_atomic`'s symlink refusal never fires here and stays an unconditional last-resort guard. |
| `save::fingerprint` → `stored_fp` | **resolved** | The fingerprint must describe the file actually written. Note this is not a new asymmetry: `fingerprint` uses `metadata` and `bounded_read_opt`, both of which **follow** symlinks, so `fingerprint(chosen)` and `fingerprint(resolved)` agree whenever the link resolves. Using `resolved` keeps today's `fingerprint(&write_path)` structure unchanged and describes the bytes we wrote. It also stays comparable with `dispatch_save`'s `fingerprint(&Document.path)` external-mod check, since that call follows the link to the same file. |
| `b.document.path` rekey | **chosen** | Middle B (§7.6.2). Display and navigation stay logical; this is what keeps all seven consumers correct. |
| `SessionMigration { to }` | **chosen** | Choice and reason below. |
| `fire_save` plugin-event payload | **chosen** | Consistency with `plugin::api`'s `wc.path()`, which returns `Document.path` = chosen. A Save event reporting a path that `wc.path()` never returns would make the two disagree for any plugin correlating them. |
| `swap::delete(prior_key)` | **unchanged** | Dispatch-time `prior_key`, exactly as today (§11.1 note below). `swap::swap_path` canonicalizes internally, so the new path's swap key is unaffected by which field is used. |

**Why `SessionMigration { to }` is `chosen`.** Functionally either works — `persist_session` and
`restore_resume` both canonicalize their keys (`std::fs::canonicalize(raw_path)`), so a chosen symlink
path and its resolved target converge to the same `SessionState.entries` key. The tiebreak is internal
consistency of the struct: `from` is the buffer's pre-rekey `Document.path`, which under Middle B is
logical. Making `to` resolved would put two coordinate systems in one two-field struct for no gain,
and would invite a future reader to conclude the asymmetry is meaningful. Both fields are logical; the
migration helper canonicalizes on the way into the store, and that canonicalization is what makes the
choice safe rather than lucky.

**The overwrite-confirm is unaffected and must stay resolved.** It runs in
`prompts::save_as_submit`/`block_write_submit` *before* `do_save_to` is ever called, and §7.6.1
already requires it to name the resolved target — the file whose bytes will actually be replaced.
Nothing in this section overrides that. Likewise the completion status (item 2 below) names the
resolved write target, while the title keeps the chosen name; that split is stated in §7.6.2 and is
deliberate.

#### 11.1.1 The two fixes

Both in the `SaveMode::SaveAs` success branch of `save::do_save_to`'s merge:

1. **Migrate the session entry — recorded in the merge, applied in the run loop.** Move the
   `SessionState.entries` record from the buffer's **pre-rekey** path to the new target, preserving
   cursor/scroll/marks/folds/block and taking a fresh `seq`. Both endpoints are **logical/chosen**
   paths (§11.1.0) and both are canonicalized by the migration helper on the way into the store —
   which is what makes a symlinked destination converge on the same key as its target.
   ("Pre-rekey," not the dispatch-time `prior_key` — the distinction is load-bearing and is specified
   below.)

   **The merge cannot do this itself, and specifying it there would be unimplementable.** A
   `JobResult::merge` closure receives only `&mut Editor`, while the session store is a **local in
   `app::run`** (`let mut session = crate::state::load();`, alongside `session_seq` and `cfg`), and
   `registry::Ctx` carries no `SessionState`. So the work is split across the two places that each
   have half the data:

   - **In the merge** (`save::do_save_to`, `SaveMode::SaveAs`, the `Ok(Saved | Unchanged)` arm):
     **push** the intent onto a queue on the editor —
     `editor.pending_session_migrations.push_back(SessionMigration { from: pre_rekey, to: target })`,
     where `pre_rekey` is read from the buffer in the merge immediately before the rekey. No session
     access needed.

     **It must be a queue, not an `Option` slot.** `app::fold_and_continue` and the dispatch paths
     drain the executor in a **loop** (`for o in ex.drain() { apply_job_outcome(…) }`), so several
     ready save jobs can merge before `app::run` next reaches a persist point. Two Save-As jobs
     completing in one drain would overwrite a single slot and silently lose the first migration —
     the writer's marks on that document, gone with no error. A `VecDeque<SessionMigration>` makes
     the count independent of drain batching.
   - **In `app::run`**, where `session`, `session_seq`, and `cfg` are in scope: drain
     `pending_session_migrations` and apply them to `session` **before** `persist_session` flushes, so
     the migrated entries and any fresh record land in one write.

   **The drain trigger must NOT be the existing `sv != last_persisted_saved` condition alone — that
   condition is buffer-blind and would strand the migration.** `save::do_save_to`'s merge deliberately
   targets `editor.by_id_mut(buffer_id)` so a save lands on the right buffer even after the user
   switches away, but `sv` reads `editor.active().document.saved_version`. Save-As a document, switch
   buffers before the write completes, and the active buffer's `saved_version` never moves — the
   branch does not fire and the migration never drains.

   So a non-empty queue is **its own trigger**, independent of which buffer is active:

   ```
   let has_migrations = !editor.borrow().pending_session_migrations.is_empty();
   if has_migrations || sv != last_persisted_saved { … drain ALL, then persist … }
   ```

   Verification of the switched-away case: the merge pushes regardless of active buffer;
   `has_migrations` is read off the `Editor`, not off `active()`; so the branch fires on the next loop
   iteration whatever the user switched to. The condition is deliberately an `||` rather than a
   replacement — the `sv` half still governs ordinary saves, whose behavior is unchanged.

   **The `from` key is captured at MERGE time, not dispatch time.** This is load-bearing and the
   queue alone does not fix it. `do_save_to` binds `prior_key` at *dispatch*
   (`let prior_key = ctx.editor.active().document.path.clone();`) while `document.path` is only
   mutated later, inside the merge. So two **overlapping Save-As operations from the same source**
   — dispatch `a`→`b`, then dispatch `a`→`c` before the first merge lands — would queue
   (`a`→`b`, `a`→`c`), not (`a`→`b`, `b`→`c`). FIFO applies the first; the second finds `a` already
   absent and no-ops. **Session state ends at `b` while the buffer ends at `c`** — silently divergent.

   The fix is to read the buffer's path in the merge, immediately before the rekey. The merge's
   `Ok(Saved | Unchanged)` arm already holds `b: &mut Buffer` and performs
   `b.document.path = Some(target.clone())`, so the pre-rekey value is readable on the line above it:

   ```
   let pre_rekey = b.document.path.clone();          // merge-time truth
   if matches!(mode, SaveMode::SaveAs) { b.document.path = Some(target.clone()); }
   ```

   Under FIFO the `a`→`b` merge has already set `path = b` by the time the second merge runs, so it
   records (`b`, `c`) and the chain is correct **by construction** — no coalescing, no in-flight
   guard, no special-casing. It also makes the chained case (`a`→`b` then `b`→`c`) and the
   overlapping case produce the identical queue, which is why one mechanism covers both.

   Two conditions on recording, both natural consequences: if `pre_rekey` is `None` (first Save-As of
   an unnamed buffer) there is no old entry, so nothing is queued; if `pre_rekey == target` (Save-As
   onto the same path) the migration is a no-op and is not queued.

   **Deliberately NOT changed: `prior_key`'s other use.** The dispatch-time `prior_key` also feeds
   `swap::delete(prior_key.as_deref())` in the same arm. This spec leaves that exactly as it is —
   re-pointing it at `pre_rekey` would alter shipped swap-deletion behavior, which is a durability
   change needing its own analysis and is not what this fix is for.

   The consequence, stated accurately (an earlier draft of this paragraph got it wrong, and the point
   of recording it is that a reader can evaluate the tradeoff). The two branches differ:

   - **Clean branch** (`b.document.version == v`) deletes `swap(b.document.path)` — already rekeyed,
     so the *new* path — and then `swap(prior_key)`. After `a`→`b` that is `swap(b)` and `swap(a)`,
     both gone. So "`swap(b)` is left behind" is **not** true here.
   - **Still-editing branch** deletes only `swap(prior_key)` and sets `last_swap_at = None`, which is
     deliberate: the buffer is still dirty at the new path and wants a fresh swap promptly.

   The actual residue in the overlapping case (`a`→`b`, `a`→`c`) is narrow: merge 2's `prior_key` is
   still `a`, already deleted, so its second delete is a no-op — and if a swap happened to be written
   under the intermediate path `b` in the window between the two merges, nothing in merge 2 covers it.
   That requires the swap debounce to fire inside that window, so it is rare rather than routine; when
   it does occur the leftover is exactly the diverged-orphan class §11.3 surfaces rather than sweeps,
   so it is already accounted for. Noted so a reader sees it was considered, not missed.

   **The drain empties the whole queue**, applying migrations in FIFO order before the single
   `persist_session` flush, so N migrations recorded in one drain batch cost one write, not N. FIFO is
   required, not incidental: with merge-time capture each entry's `from` is the previous entry's `to`,
   so any other order strands the chain.

   **Drain at both persist sites.** `app::run` persists in two places: the in-loop branch above and the
   post-loop clean-quit persist. A shared helper is called at both, so migrations recorded by a save
   that completes on the final iteration are not lost at exit.

   **One inherited constraint governs everything below: `persist_session` records a per-file entry
   only for the ACTIVE buffer.** So the migration is not merely tidying a duplicate that would
   otherwise resolve itself — for a switched-away Save-As, migrating the old entry is the *only* thing
   that carries the writer's cursor, marks, and folds onto the new path. The fresh record for that
   path waits until the buffer is active at some later persist. Stating it precisely because an
   earlier draft of this section claimed the new path "already acquires an entry via that same
   branch," which is true only when the renamed buffer happens to be active, and reading it as general
   is what makes the migration look optional.

   **If the process exits between the two halves** — merge pushed, drain never ran — the queue is
   in-memory only and is lost. The outcome is the pre-C5 behavior: **the old path keeps its entry, and
   the new path has none until a later persist finds that buffer active.** No data loss, no corruption
   — the writer's *text* was written by the save itself; what is lost is session bookkeeping (their
   marks and cursor on that document). Acceptable because this is hygiene (§4.8), not a durability
   guarantee.

   Because it is hygiene, the drain is **best-effort**: a migration whose `from` key is already absent
   is a silent no-op, never an error and never a reason to fail a persist.
2. **Name the full path on completion.** The status becomes `Saved to {path}` /
   `Saved v{v} to {path} (still editing)` for Save-As. This closes the verified gap where a successful
   Save-As reports a bare `Saved`, indistinguishable from an ordinary save. Ordinary saves keep their
   current concise wording. `{path}` is the **resolved write target** (§7.6.1) — where the bytes
   actually landed, which is the point of naming it at all. When resolution changed the path, the
   writer sees the real destination rather than the link they typed. Note this is the one place the
   *resolved* path surfaces to the writer: the title keeps the as-opened name (§7.6.2) because it
   answers "which document am I in," while this status answers "where did it go." Different questions,
   different answers, neither a lie.

The `StatusTopic::Save(buffer_id, v)` progress-correlation key and `finish_topic` behavior are
unchanged; only the message text differs.

**Explicitly unchanged:** `swapped_version` clearing, `prior_key` swap deletion, the
`last_swap_at = None` expedite on the edited-during-write branch, and the path-aware latch in
`dispatch_swap_write`. `stale_path_swap_does_not_relatch_after_rekey` and
`save_clean_deletes_swap_but_stale_save_keeps_it` are merge gates.

### 11.2 NAMED HAZARD — the quit-drain coupling

**Any implementer migrating Save-As off the minibuffer will break this unless they are told.**

`save::dispatch_save_then` decides whether to arm `pending_save_as` by **inspecting the minibuffer's
kind**:

```rust
if ctx.editor.minibuffer.as_ref().map(|m| m.kind) == Some(crate::minibuffer::MinibufferKind::SaveAs) {
    ctx.editor.pending_save_as = Some(action);
}
```

When Save-As stops opening a `MinibufferKind::SaveAs`, this condition silently becomes false forever.
The consequence is not a compile error and not a visible bug in the common path — it is that
**save-and-quit on an unnamed buffer stops completing**: the write happens, `pending_after_save` is
never armed, and the quit the user asked for never fires.

The migration must replace the probe with an equivalent that asks the same question of the new state
("is a Save-As destination prompt now open for this buffer?"), preferably by having
`prompts::open_save_as`'s replacement return that fact rather than by having the caller sniff UI
state. Sniffing UI state to infer control flow is what made this fragile; the migration is the
opportunity to remove the sniff, not to relocate it.

Second half of the hazard: `prompts::save_as_submit`'s **empty-path arm** clears `pending_save_as` and
aborts the quit drain (`quit_drain = None; quit_drain_advance = false`) — the Effort-6 Codex-C2 fix.
Destination-mode **cancel (Esc)** and **empty-field commit** must both preserve this abort. Without
it, backing out of a drain's Save-As leaves `quit_drain` `Some`-but-inert: the drain is stranded with
no in-flight save and nothing to re-drive it. The same applies to the `Esc` cleanup in
`prompts::intercept`, which today clears `pending_export`, `pending_save_overwrite`,
`pending_save_as`, `pending_write_block`, and `pending_clean`.

Guard tests that must stay green, and be **extended to the new path rather than deleted with the old
one**:

- `prompts::tests::save_and_quit_on_unnamed_buffer_does_not_arm_pending_after_save`
- `prompts::tests::save_as_empty_path_is_a_sticky_warning`
- `prompts::tests::block_write_empty_path_is_a_sticky_warning`
- `save::tests::panicked_save_keeps_dirty_and_aborts_quit`

New tests required: save-and-quit on an unnamed buffer completing through the **destination picker**;
Esc-out of a drain's destination picker aborting the drain rather than stranding it.

### 11.3 Diverged orphans — visibility, never sweeping

A swap whose recorded realpath is never revisited (the file was renamed outside the editor) sits inert
in the state dir. It is tempting to sweep these. **We must not**, and the spec records why so the
temptation does not recur:

`swap::swap_is_cleanable` fails closed on `RecoveryDecision::Prompt` precisely because a diverged swap
**holds content that is not on disk at its recorded realpath**. It is the *most* recoverable object in
the state dir, not the least. `swap_is_cleanable_only_for_valueless_dead_pid_swaps` asserts exactly
this, and it is a no-data-loss guarantee.

C5 therefore adds **visibility only**: the `clean_recovery` modal reports, alongside the count it will
delete, the count it is **keeping because they may hold unsaved work** — with enough identifying
information (recorded realpath, timestamp) for the human to go extract or explicitly discard them. The
enumerator's inclusion rules, the `pending_clean` snapshot-as-ceiling discipline, and the
`recovery_path_still_cleanable` per-path re-verify are untouched.

This section is **severable**: under size pressure it can move to a follow-up without affecting
anything else in C5.

---

## 12. F5 — path-as-identity, resolved and recorded

This section exists to be read by **S3**. It records the argument, not merely the conclusion.

### 12.1 The three-part decision

1. **Swap file naming stays path-derived, permanently.** `swap::swap_path` will continue to derive
   the filename as `sanitize(basename)-fnv1a64(canonicalize(path))` (scratch: `scratch-{pid}.swp`).
2. **Identity contract (binding).** Any *new* per-document persistent state — **S3 snapshots
   foremost** — keys on a `DocumentId`, never on a path. Paths appearing in such records are
   display/forensic hints, not keys.
3. **Rider: mint and stamp, key nothing.** Mint a `DocumentId` at first durable association; carry it
   on `Document`; stamp it into `state::StateEntry` (a defaulted serde field) and into the swap header
   (an `id:` line). **Nothing reads it in C5.**

   **Minted with std only — no new dependency.** An earlier draft said "128-bit random," which
   silently contradicted decision 2: there is no `rand`, `getrandom`, or `uuid` in
   `wordcartel/Cargo.toml`, so that wording would have smuggled in a dependency the human excluded.
   Corrected: `DocumentId(u64)`, rendered as 16 hex digits, derived from
   `std::collections::hash_map::RandomState` — which is OS-seeded per instance — hashing a tuple of
   (process id, `SystemTime::now()` nanos, a process-local `AtomicU64` counter). `RandomState`
   contributes the entropy; the counter guarantees two ids minted in the same nanosecond still differ.

   64 bits is sufficient **because the id is a lineage hint, not a uniqueness invariant** (§12.6): a
   collision means two unrelated documents share a hint that nothing currently keys on, which is
   harmless, and it is not a security token so unpredictability is not a requirement.

   **Widening later is possible only if the persistence layer is written for it, so that is a
   requirement, not an assumption.** The id is stored as an **opaque hex string** in both formats —
   `Option<String>` in `state::StateEntry`, a text `id:` line in the swap header — and **nothing in the
   persistence layer parses it into a fixed-width integer**. C5 makes this easy to honor because C5
   reads the id nowhere at all; the requirement exists so a future reader is added without introducing
   a `u64::from_str_radix` that would silently cap the width. Under that constraint a wider id is a
   longer string and needs no format migration. **And the one thing S3 must not do is assume
   uniqueness** — §12.6 states the semantics for that reason, rather than leaving them to be inferred
   from the width.

### 12.2 Why swap stays path-derived

Recovery answers a **path-shaped question**: *"I am opening path X right now — is there unsaved
content for it?"* You arrive holding a path. A path-derived key is therefore the semantically correct
key, not a compromise forced by implementation. It also means recovery needs **no index at all**:
`swap::assess` computes exactly where the swap would be and looks there. That property — "works when
everything else is lost" — is the entire point of the crash lifeline and nothing in this or any future
effort should trade it away.

### 12.3 The controller's SPOF argument was wrong, and precisely how

The controller argued: a stable identity implies a path→id mapping on disk; that index becomes a
single point of failure for crash recovery; therefore keep path as the only key.

**The leap is invalid.** It only follows if the swap *filename* is rekeyed on the id — which nobody
needs and which §12.1(1) explicitly forbids. Identity does not require an index, because
**`swap::parse` ignores unknown header keys** (its key `match` ends in `_ => {}`; verified against
source). An `id:` line in the header is therefore:

- **index-free** — the header is already read at recovery time, so identity arrives with the artifact
  it identifies, self-describing;
- **forward-compatible** — an older binary reading a newer swap skips the unknown key and recovers
  normally;
- **backward-compatible** — a newer binary reading an older swap simply sees no id.

`SwapHeader` already carries `realpath` and `content_hash` for exactly this kind of self-description.
An id is one more field of the same character.

### 12.4 "Two identity models" is correct layering, not a cost

Swap answers a path-shaped question; snapshots and session state answer a document-lifetime question
(*"what history belongs to this document across renames?"*). Keying each subsystem on the question it
actually answers is not two models awkwardly coexisting — it is each layer keyed correctly. The swap
header carrying **both** `realpath` and `id` is the stitch between them if forensics ever needs one.

### 12.5 The SPOF logic inverts for snapshots — this is the part S3 needs

- **Path-keyed snapshots fail in ordinary operation.** Every Save-As — a normal, frequent act, and one
  C5 makes *easier* — silently orphans the entire checkpoint history. That is not an edge case; it is
  Tuesday.
- **Id-keyed snapshots degrade gracefully.** If each snapshot records `(id, realpath-at-capture,
  content-hash)` in its own header — mirroring what `SwapHeader` already does — then even a lost
  mapping leaves snapshots **listable and reassociable**. Annoying, never data loss.

So the very reasoning that correctly protects the crash lifeline argues *for* id-keying in the
snapshot subsystem. Same principle, opposite conclusion, because the failure requirements are
opposite.

### 12.6 Semantics of the id, and what is deliberately left to S3

The id is a **lineage hint, not a uniqueness invariant** — deliberately mirroring the existing law
that *path* is not a uniqueness invariant (the workspace permits the same path open in multiple
buffers; `swap::open_swap_paths` collects a `HashSet` precisely because collisions are normal).

C5's position, stated so it is falsifiable: **the id follows the buffer through Save-As**; a
stay-behind buffer at the old path re-mints on next durable touch.

**What the id is NOT, in C5 — stated because an earlier draft of this spec got it wrong.** Because C5
**mints and stamps but reads nothing** (§12.1 item 3), the id has no cross-session or cross-route
identity at all:

- A document closed and reopened **mints a fresh id.** The stamped value in `StateEntry` and the swap
  header is written, never read back, so nothing seeds from it.
- A document reached via a symlink path and via its real path does **not** thereby share an id.
  §7.5's unification is real but is a property of the *session and swap keys* (both canonicalize
  internally); it does not reach the id. An earlier draft claimed this sharing came "for free," which
  cannot hold while nothing reads the stamped value. The claim is withdrawn rather than repaired by
  adding a read point, because a seed/read path is exactly the scope creep decision 11 avoided.

So in C5 the id is a **stamp on artifacts**, not an identity anyone can rely on. That is the ratified
scope, and it is enough for the rider's actual purpose (§12.7): the contract is enforced in code and
ids accumulate in the artifacts S3 will read.

**S3's obligations — the concrete handoff.** To make the id load-bearing, S3 must specify:

1. **The read/seed point.** Where a document opening reads an existing stamped id instead of minting —
   the `StateEntry` field, the swap header, or both, and which wins when they disagree.
2. **Route convergence.** How a document reached by two paths (symlink and real, or two co-open
   buffers on one path) arrives at one id. The canonicalized `SessionState.entries` key is the natural
   join, but S3 must say so and handle the case where no entry exists yet.
3. **Divergence lineage.** What happens when one document becomes two — the Save-As stay-behind case
   C5 answers by re-minting, which S3 may need to answer differently once history hangs off the id.

These are recorded as deliberately unanswered rather than accidentally unconsidered. A more useful
handoff than a guarantee that was not true.

### 12.7 Honest accounting of the rider's value

Mechanically, S3 could add the id later at the same cost: both persistence formats are already
forward-compatible by construction (`StateEntry` is serde with defaulted fields — the existing
`folds` and `block` fields prove the pattern; `swap::parse` ignores unknown keys). There is no
migration debt being pre-empted, and this spec does not claim otherwise.

The rider's actual value is threefold: the identity contract becomes enforced-by-code rather than a
paragraph in an archive; session entries accumulate ids between C5 and S3, so S3 starts with a
populated mapping for every document touched in the interim; and it answers the human's stated wish
for "a good surface for future efforts like snapshots" without touching the crash lifeline.

---

## 13. Command-surface conformance

`docs/design/command-surface-contract.md` is binding. C5 touches commands, user-settable options, the
palette, the menu, and keybinding hints — so conformance is stated concretely here, and again in the
plan.

### 13.1 New and changed commands

| Command | Category | Notes |
|---|---|---|
| `open_recent` | File | New. Opens the picker over the recents source (§10). |
| `show_clutter_on` / `show_clutter_off` | — (palette-only) | Set-per-state primitives, `menu: None`. |
| `toggle_clutter` | **View** | Stateful representative, `register_stateful` with `MenuMark::OnOff(bool)` — see the note below. |
| `file_types_documents` / `file_types_all` | — (palette-only) | Set-per-state primitives, `menu: None`. |
| `toggle_file_types` | **View** | Stateful representative, `MenuMark::Value("Documents" \| "All files")` (both `&'static str`, as `MenuMark::Value` requires). |
| `save_as`, `block_write`, `export_*` | unchanged ids | Behavior rewired to the destination picker; **ids, labels, and categories are unchanged**, so no keybinding or palette entry the user has learned is invalidated. |

**Note on `MenuMark::OnOff` (raised at the spec gate as possibly invented — it is not).** The enum is
`pub enum MenuMark { OnOff(bool), Value(&'static str), Text(String) }` in `registry.rs`, and `OnOff`
has six production call sites there, every one of them the `state` fn of a `register_stateful`
boolean toggle: `toggle_measure` (`e.view_opts.measure`), `toggle_ventilate`
(`e.active().view.ventilate`), `toggle_wrap_guide`, `toggle_word_count`, the splash toggle
(`e.view_opts.splash`), and the caret-blink toggle (`e.caret_blink`). Registry tests assert on
`MenuMark::OnOff(true)` / `OnOff(false)` directly. So `OnOff` for a boolean toggle and
`Value(&'static str)` for a multi-state cycle is the established split, and C5 follows it exactly:
`toggle_clutter` is boolean → `OnOff`; `toggle_file_types` is two-state-with-names → `Value`.

### 13.2 Law-by-law

- **Law 1 (registry is the single source of truth).** All **seven** new commands are registered in
  `registry::Registry::builtins`. Nothing reads the toggles' state except through the shared setters.
- **Law 2 (every user-settable option is a command).** The two toggles are **persisted** options
  (**decision 8**), so they are added to `settings::SettingsSnapshot` and to the overrides serde
  mirror. This makes them subject to `settings::tests::every_persisted_setting_has_a_command`, whose
  compile-time exhaustive destructure of `SettingsSnapshot` will **fail to compile** until each new
  field is given a resolving command. That is the enforcement, and it is why persisting them is the
  right call rather than extra work: a writer sets "show all files" once and expects it to stick, and
  the contract then guarantees reachability. Both live in `MenuCategory::View` — they govern what the
  picker *shows*, not what it *does*, which is the View/File line the existing menu already draws.
- **Law 3 (palette exhaustive).** All **seven** commands are palette-reachable by construction —
  `palette::tests::palette_is_exhaustive_over_the_registry` and
  `palette_is_exhaustive_over_a_plugin_loaded_registry` gate it.
- **Law 4 (menu ⊆ palette).** Of the seven, **three** carry a `MenuCategory` — `open_recent`
  (`File`) and the two stateful representatives `toggle_clutter` / `toggle_file_types` (`View`) — and
  the **four** set-per-state primitives are palette-only (`menu: None`). Three plus four is the whole
  set; an earlier draft's "two representatives + four primitives" omitted `open_recent` and summed to
  six. Gated by `menu::tests::parameterized_plugin_command_and_plugin_list_satisfy_law3_law4`.
- **Law 5 (every mouse affordance has a keyboard path).** Destination mode keeps select mode's
  hover-to-highlight and wheel-scroll. **Click on a file in destination mode copies the name into the
  field** (the mouse equivalent of `Tab`, §7.2) and does **not** commit. Every destination-mode mouse
  action has a keyboard equivalent (`Tab`, nav keys, `Enter`), so law 5 holds.

  **The click divergence is deliberate and ratified (decision 9) — do not "fix" it.** Select mode's
  `mouse::mouse_file_browser` treats `Down(Left)` as select-**and**-commit, and that stays. Destination
  mode does not, because the two modes have asymmetric stakes: a mis-click in select mode opens the
  wrong file, which is free to undo — close the buffer. A mis-click in destination mode would land on
  the overwrite path for an existing file, and the cost of being wrong is somebody's manuscript. A
  future reader will notice the inconsistency between two modes of one overlay and be tempted to
  unify; the inconsistency **is** the safety property. A test asserts that a destination-mode click on
  a file mutates only the field and never dispatches a write.
- **Law 6 (one setter per option).** `Editor::set_show_clutter` and `Editor::set_file_type_filter` are
  the sole mutators; the set-primitives, the cycles, config seeding, and any future preset all call
  them. No call site writes the fields directly.
- **Law 7 (hints track the active keymap).** No new default keybindings are strictly required; any
  that are added resolve through the standard hint path.
  `keymap::tests::hints_reresolve_on_preset_switch` and
  `menu::tests::custom_bind_surfaces_in_menu_and_palette` gate it.
- **Law 8 (multi-state ⇒ set-per-state primitives + one stateful representative).** Both toggles
  follow the shipped `scrollbar_off` / `scrollbar_auto` / `scrollbar_on` + `cycle_scrollbar` pattern
  exactly, including `menu: None` on the primitives and `register_stateful` on the representative.
- **Law 9 (a preset is never the only door).** No preset is introduced.
- **Law 10 (commands are nullary).** All seven are nullary. The destination picker is opened *by* a
  command and gathers its argument interactively — it is not a parameterized command.

### 13.3 Registration order — one stale comment, one REAL constraint

C5 inserts seven commands into `registry::Registry::builtins`, so the ordering situation there needs
stating. It is not what the comments say.

**Stale — do not treat as a constraint.** Two comments in `registry.rs` assert that `save_settings`
must stay the last registered command ("toggle_canvas and toggle_chrome MUST be registered BEFORE
save_settings…", and "Registered BEFORE save_settings (Codex F4)… rely on save_settings staying
last"). Both are **out of date**: the tree already registers `plugins_reload` and `plugin_list` after
`save_settings`. Nothing depends on `save_settings` being last. Correcting or deleting these comments
is fair game for whichever task touches the registry — a comment asserting an invariant the code does
not have is worse than none.

**Live — this one is real and the plan must honor it.** The e2e journey
`journey_palette_end_reaches_last_command` presses `End` then `Enter` in the palette and asserts:

```rust
assert!(h.editor.borrow().status_text().starts_with("plugins:"),
    "plugin_list must be dispatched and write its inventory summary to the status line");
```

That hardcodes **`plugin_list` as the last registered command**. It is a merge-gate test, so
registering any of C5's seven commands *after* `plugin_list` breaks it. The stale comments are a
garbled memory of this constraint, which migrated to `plugin_list` when P2 added the plugin commands
and was never re-recorded.

**Therefore: C5's new commands register BEFORE `plugin_list`.** There is no reason to prefer the tail
— the palette orders by registration, and these belong near their File/View siblings anyway. If some
future need genuinely requires appending past `plugin_list`, that is a deliberate change to this e2e
test, not something to discover from a red gate.

### 13.4 Merge-gating invariant tests

`settings::tests::every_persisted_setting_has_a_command`,
`palette::tests::palette_is_exhaustive_over_the_registry`,
`palette::tests::palette_is_exhaustive_over_a_plugin_loaded_registry`,
`menu::tests::parameterized_plugin_command_and_plugin_list_satisfy_law3_law4`,
`menu::tests::custom_bind_surfaces_in_menu_and_palette`,
`keymap::tests::hints_reresolve_on_preset_switch`.

Plus the H21 overlay guardrail suite, which C5 inherits unchanged by not adding an `OverlayId`
variant — including `render_order_is_exactly_the_frame_overlays` and
`every_overlay_consumes_moved_without_panic_or_data_loss`.

---

## 14. Testing

**Scope enforcement.** `wordcartel/tests/fs_chokepoint.rs` (§2.3) — scans production sources for raw
filesystem access and fails on detected occurrences not in the clause-citing allow-list. It is a
**merge gate**. It is a high-coverage detector with the limits §2.3 states — it makes the scope claim
substantially harder to break by accident, and does not claim to make it unbreakable.

Its own coverage is asserted by the **four-sample self-check specified in §2.3** — one planted evasion
per detection route: a fully-qualified `std::fs::read`; a module containing `use std::fs;` plus a
short-form `fs::write`; an inherent dot-call `p.symlink_metadata()`; and an inherent UFCS call
`Path::metadata(p)`. All four must be detected. Fewer samples is insufficient by construction, since
each evasion class is invisible to the routes that do not target it; §2.3's table is the authority on
the exact samples. The self-check asserts the layers work on the spellings they target — it is **not**
evidence that the uncaught spellings in §2.3's gap list are caught.

**Seam and migration.** `FaultFs` promoted to `test_support` and extended with arms for
`read_capped` / `list_dir` / `stat`; fault coverage at each migrated call site.

**Injectability actually reaches the worker (§5.2 ownership).** The test that proves the seam
extension bought what it claims: dispatch a save with an `Arc<FaultFs>` injected at `Ctx`, and assert
the injected failure surfaces through the merge. It fails if the implementer hardcoded `RealFs` inside
the job closure — the compile-pressure shortcut this section exists to prevent — and it is the first
time the worker-side write path is fault-testable at all. The same shape covers the swap job and the
listing thread.

**`FileStat::broken` (§5.2).** A broken symlink reports `broken == true`, `is_symlink == true`,
`is_file == false`, `is_dir == false`; a path that does not exist at all reports `Err` (the ordinary
new-file answer), so the two remain distinguishable — without which §7.6.1's broken-destination
refusal cannot be implemented. `save::fingerprint` on a broken symlink returns `None`, matching
today's `metadata(path).ok()?` behavior exactly. **A permission-denied symlink chain also reports
`broken == true`** (the shared unresolvable definition), asserted so the two seam methods cannot drift
back to disagreeing definitions.

**`EntryKind` classification (§5.2).** One case per variant: a regular file → `File`; a directory →
`Dir`; a **fifo** → `Other` (not `File`, the rider-2 guard); a **symlink to a regular file** → `File`
via the resolved `m.is_file()`, proving `kind` is populated on the resolution branch and not only the
cheap one; a **broken symlink** → `Unknown` with `broken == true`.

**Case 2 — named but unclassifiable (§5.2).** The regression test for the category that had no home:
an entry whose `file_type()` probe fails is **emitted in `entries` with its name** and
`kind == Unknown`; it is **not** counted in `unreadable`, and it does **not** abort the listing. That
last assertion is the one that fails against a `?` in the resolution algorithm, where a single bad
entry would take the whole directory down with it. Paired with `plugin::load::discover` **reporting it
by name** when the name ends in `.lua`, which is what the name buys over a tally.

**Cap is opt-in (§5.2).** `plugin::load::discover` and the two `swap` scans call `list_dir` with
`cap: None` and retain every entry — asserted against a directory holding more than
`MAX_DIR_ENTRIES` items, which fails if a future change routes them onto the capped path. This is the
regression guard for decision 12's rider 3: a plausible plugin must not be droppable past a cap.

**Counter separation (§5.2).** In one listing containing both capped-out entries and unreadable ones,
`total_seen == entries.len() + unreadable + capped_out` holds, and the two disclosures render as
**separate** footer lines. It fails against a single conflated counter, which is what made the
cap/no-silent-drop conflict invisible.

**Plugin discovery follows symlinks (decision 12, §5.2).** A symlinked `.lua` file **and** a symlinked
plugin directory are both discovered and loaded — the two halves that disagree today. Paired with an
assertion that a real directory whose `init.lua` is a symlink still loads, so the fix converges the
inconsistency rather than inverting it.

**Plugin discovery drops nothing plausible (rider 3, §5.2).** One case per row of the audit table: a
broken symlink, a fifo named `x.lua`, and a `.lua` name that is not valid UTF-8 each appear in
`skipped` with a name; a `README.md` and an ordinary subdirectory appear **nowhere**, asserting the
"plausibly a plugin" qualifier actually bounds the report rather than flooding it. This is the test
that makes `discover`'s "never silently dropped" contract true rather than aspirational.

**`save_atomic_bytes` symlink guard (§5.2).** An export whose resolved target is replaced by a symlink
before the write is refused rather than writing through the link; the session-write path acquires the
same guard. Over-cap behavior for
each read (document-class and config-class). The dictionary append's new atomic path, including
symlink refusal and preservation of `append_word_to_dict_creates_parent_dir`.

**Listing.** Cache correctness (a query keystroke performs no `read_dir`) — asserted by counting
listing calls through the seam, not by timing. Cap + disclosure arithmetic: `entries.len()` ≤ cap,
`total_seen` is the true count, and the §5.2 invariant
`total_seen == entries.len() + unreadable + capped_out` holds — asserted directly, since it is what
keeps the cap disclosure and the unreadable disclosure from being confused for each other. A
non-interactive caller passing `cap: None` (discovery, the swap scans) retains everything, so
`capped_out == 0` and no plausible plugin can be dropped past a cap (§5.2, decision 12 rider 3). A
directory larger than the
cap reports the real total, which is the regression test for the capped-enumeration contradiction
(§5.2). `enter_on_unreadable_dir_stays_put_and_sets_status` stays green through the async migration.

**Epoch / ABA (§6.3).** A `ListingDone` for a stale epoch is discarded; a result arriving after the
overlay closed is discarded without panic; and the **close/reopen ABA case** — open, start a listing,
close, reopen, then deliver the first result — discards it and leaves the reopened picker untouched.
That last one fails against a `FileBrowser`-local epoch and passes against the process-global one,
which is the whole point of writing it.

**Session-entry migration siting (§11.1).** The Save-As merge pushes onto
`pending_session_migrations` and does **not** touch the session store; the drain applies the queue to
`session` before `persist_session` flushes. A migration whose `from` key is already absent is a silent
no-op.

**Two migrations in one drain batch (§11.1).** The regression test for the clobbering defect:
complete **two** Save-As jobs within a single `ex.drain()` loop, then drain — **both** migrations must
apply. It fails against an `Option` slot (the first is overwritten and silently lost) and passes
against the queue.

**Both multi-Save-As orderings (§11.1).** Two separate cases, because they fail for different reasons
and one mechanism must cover both:

- **Chained** — Save-As `a`→`b`, then `b`→`c` after the first merge lands.
- **Overlapping same-source** — dispatch `a`→`b`, then dispatch `a`→`c` **before the first merge
  lands**. This is the case dispatch-time `prior_key` capture gets wrong: it would queue
  (`a`→`b`, `a`→`c`), the second no-ops on an absent `a`, and session state ends at `b` while the
  buffer ends at `c`.

Both must end with exactly one session entry, at `c`, carrying the original cursor/marks/folds. The
overlapping case fails against dispatch-time capture and passes against merge-time capture, which is
why it is written as its own test rather than folded into the chained one.

**Session-migration drain is buffer-blind (§11.1).** The regression test for the trigger defect:
Save-As a document, **switch to another buffer before the write completes**, then deliver the merge —
the migration must still drain. It fails against a drain gated only on
`editor.active().document.saved_version` and passes against the `has_migration ||` condition, which is
why it is written this way. Paired assertions: the drain also fires at the post-loop clean-quit
persist, and an undrained migration lost to process exit leaves the old entry intact with no
corruption (the documented acceptable outcome, asserted rather than assumed).

**Field resolution (§7.3).** With `fb.dir` and the process cwd set to **different** directories, a
bare relative field resolves under `fb.dir`; `~/` and absolute inputs are unaffected. A regression to
`prompts::expand_path`'s cwd-join fails this test.

**`DocumentId` minting (§12.1).** Two ids minted back-to-back differ (the counter component), an id is
stable across saves, and minting introduces no dependency beyond std.

**Commit semantics.** One test per row of the §7.2 Enter table, plus: field preserved across descend;
`Tab` copies a file name and does not commit; field naming a directory descends rather than creating a
file; click-on-file in destination mode does not commit.

**Symlinks (§7.5).** These are regression tests for a live defect, so they are written to fail against
today's code:

- A symlink to a directory classifies `kind == Dir`, sorts with the directories, renders `name/@`,
  and **Enter descends into it** — the direct §4.9 regression, covering all three corrupted consumers.
- A symlink to a regular file classifies `kind == File` with `is_symlink == true`, renders `name@`,
  and opens normally.
- A broken symlink lists (never hidden by either filter), reports `broken == true` and
  `kind == Unknown`, renders `name@ (broken)`, and on Enter sets a **Sticky Warning** while dispatching
  no open and mutating no state.
- A fifo or socket classifies `kind == Other` — **shown and marked, Enter refused** (opening a fifo
  would block), and distinguishable from `Unknown` in the status, which is the pair of facts the bool
  model could not separate.
- `DirEntryInfo` invariants: `broken` implies `is_symlink`; `broken` implies `kind == Unknown`.
- Syscall economy (§5.2): listing a directory of N regular files performs **no** `metadata` calls
  beyond `read_dir` — asserted through a counting `Fs` impl, so a future naive "stat everything"
  refactor fails here rather than silently costing 5,000 syscalls.
- `..` after descending through a symlinked directory returns to the **logical** parent (the directory
  the writer came from), not the target's real parent — the §7.5 point-1 guarantee.
- A file reached via a symlinked path and the same file reached directly resolve to the **same**
  `swap::swap_path` and the same `SessionState.entries` key — the §7.5 point-2 guarantee, asserted so
  a future change to either canonicalization is caught.

**Symlinked files (§7.6).** Also regression tests against a live defect:

- **Opening a symlink to a file and saving it succeeds, and the symlink survives** — the direct §4.10
  regression. Asserted three ways: the save reports success, the link is still a link afterward
  (`symlink_metadata().is_symlink()`), and the *target* holds the new bytes.
- `file::save_atomic` still refuses a symlink handed to it directly — `save_through_symlink_refused`
  stays green **unmodified**, proving resolution happens before the guard rather than by weakening it.
- Save-As, Write-Block, and the Export destination each resolve a symlinked destination and preserve
  the link (all four write boundaries in §7.6.1, not just Save).
- **Save-As onto a symlink destination splits correctly (§11.1.0)** — the single highest-value test of
  this section, since one `SaveTarget` field going to the wrong consumer reintroduces either §4.10's
  defect or §7.6.2's regressions. After the save: the write landed on the **resolved** target and the
  link survives as a link; `Document.path` holds the **chosen** (symlink) path, so
  `workspace::buffer_display_name` shows the chosen name; `stored_fp` matches the written file and a
  follow-up `dispatch_save` raises **no** spurious external-mod prompt; the plugin `Save` event payload
  equals `wc.path()`; and the completion status names the resolved path.
- **Export after a Save-As onto a symlink targets the chosen directory** — the §7.6.2 regression
  tripwire applied to the rekey specifically, since `derived_export_path` reads `Document.path`.
- A **broken symlink chosen as a destination** is refused with a Sticky Warning before any write is
  dispatched, and never surfaces as `SaveError::Symlink`.
- The overwrite-confirm names the **resolved** target when it differs from the typed path (§7.6.1).
- **`Document.path` is unchanged by opening through a symlink** — it holds the as-opened path, and the
  title/buffer-switcher name (`workspace::buffer_display_name`) reflects it. This is the guard on
  §7.6.2's rule: it fails if a future change canonicalizes the buffer path, which is precisely the
  seven-consumer regression the table maps.
- **Export through a symlinked document targets the as-opened directory** —
  `export::derived_export_path` lands the output beside the link the writer opened, not beside the
  canonical target. The sharpest row of the §7.6.2 table, asserted directly so the regression cannot
  return silently.

**Extension policy.** Table-driven over the §8 cases including every edge case listed there.

**Durability.** Save-As migrates the session entry and reports the full path; the path-aware latch
regression test unchanged; save-clean-deletes-swap unchanged.

**Quit-drain (§11.2).** Save-and-quit on an unnamed buffer completes through the destination picker;
Esc-out of a drain's destination picker aborts the drain rather than stranding it; the three existing
empty-path Sticky-Warning tests extended to the new path.

**Identity.** `DocumentId` mints once and is stable across saves; round-trips through
`state::StateEntry` (including a pre-C5 `session.toml` with no id field deserializing to `None` —
mirroring `old_session_toml_without_folds_loads_with_empty_folds`); round-trips through the swap
header; **a pre-C5 swap file with no `id:` line still parses and recovers** (the backward-compatibility
claim in §12.3 asserted, not assumed).

**Integration.** An `e2e.rs` journey: open via picker → first save via destination picker (extension
appended, footer target correct) → export with destination (Enter-through) → Save-As to a new name
(status names the path) → reopen via `open_recent`.

**Budgets.** `file_browser.rs` will grow past its current size; it splits along the natural seam —
listing/filtering (pure), mode state + commit semantics, and the intercept — keeping
`wordcartel/tests/module_budgets.rs` and `clippy::too_many_lines` green.

**Advisory.** `scripts/smoke/run.sh` run and its one-line summary quoted verbatim in the pre-merge
report (mandatory-run, advisory-pass).

---

## 15. Risks

1. **Destination-mode commit semantics** — highest risk, retired by §7.2's exhaustive decision table
   and its per-row tests. A wrong resolution here produces silent overwrite or save-to-nowhere.
2. **Quit-drain coupling** — §11.2. Silent, not a compile error, and only manifests in save-and-quit
   on an unnamed buffer. Named as a hazard with its guard tests precisely so no implementer discovers
   it by breaking it.
3. **Broad migration across durability-critical code** — mitigated by the `&dyn Fs` convention with
   `RealFs` wrappers (§5.2): every call site stays source-compatible and the tree is green at each
   step. No behavior change is bundled into a migration commit except the dictionary append, which is
   called out and separately tested. The residual risk is lower than it first appears because the
   target shape is not novel — `settings::save_overrides` already runs it in production with a
   fault-injecting test, so the pattern is being extended, not introduced.
4. **Async listing staleness** — mitigated by the epoch check and by the overlay remaining closable
   mid-flight. Worst case is one stranded short-lived thread on a hung mount, never a stuck UI and
   never a blocked save (§6.3).
5. **Geometry drift between painter and mouse** — the footer changes the block's bottom edge;
   `chrome_geom::file_browser_row_at` must move in lockstep (§7.3). The existing single-sourcing is
   the mitigation; the risk is forgetting it.
6. **Symlink resolution cost regression** — the §5.2 algorithm is cheap only because it stats *just*
   the symlinks. A later refactor that "simplifies" it into `metadata()` on every entry would add a
   syscall per entry on the listing path. Mitigated by the counting-`Fs` economy test in §14, which
   exists precisely to fail on that refactor rather than let it ship as a silent slowdown.
7. **A missed write-destination boundary** (§7.6.1). If a write site is added later without routing
   through the shared resolution helper, a symlinked destination there is refused rather than
   resolved. This fails **loudly and locally** — `SaveError::Symlink` on the status line, today's
   behavior, a bug report rather than a loss — which is why this design was chosen over
   canonicalizing `Document.path` (§7.6.2, argument 4). Mitigated by putting resolution in one helper
   and by §14 asserting all four boundaries, not just Save.

   **The retired risk, recorded because it returns if anyone reverses §7.6.2:** canonicalizing
   `Document.path` would put seven consumers one missed reroute away from *silent* misbehavior —
   a file written or displayed in the wrong place with nothing surfaced. §7.6.2's table is the map of
   exactly which seven, and §14's buffer-path and export assertions are the tripwires.

**Size:** medium-plus, ~15 tasks, A17-class — clearly smaller than H1. The migration mass is wide but
shallow; the genuine difficulty is concentrated in §7.2 and §11.2.

---

## 16. Navigation depth — decided: one level (decision 7)

The picker navigates **one directory level at a time**, with filtering and fuzzy ranking within that
level. Recursive, project-wide find is **not** in C5; it belongs to **S2**. This was originally a
unilateral controller decision that the human had never been asked directly; it was put to them
against this spec and ratified.

Reasons it went this way: recursive search wants the `ignore` crate that decision 2 deliberately
removed; S2 owns recursive traversal and would otherwise build it twice; and `open_recent` (§10)
covers the writer scenario project-wide find would serve — "I know I was working on it, I just can't
find it" — with no traversal at all.

**The accepted cost, recorded rather than dropped now that it is decided:** a writer with a deep
folder hierarchy, looking for a file they have **never opened in wcartel** — so it is absent from
recents — has no fast path to it and must navigate down by hand, one level at a time. That gap is real
and it stays open until S2 lands. If it proves painful in practice before then, the remedy is a
bounded recursive find (depth-limited, entry-capped, disclosed per §6.2/§7.4) — not large, but not
free, and partially duplicative of S2. This paragraph exists so that a future reader hitting the
complaint recognizes it as a known, priced consequence rather than an oversight.

**No open questions remain. This spec is ready to gate.**

---

## 17. Judgment calls — three ratified, four open, one confirmation

Per the latitude granted, these are places I went beyond the decisions file. **No open questions
remain.** Items 1–2 and 8 were put to the human and **ratified** (item 8 by reversing an earlier
decision on new evidence); items 3–6 remain implementer/reviewer discretion and may be changed in the
plan without amending this spec; item 7 confirms latitude the decisions file already granted.

**RATIFIED — no longer open:**

1. **The two filter toggles are persisted settings** (§13.2, law 2) — **decision 8**. This adds
   `SettingsSnapshot` + the overrides serde mirror to C5's scope. Ratified precisely because the
   compile-time reachability gate is the enforcement mechanism, and because a writer expects "show all
   files" to stick.
2. **Click-on-file in destination mode does not commit** (§13.2, law 5) — **decision 9**. The
   inconsistency with select mode is deliberate and stakes-based; §13.2 records the justification so it
   is not later "fixed."

**OPEN — implementer/reviewer discretion:**

3. **`Tab` proposed as the copy-name-into-field key.** Not ratified; it is the least-loaded key that
   reads as "complete this." Easily changed.
4. **`MAX_CONFIG_BYTES = 1 MiB` and `MAX_DIR_ENTRIES = 5,000`** are proposed values, not ratified
   ones.
5. **The `read_capped` primitive returns `Result<Option<Vec<u8>>>`**, distinguishing over-cap from IO
   error, where the existing `file::bounded_read_opt` collapses both into `None`. This is a
   deliberate improvement at the seam; existing callers keep their degrade-silently behavior by
   discarding the distinction at the wrapper.
6. **`Msg::ListingDone` carries `dir`** in addition to `epoch`. Strictly, `epoch` suffices for
   staleness; `dir` is for diagnostics and for asserting the merge targets what it thinks it does.
**CONFIRMATION — not an open question:**

7. **The diverged-orphan visibility section (§11.3) is cleanly severable.** The decisions file already
   granted this under size pressure; I am confirming the separability holds as specified, and
   recommending it ships with C5 since it is small and completes the honesty story.

**RESOLVED — was an objection, now decided:**

8. **`Document.path` stays as-opened; resolution happens at the write boundary.** Decision 11
   originally canonicalized `Document.path` with a separate display path. I objected on evidence
   gathered after that choice was made — a sweep of every consumer — and the human **reversed the
   decision**. The coordinator noted that the original recommendation rested on a premise the sweep
   disproved: that resolving on the buffer would bring it into agreement with swap and session, when
   those already canonicalize at their own point of use and were never divided from it.

   The spec now specifies only the adopted design. The full argument lives in **§7.6.2** — the
   seven-consumer table, the already-canonicalizes finding, the strictly-less-machinery point, the
   loud-vs-silent failure asymmetry, and the honest case against — because the underlying rule
   (*resolve at the point of use, not on the buffer*) is reusable and a future effort should find the
   reasoning, not just the verdict. `scratchpad/c5-file-interface/decisions.md` records it as decisions
   10 and 11 so it also survives outside this spec.
