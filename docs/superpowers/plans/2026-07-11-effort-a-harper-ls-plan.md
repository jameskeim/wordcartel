# Effort A — harper-ls diagnostics provider: implementation plan

**Date:** 2026-07-11
**Spec (authoritative):** `docs/superpowers/specs/2026-07-11-effort-a-harper-ls-design.md`
**Grounded against:** `main` @ the spec-merge commit. Anchor on symbol NAMES; lines drift.
**Discipline:** `superpowers:writing-plans` — task-by-task, TDD (failing test → impl → green →
commit), complete code, one implementer subagent per task, dependency order.

---

## Global Constraints (every task obeys these)

**Command-surface contract.** Effort A adds no command, menu row, palette entry, keybinding hint,
or user-settable option; the provider is not (yet) a user-settable multi-option (spec §14). Every
task states its conformance line; most are "N/A — does not touch the command surface." T5 touches
`render_status` (display-only) and re-points existing command/dispatch plumbing without changing
any registration — it states conformance explicitly. The contract's invariant tests
(palette-completeness, every-option-has-a-command, hint re-resolution) must stay green.

**House style (GATE by review).** Hand-formatted dense style — **never run `cargo fmt`**. 4-space
indent; ~100-col hand-wrapped; snake_case fns, PascalCase types, SCREAMING_SNAKE consts; imports
grouped by hand; `—` em-dash in prose comments, never `--`; no emoji in code except multibyte test
strings (`é`/`中`/`🙂`). Private fields by default; newtypes for distinct primitives; typed error
enums to the status line (no console). No `.unwrap()` on fallible/external paths — guarded
`.expect("…invariant…")`. Doc-comment every public item.

**Empirical harper-ls facts (verified, harper-ls 2.1.0 stdio probe — spec §3.3/§8/§16). Bake in;
do not re-litigate:**
- **Config is a PULL.** harper delivers config only by sending `workspace/configuration` requests
  and awaiting the client's response. `didChangeConfiguration` PUSH alone delivers nothing. The
  PULL request arrives as `params.items = [{}]` (empty section); the client must answer with a
  **result array of BARE, UNWRAPPED settings objects** — `result: [{ dialect, userDictPath,
  linters… }]`, **NOT** `{"harper-ls": {…}}`. The PUSH payload, by contrast, IS nested under
  `"harper-ls"` and is only a trigger that makes harper re-pull. Without the pull responder,
  config silently defaults (American / all linters / no dictionary → every custom word flagged).
- **`untitled:` + `languageId="markdown"`** docs lint identically to `file://`, and `userDictPath`
  applies to them — so all docs use opaque generation-tagged URIs `untitled:wcartel-<id>-<gen>`
  (no `url` crate, no file-path derivation).
- **`PublishDiagnosticsParams.version` echo is `None`** on 2.1.0 → the generation embedded in the
  URI is the **load-bearing** stale-publish discriminator; the URI is echoed verbatim.
- **codeAction returns structured quickfix edits:** `CodeAction { kind:"quickfix",
  edit.changes:{<uri>:[{newText,range}]} }` with clean `newText` (map → `Suggestion::ReplaceWith`);
  command-only actions (`kind:None`, empty edit) are ignored (we do ignore/add-dict client-side).

**Latch invariant (spec §5.1).** `in_flight_version == Some(v)` ⟹ **at least one** terminal
`Msg::DiagnosticsDone` for `(buffer, v)` arrives (duplicates tolerated — idempotent under the
version/generation filter). Two halves: dispatch latches **only** on `Accepted::Yes`; the client
thread's `FlushGuard` guarantees a terminal for every accepted change on any exit (publish,
watchdog, crash, panic-unwind, channel-drain, `Cmd::Close`-before-remove).

**No-data-loss on the dictionary.** Our `append_word_to_dict` is the *sole* writer of
`dictionary.txt`; `editor.dictionary` (loaded at startup, unioned into the client-side ignore
filter) suppresses every saved word regardless of harper. harper reads the same file as
`userDictPath` and is nudged to re-read via `reload_dictionary()` (a config resend) — never a
second write.

**Resource behavior.** Idle is free: lazy spawn on entering Review (no startup thread); the client
pump blocks on `recv` when nothing is pending; every send is edge-triggered by an edit or explicit
command. The 8 MiB `DIAG_MAX_SEND_BYTES` cap bounds per-recheck stdio.

**GATEs (before merge).** `cargo test` green (core lib+oracle, shell lib); `cargo build` +
`cargo test --no-run` warning-free for touched crates; **workspace clippy clean**
(`cargo clippy --workspace --all-targets`, `all = "deny"`) — a `too_many_lines` function needs an
item-local `#[allow(clippy::too_many_lines)]` + one-line reason; **module budgets**
(`wordcartel/tests/module_budgets.rs`: app.rs ≤ 1000, render.rs ≤ 900, timers.rs ≤ 400 — new
behavior enters new modules, not the hubs); **backlog drift** (`wordcartel/tests/backlog.rs`).
PTY smoke `scripts/smoke/run.sh` is mandatory-run / advisory-pass. Commit trailers verbatim on
every commit.

**Mockability.** The `DiagnosticsProvider` trait is the seam; `NullProvider` (production default)
and `RecordingProvider` (`#[cfg(test)]`) mean no test needs harper-ls installed. All
harper-ls-touching tests are `#[ignore]`-gated and skip cleanly when the binary is absent.

**Ledger.** Track completed tasks in `$(git rev-parse --git-path sdd)/progress.md` — one line per
task with its commit range. Branch: `effort-a-harper-ls` off `main`.

---

## Task graph (dependency order)

```
T1 foundation (deps: none) ─┬─ T2 lsp_rpc pure conversions ─┐
                            └─ T4 seam+NullProvider+Msg variant+wiring ──┼─ T3 harper_ls client ─┐
                                                                        └───────────────────────┴─ T5 integration ─ T6 degradation+build ─ T7 H18 tail
```

T2 and T4 are independent of each other and can run in parallel after T1. **T4 introduces the
`Msg::DiagProviderEvent` variant, the `ProviderEvent` type, and a minimal `reduce_dispatch`
handler** so that T3 (which emits `Action::Emit(Msg::DiagProviderEvent(..))`) compiles — no task
forward-references a `Msg` variant a later task introduces. T3 depends on T2+T4. T5 completes the
wiring (full `apply_provider_event` in the handler + the `prompts::intercept` arm). T5 depends on
T3+T4. T6 depends on T5. T7 last.

---

## T1 — Foundation: new limits, `lib.rs` module decls, empty module stubs, R1 reconfirm harness

**Goal.** Land the crate scaffolding so later tasks compile in isolation, plus a guarded probe test
that reconfirms the **load-bearing** harper-ls facts on the packaged binary: the config PULL model,
the **unwrapped** `workspace/configuration` response shape, and **initial** `userDictPath`
application. (Round-1 MINOR 6 — honest scope: the probe does NOT verify dict *reload-on-resend*;
that remaining fact is backstopped by the client-side ignore filter, which hides dictionary words
regardless of harper re-reading, spec §7.4/§16, so it is low-risk and left unprobed. Do not claim
it as reconfirmed.)

**Files:** `wordcartel/src/limits.rs`, `wordcartel/src/lib.rs`, new empty
`wordcartel/src/diag_provider.rs`, `wordcartel/src/lsp_rpc.rs`, `wordcartel/src/harper_ls.rs`,
new `wordcartel/tests/harper_ls_probe.rs`.

**Command-surface conformance:** N/A — no command surface touched.

### Steps

1. **Add limits** — append to `wordcartel/src/limits.rs`:

```rust
/// Effort A: harper-ls `maxFileLength` — raise well above the 120 KB default so real
/// long-form documents are checked (the server silently skips longer docs otherwise).
pub const HARPER_MAX_FILE_LENGTH: u64 = 10_000_000;
/// Effort A: client-side cap on the text shipped per recheck over stdio (full-document sync).
/// Comfortably under the server's 10 M-char limit; proportional-to-work discipline, not a
/// correctness need — an over-cap document is skipped with a status and no in-flight state.
pub const DIAG_MAX_SEND_BYTES: u64 = 8 * 1024 * 1024;
```

2. **Declare modules** — in `wordcartel/src/lib.rs`, beside the existing `pub mod diag_overlay;`
   line, add (grouped with the diagnostics modules):

```rust
pub mod diag_provider;
pub mod lsp_rpc;
pub mod harper_ls;
```

3. **Create the three module files as compiling stubs** so `lib.rs` builds. Each carries only its
   module doc-comment for now (later tasks fill them):

`wordcartel/src/diag_provider.rs`:
```rust
//! The `DiagnosticsProvider` seam (Effort A): a thin, mockable trait behind which a diagnostics
//! backend runs. `NullProvider` is the hermetic default; `HarperLs` (harper_ls.rs) is the real one.
//! No merge/multi-provider machinery — harper is the only provider; the seam is Open-Closed
//! insurance for provider #2.
```

`wordcartel/src/lsp_rpc.rs`:
```rust
//! Pure/IO-light LSP plumbing (Effort A): Content-Length framing, JSON-RPC envelopes over
//! `serde_json::Value`, opaque document URIs, UTF-16→byte position conversion, and
//! codeAction `TextEdit`→`Suggestion` mapping. No process IO lives here — see harper_ls.rs.
```

`wordcartel/src/harper_ls.rs`:
```rust
//! The harper-ls client (Effort A, imperative shell): the `HarperLs` provider handle, the
//! long-lived client thread + `FlushGuard`, child spawn/respawn/shutdown, the pure `HarperState`
//! protocol state machine (incl. the `workspace/configuration` PULL responder), and eager-assembly.
```

4. **Add the `serde_json` + `lsp-types` deps** to `wordcartel/Cargo.toml` `[dependencies]`
   (alphabetical-ish with neighbors; no `url`):

```toml
lsp-types = "0.97"
serde_json = "1"
```

5. **TDD — the R1 reconfirm probe** (`wordcartel/tests/harper_ls_probe.rs`). This is an
   `#[ignore]`-gated integration test that drives the packaged/pinned `harper-ls` over stdio and
   asserts the two remaining facts; it is the executable form of the spec §16 reconfirm. Write it
   FIRST (it fails to compile until the deps land, then runs only under `--ignored` when harper-ls
   is on PATH). Keep it self-contained (raw stdio; it does not depend on `lsp_rpc`/`harper_ls`,
   so it stays valid even as those modules evolve):

```rust
//! Guarded reconfirm of the two remaining harper-ls facts (spec §16) against the packaged binary.
//! `#[ignore]` by default; run with `cargo test -p wordcartel --test harper_ls_probe -- --ignored`.
//! Skips (passes) cleanly when `harper-ls` is not on PATH.
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};

fn harper_on_path() -> bool {
    Command::new("harper-ls").arg("--version").stdout(Stdio::null())
        .stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

fn frame(v: &serde_json::Value) -> Vec<u8> {
    let body = serde_json::to_vec(v).expect("serialize");
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(&body);
    out
}

/// Read one Content-Length-framed JSON message from `r`.
fn read_frame<R: BufRead>(r: &mut R) -> serde_json::Value {
    let mut len = 0usize;
    loop {
        let mut line = String::new();
        r.read_line(&mut line).expect("header line");
        let t = line.trim_end();
        if t.is_empty() { break; }
        if let Some(n) = t.strip_prefix("Content-Length:") {
            len = n.trim().parse().expect("content-length");
        }
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).expect("body");
    serde_json::from_slice(&buf).expect("json")
}

#[test]
#[ignore = "requires harper-ls on PATH; run with --ignored"]
// Skip-diagnostic prints to stderr; the workspace denies clippy::print_stderr, so allow it here
// (item-local, house-style exception — an ignored probe's skip message is legitimate test output).
#[allow(clippy::print_stderr)]
fn config_pull_is_unwrapped_and_dictionary_applies() {
    if !harper_on_path() { eprintln!("skip: harper-ls not on PATH"); return; }
    // A temp dictionary containing "wcartelword".
    let dir = std::env::temp_dir().join(format!("wcartel_probe_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let dict = dir.join("dictionary.txt");
    std::fs::write(&dict, "wcartelword\n").unwrap();

    let mut child = Command::new("harper-ls").arg("--stdio")
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn harper-ls");
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    let settings = serde_json::json!({
        "dialect": "American",
        "userDictPath": dict.to_string_lossy(),
        "maxFileLength": 10_000_000,
    });

    // initialize (advertise workspace.configuration = true)
    stdin.write_all(&frame(&serde_json::json!({
        "jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"processId":std::process::id(),"rootUri":null,
            "capabilities":{"workspace":{"configuration":true,"didChangeConfiguration":{}},
                "textDocument":{"publishDiagnostics":{"versionSupport":true},"codeAction":{}}}}
    }))).unwrap();
    // Pump until we've answered a workspace/configuration pull, then opened a doc, then
    // observed a publishDiagnostics that does NOT flag the dictionary word.
    stdin.write_all(&frame(&serde_json::json!({"jsonrpc":"2.0","method":"initialized","params":{}}))).unwrap();
    stdin.write_all(&frame(&serde_json::json!({
        "jsonrpc":"2.0","method":"workspace/didChangeConfiguration",
        "params":{"settings":{"harper-ls":settings}}}))).unwrap();
    stdin.write_all(&frame(&serde_json::json!({
        "jsonrpc":"2.0","method":"textDocument/didOpen",
        "params":{"textDocument":{"uri":"untitled:wcartel-probe-1","languageId":"markdown",
            "version":1,"text":"wcartelword teh\n"}}}))).unwrap();

    let mut answered_pull = false;
    let mut saw_publish = false;
    let mut dict_word_flagged = true;
    for _ in 0..200 {
        let msg = read_frame(&mut stdout);
        if msg.get("method").and_then(|m| m.as_str()) == Some("workspace/configuration") {
            // VERIFY: request items are empty-section objects.
            let items = msg["params"]["items"].as_array().cloned().unwrap_or_default();
            assert!(!items.is_empty(), "configuration request has items");
            // Respond UNWRAPPED: result is an array of bare settings objects, one per item.
            let result: Vec<serde_json::Value> = items.iter().map(|_| settings.clone()).collect();
            let id = msg["id"].clone();
            stdin.write_all(&frame(&serde_json::json!({"jsonrpc":"2.0","id":id,"result":result}))).unwrap();
            answered_pull = true;
        }
        if msg.get("method").and_then(|m| m.as_str()) == Some("textDocument/publishDiagnostics") {
            saw_publish = true;
            let diags = msg["params"]["diagnostics"].as_array().cloned().unwrap_or_default();
            // "wcartelword" must NOT be flagged (dictionary applied); "teh" SHOULD be.
            let text = "wcartelword teh\n";
            dict_word_flagged = diags.iter().any(|d| {
                let s = d["range"]["start"]["character"].as_u64().unwrap_or(0) as usize;
                s < "wcartelword".len() && text.starts_with("wcartelword")
                    && d["range"]["end"]["character"].as_u64().unwrap_or(0) as usize <= "wcartelword".len()
            });
            break;
        }
    }
    let _ = child.kill();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(answered_pull, "harper-ls pulled config (PULL model confirmed)");
    assert!(saw_publish, "harper-ls published diagnostics after config answered");
    assert!(!dict_word_flagged, "userDictPath word must not be flagged (unwrapped pull response applied)");
}
```

6. **Green + commit.** `cargo test -p wordcartel` builds (the probe compiles, is skipped by
   default); commit `T1: harper-ls foundation — limits, module decls, deps, R1 reconfirm probe`.

---

## T2 — `lsp_rpc`: framing, URIs, UTF-16→byte, codeAction→Suggestion (pure)

**Goal.** All pure/IO-light plumbing, fully unit-tested without a process. This is the highest-yield
TDD surface (spec §3.3 URI, §6).

**Files:** `wordcartel/src/lsp_rpc.rs` (fill the stub; tests inline in a `#[cfg(test)] mod tests`).

**Command-surface conformance:** N/A.

### Steps (write each test first, then the impl, in this module)

1. **`doc_uri`** — opaque, generation-tagged, path-independent (spec §3.3):

```rust
use crate::editor::BufferId;
use wordcartel_core::diagnostics::Suggestion;

/// The opaque, generation-tagged wire URI for a document. Identical form for saved and unsaved
/// buffers — harper lints the sent text + `languageId`, not the file at any path, and the
/// embedded generation is the load-bearing stale-publish discriminator (spec §3.3, §5).
pub fn doc_uri(buffer_id: BufferId, generation: u64) -> String {
    format!("untitled:wcartel-{}-{}", buffer_id.0, generation)
}
```

Test: `doc_uri(BufferId(7), 3) == "untitled:wcartel-7-3"`; distinct generations → distinct strings.

2. **Content-Length framing** — `write_frame` / `read_frame`:

```rust
use std::io::{self, BufRead, Read, Write};

/// Serialize a JSON-RPC message and write it Content-Length-framed to `w`.
pub fn write_frame<W: Write>(w: &mut W, msg: &serde_json::Value) -> io::Result<()> {
    let body = serde_json::to_vec(msg).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    write!(w, "Content-Length: {}\r\n\r\n", body.len())?;
    w.write_all(&body)?;
    w.flush()
}

/// Read one Content-Length-framed JSON-RPC message. `Ok(None)` on clean EOF before any header;
/// `Err` on a malformed frame or a mid-frame EOF (the caller treats either as stream corruption).
pub fn read_frame<R: BufRead>(r: &mut R) -> io::Result<Option<serde_json::Value>> {
    let mut content_length: Option<usize> = None;
    let mut saw_any_header = false;
    loop {
        let mut line = String::new();
        let n = r.read_line(&mut line)?;
        if n == 0 {
            // EOF: clean iff it landed on a frame boundary (no partial headers seen).
            return if saw_any_header {
                Err(io::Error::new(io::ErrorKind::UnexpectedEof, "eof mid-header"))
            } else { Ok(None) };
        }
        let t = line.trim_end_matches(['\r', '\n']);
        if t.is_empty() { break; } // end of headers
        saw_any_header = true;
        if let Some(v) = t.strip_prefix("Content-Length:") {
            content_length = Some(v.trim().parse()
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad Content-Length"))?);
        }
    }
    let len = content_length
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length"))?;
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    let v = serde_json::from_slice(&buf)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(Some(v))
}
```

Tests: round-trip one frame; two back-to-back frames from one reader; `Ok(None)` on empty input;
`Err` on a body shorter than Content-Length; a split read (feed via a reader that yields a few
bytes at a time — a small `struct ChunkReader`).

3. **UTF-16 → byte** (spec §6.1) — mandatory multibyte tests `é`/`中`/`🙂`:

```rust
/// Map an LSP position (0-based `line`, UTF-16 code-unit `character`) to a byte offset into `text`.
/// Lines split on '\n' (we sent the text; wordcartel buffers are '\n'-normalized). A `character`
/// past the line end clamps to the line end; a `character` landing inside a code point's UTF-16
/// width maps to that code point's start (never splits a char). `None` when `line` exceeds the
/// text's line count.
pub fn utf16_pos_to_byte(text: &str, line: u32, character: u32) -> Option<usize> {
    let mut line_start = 0usize;
    let mut cur_line = 0u32;
    // Find the byte offset where `line` begins.
    if line > 0 {
        let mut seen = 0u32;
        let mut idx = 0usize;
        for (i, ch) in text.char_indices() {
            if ch == '\n' {
                seen += 1;
                if seen == line { idx = i + 1; break; }
            }
        }
        if seen < line { return None; } // line past EOF
        line_start = idx;
        cur_line = line;
    }
    let _ = cur_line;
    // Walk the target line, accumulating UTF-16 units. When `character` lands AT or INSIDE the
    // current scalar's UTF-16 width — i.e. character < u16_count + width — map to that scalar's
    // START byte, so a position inside a surrogate pair (🙂, char 1) never splits it and clamps to
    // byte 0 of the scalar (round-1 IMPORTANT 3). Only advance when the target is strictly past
    // this scalar.
    let mut u16_count = 0u32;
    for (off, ch) in text[line_start..].char_indices() {
        if ch == '\n' { return Some(line_start + off); } // clamp to line end
        let width = ch.len_utf16() as u32;
        if character < u16_count.saturating_add(width) {
            return Some(line_start + off);
        }
        u16_count = u16_count.saturating_add(width);
    }
    Some(text.len()) // ran off the end (last line, no trailing '\n') → clamp to end
}

/// Half-open byte range for an LSP range; `None` if either end is unmappable or end < start.
pub fn lsp_range_to_bytes(text: &str, start: (u32, u32), end: (u32, u32))
    -> Option<std::ops::Range<usize>> {
    let s = utf16_pos_to_byte(text, start.0, start.1)?;
    let e = utf16_pos_to_byte(text, end.0, end.1)?;
    if e < s { None } else { Some(s..e) }
}
```

Tests: `"café teh"` — position of `teh` maps to byte 6 (`"café "` = 6 bytes, `é` = 1 UTF-16 unit);
`"中文 x"` (each CJK char = 1 UTF-16 unit, 3 bytes) column mapping; `"🙂ab"` — `🙂` is 2 UTF-16
units / 4 bytes: **`utf16_pos_to_byte("🙂ab", 0, 0) == Some(0)`, `…(0, 1) == Some(0)` (character 1
lands INSIDE the surrogate pair → clamps to the scalar's start byte 0, NOT byte 4 — the round-1
IMPORTANT 3 assertion), `…(0, 2) == Some(4)` (`a`), `…(0, 3) == Some(5)` (`b`)**; past-EOL clamp;
`line` past EOF → `None`; last line with no trailing `\n`.

4. **codeAction edit → `Suggestion`** (spec §3.3.6, §6.2) — parse the verified quickfix shape from a
   `serde_json::Value` code action and produce a `ReplaceWith`. Given a diagnostic's byte range `d`:

```rust
/// Extract a `Suggestion::ReplaceWith` from a harper quickfix `CodeAction` value, matched to a
/// diagnostic whose byte range is `d`. Returns `None` for command-only actions (`kind != "quickfix"`
/// or no `edit`), for edits on a different uri, or for an edit that does not correspond to `d`.
/// (harper 2.1.0 verified: `edit.changes[uri] = [{newText, range}]` with clean `newText`.)
pub fn quickfix_suggestion(
    action: &serde_json::Value, our_uri: &str, doc_text: &str, d: &std::ops::Range<usize>,
) -> Option<Suggestion> {
    if action.get("kind").and_then(|k| k.as_str()) != Some("quickfix") { return None; }
    let changes = action.get("edit")?.get("changes")?.as_object()?;
    let edits = changes.get(our_uri)?.as_array()?;
    for te in edits {
        let new_text = te.get("newText")?.as_str()?.to_string();
        let r = te.get("range")?;
        let s = (r["start"]["line"].as_u64()? as u32, r["start"]["character"].as_u64()? as u32);
        let e = (r["end"]["line"].as_u64()? as u32, r["end"]["character"].as_u64()? as u32);
        let er = lsp_range_to_bytes(doc_text, s, e)?;
        // Map to our three-variant Suggestion the exact inverse of build_range_replace (spec §6.2).
        if er == *d {
            return Some(if new_text.is_empty() { Suggestion::Remove }
                        else { Suggestion::ReplaceWith(new_text) });
        }
        if er.is_empty() && er.start == d.end {
            return Some(Suggestion::InsertAfter(new_text));
        }
    }
    None
}
```

Tests: a quickfix action whose edit range == `d` with `newText:"the"` → `ReplaceWith("the")`;
empty `newText` at `d` → `Remove`; empty range at `d.end` → `InsertAfter`; a `kind:null`
command-only action → `None`; an edit on a foreign uri → `None`.

5. **Green + commit** `T2: lsp_rpc — framing, opaque URIs, UTF-16→byte, codeAction→Suggestion`.

---

## T3 — `harper_ls`: `HarperState` machine, client thread, `FlushGuard`, config-pull responder, supervision

**Goal.** The imperative-shell client. Split into two commits within the task if the reviewer
prefers, but the pure `HarperState` machine is the payoff and must be exhaustively unit-tested
without a process (spec §3.2–§3.4, §5, §8).

**Files:** `wordcartel/src/harper_ls.rs` (fill; tests inline). Depends on T2 (`lsp_rpc`) and the
provider types from T4 (`ProviderConfig`, `Accepted`, `Availability`, `ProviderEvent`) — **so T4
lands before T3's `impl DiagnosticsProvider for HarperLs`**; the pure `HarperState` machine itself
depends only on T2 + core types and may be written first.

**Command-surface conformance:** N/A.

### 3a. The pure `HarperState` machine (no IO)

Model exactly per spec §3.3. Inputs are `Inbound` + `now_ms`; output is `Vec<Action>`. Key
structures and the load-bearing methods (complete):

```rust
use std::collections::HashMap;
use crate::editor::BufferId;
use crate::app::Msg;
use crate::diag_provider::{Availability, ProviderConfig, ProviderEvent};
use wordcartel_core::diagnostics::{Diagnostic, DiagnosticKind};

const PUBLISH_TIMEOUT_MS: u64 = 10_000;
const CODEACTION_TIMEOUT_MS: u64 = 5_000;
const SHUTDOWN_GRACE_MS: u64 = 1_000;
const MAX_SPAWN_ATTEMPTS: u32 = 3;

/// Grammar/style linter names toggled off when `grammar = false` (spec §7.2). Curated best-effort;
/// harper ignores unknown keys and the client-side kind gate is the correctness backstop.
const GRAMMAR_LINTERS: &[&str] = &[
    "SentenceCapitalization","UnclosedQuotes","WrongQuotes","LongSentences","RepeatedWords",
    "Spaces","Matcher","CorrectNumberSuffix","NumberSuffixCapitalization","MultipleSequentialPronouns",
    "LinkingVerbs","AvoidCurses","TerminatingConjunctions","EllipsisLength","DotInitialisms",
    "BoringWords","ThatWhich","CapitalizePersonalPronouns","AnA","SpelledNumbers","UseGenitive",
];

#[derive(Debug, Clone)]
pub(crate) enum Cmd {
    Configure(ProviderConfig),
    Change { buffer_id: BufferId, version: u64, path: Option<std::path::PathBuf>, text: String },
    Close { buffer_id: BufferId },
    ReloadDict,
    Shutdown,
}

pub(crate) enum Inbound { Cmd(Cmd), Server(serde_json::Value), ServerEof }

pub(crate) enum Action {
    Send(serde_json::Value),
    Emit(Msg),
    SetAvailability(Availability),
    Respawn,
    Exit,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase { Initializing, Running, ShuttingDown }

struct DocState {
    uri: String, lsp_version: i32, our_version: u64, generation: u64, text: String, open: bool,
}
struct AwaitPublish { our_version: u64, generation: u64, deadline: u64 }
struct Assembly { our_version: u64, generation: u64, diags: Vec<Diagnostic>, deadline: u64 }
enum PendingKind { Initialize, Shutdown, CodeAction { buffer_id: BufferId, generation: u64 } }

pub(crate) struct HarperState {
    phase: Phase,
    cfg: ProviderConfig,
    docs: HashMap<BufferId, DocState>,
    uri_owner: HashMap<String, (BufferId, u64)>,
    next_generation: u64,
    queued: Vec<Cmd>,
    next_id: u64,
    pending_requests: HashMap<u64, PendingKind>,
    awaiting_publish: HashMap<BufferId, AwaitPublish>,
    assembling: HashMap<BufferId, Assembly>,
    spawn_attempts: u32,
}
```

Implement (complete bodies — the reviewer checks these against spec §3.3/§8/§5):

- `new(cfg) -> HarperState` — `phase: Initializing`, empty maps, `next_generation: 1`,
  `next_id: 1`, `spawn_attempts: 1` (the first spawn counts).
- **`settings_object(&self) -> serde_json::Value`** — the BARE harper settings (spec §8): `{ dialect:
  "American", userDictPath?: <path or omitted when None>, maxFileLength: cfg.max_file_length,
  linters: { "SpellCheck": true, <each GRAMMAR_LINTERS name>: false when !cfg.grammar } }`.
- `on_spawned(now) -> Vec<Action>` — reset to `Initializing`, mark every `DocState.open = false`,
  clear `pending_requests`, allocate an `initialize` id (record `PendingKind::Initialize`) with
  capabilities incl. **`workspace.configuration = true`** and `publishDiagnostics.versionSupport =
  true`; `Send(initialize)`.
- `on_inbound(&mut self, inb, now) -> Vec<Action>` — the router:
  - `Inbound::Cmd(c)` if `phase != Running` and `c` is not `Shutdown` → push to `queued`, return
    `[]` (except `Configure` updates `self.cfg` immediately). If `Running` → `apply_cmd(c, now)`.
  - `Inbound::Server(v)` → `on_server(v, now)`.
  - `Inbound::ServerEof` → `on_server_gone(now)`.
- `apply_cmd`:
  - `Change{buffer_id,version,path,text}` → `on_change(buffer_id, version, path, text, now)`
    (records the awaiting slot FIRST — the latch-gap guard — then emits `didOpen`/`didChange`).
  - `Close{buffer_id}` → **emit terminals first**: for an `awaiting_publish`/`assembling` entry,
    `Emit(Msg::DiagnosticsDone{buffer_id, version: our_version, diagnostics: vec![]})`; then
    `Send(didClose(uri))`, remove `docs`/`uri_owner`/awaiting/assembling (spec §3.3 Cmd::Close).
  - `ReloadDict` → `Send(didChangeConfiguration{ settings: {"harper-ls": settings_object }})`.
  - `Configure(cfg)` → `self.cfg = cfg;` then a `ReloadDict`-style resend.
  - `Shutdown` → `phase = ShuttingDown`, allocate a `shutdown` id (`PendingKind::Shutdown`), `Send`.
- **`on_change`** (records awaiting first — the accepted-but-unrecorded guard, spec §3.2/§5.1):
```rust
fn on_change(&mut self, buffer_id: BufferId, version: u64,
    _path: Option<std::path::PathBuf>, text: String, now: u64) -> Vec<Action> {
    let reopen = !self.docs.get(&buffer_id).map(|d| d.open).unwrap_or(false);
    let mut out = Vec::new();
    if reopen {
        let generation = self.next_generation; self.next_generation += 1;
        let uri = crate::lsp_rpc::doc_uri(buffer_id, generation);
        self.uri_owner.insert(uri.clone(), (buffer_id, generation));
        let lsp_version = 1;
        // Record awaiting BEFORE the Send action (non-IO first step; flush covers a mid-send death).
        self.awaiting_publish.insert(buffer_id,
            AwaitPublish { our_version: version, generation, deadline: now + PUBLISH_TIMEOUT_MS });
        out.push(Action::Send(serde_json::json!({
            "jsonrpc":"2.0","method":"textDocument/didOpen",
            "params":{"textDocument":{"uri":uri,"languageId":"markdown","version":lsp_version,"text":text}}})));
        self.docs.insert(buffer_id,
            DocState { uri, lsp_version, our_version: version, generation, text, open: true });
    } else {
        let (uri, generation, lsp_version) = {
            let d = self.docs.get_mut(&buffer_id).expect("open doc exists");
            d.lsp_version = d.lsp_version.saturating_add(1);
            debug_assert!(d.lsp_version < i32::MAX, "lsp_version overflow");
            d.our_version = version; d.text = text.clone();
            (d.uri.clone(), d.generation, d.lsp_version)
        };
        self.awaiting_publish.insert(buffer_id,
            AwaitPublish { our_version: version, generation, deadline: now + PUBLISH_TIMEOUT_MS });
        out.push(Action::Send(serde_json::json!({
            "jsonrpc":"2.0","method":"textDocument/didChange",
            "params":{"textDocument":{"uri":uri,"version":lsp_version},
                "contentChanges":[{"text":text}]}})));
    }
    out
}
```
- **`on_server`** handles: `initialize` response → `initialized` + `didChangeConfiguration` push +
  `phase = Running` + replay `queued`; a `workspace/configuration` **request** → the UNWRAPPED
  responder (complete):
```rust
fn answer_configuration(&self, req: &serde_json::Value) -> Action {
    let items = req["params"]["items"].as_array().map(|a| a.len()).unwrap_or(1);
    let obj = self.settings_object();
    let result: Vec<serde_json::Value> = (0..items).map(|_| obj.clone()).collect(); // BARE, unwrapped
    Action::Send(serde_json::json!({"jsonrpc":"2.0","id":req["id"].clone(),"result":result}))
}
```
  ...a `publishDiagnostics` notification → `on_publish` (URI-keyed generation attribution §3.3
  Receive, version echo tolerated-None, convert via `lsp_rpc`, empty→emit / non-empty→codeAction);
  a codeAction response → assemble suggestions (`lsp_rpc::quickfix_suggestion`) and emit; other
  server requests → null result or `MethodNotFound`; a `shutdown` response → `exit` + `Exit`.

- **INVARIANT — remove-the-tracked-entry BEFORE (or atomically with) emitting its terminal**
  (round-2 IMPORTANT; guards against the assembly-overwrite hazard). The real
  `diagnostics_run::apply_diagnostics_done` stores *any* vector whose `version` matches the live
  buffer version — it cannot distinguish "empty crash-flush for an already-terminated assembly"
  from "a real empty result." So EVERY terminal-emitting path must first delete the entry it is
  terminating, so a later `flush_outstanding` can never re-emit an EMPTY terminal for a version
  whose real (non-empty) result already landed and overwrite it back to empty. Concretely, each of
  these removes-then-emits (or removes as it emits):
  - **empty publish** → `awaiting_publish.remove(buffer_id)`, then `Emit(empty terminal)`.
  - **non-empty publish** → `awaiting_publish.remove(buffer_id)`, then insert `assembling[buffer_id]`
    + send the codeAction (the await is replaced by the assembly; still exactly one tracked entry).
  - **codeAction response** → `assembling.remove(buffer_id)`, then `Emit(terminal with suggestions)`.
  - **codeAction watchdog** → `assembling.remove(buffer_id)`, then `Emit(suggestionless terminal)`.
  - **publish watchdog** → `awaiting_publish.remove(buffer_id)`, then `Emit(empty terminal)`.
  - **`Cmd::Close`** → remove `awaiting_publish`/`assembling`, then `Emit(empty terminal)` (spec §3.3).
  Result: at the instant any terminal for version `v` is emitted, `v` is no longer tracked, so
  `flush_outstanding` (crash/respawn/panic-drain) emits ONLY for still-outstanding versions — never
  a duplicate empty for a version whose result already landed. The §5.1 "at least one terminal
  (duplicates tolerated)" invariant holds, but no tolerated duplicate can be an empty-clobbering
  one: a late *genuine* publish after a watchdog empty re-converts to a non-empty (improves, never
  clobbers), while the flush can no longer touch a terminated version.
- **`on_deadline(now) -> Vec<Action>`** — publish watchdog (awaiting past deadline →
  **`awaiting_publish.remove`** then emit empty tagged) and codeAction watchdog (assembling past
  deadline → **`assembling.remove`** then emit converted diags suggestionless). Both obey the
  remove-before-emit invariant above.
- **`on_server_gone(now)`** — **flush all outstanding on EVERY path** (spec §3.4; the round-1
  CRITICAL): `flush_outstanding()` emits an empty version-tagged terminal for every
  `awaiting_publish`/`assembling`/queued `Cmd::Change` FIRST — otherwise a crash *with budget left*
  leaves `in_flight_version` latched (`diag_due` blocks) and the `Restarted` re-arm can never
  dispatch → wedge. (`flush_outstanding` drains the tracked maps as it emits, so they are already
  empty after it runs.) Then branch: attempts remaining (`spawn_attempts < MAX_SPAWN_ATTEMPTS`) →
  `spawn_attempts += 1`, mark every `DocState.open = false`, clear `uri_owner`,
  `SetAvailability(Starting)`, `Emit(ProviderEvent::Restarted)`, `Respawn`; exhausted →
  `SetAvailability(Unavailable)`, `Emit(ProviderEvent::Degraded(hint))`, `Exit`. (The flush runs in
  both branches — the fresh generation on reopen means the flushed empties are old-version-tagged
  and dropped by `apply_diagnostics_done`'s version gate, but they still clear the latch.)
- **`flush_outstanding(&mut self) -> Vec<Action>`** — **drain-as-it-emits**: for every entry STILL
  in `awaiting_publish` + `assembling` + queued `Cmd::Change`, emit an empty version-tagged
  `Msg::DiagnosticsDone` AND remove that entry (`self.awaiting_publish.clear()`,
  `self.assembling.clear()`, drop the queued changes). Clearing makes it **idempotent** — a second
  call (e.g. `on_server_gone` exhaustion calls it, then the `FlushGuard` drop calls it again on the
  same `HarperState`) emits nothing the second time, so no benign double-empty. Because every
  terminal-emitting path already removed its own entry first (the invariant above), a version whose
  real result already landed is not tracked and is therefore NOT flushed — this is what prevents an
  empty flush from clobbering a landed non-empty store.

Tests (inline, `#[cfg(test)]` — the payoff surface, no process): drive scripted `Inbound` sequences
and assert the `Vec<Action>` per spec §15 item 2 — handshake order + `workspace.configuration=true`;
**config-pull responder returns an unwrapped `result:[settings]` per item**; didOpen→didChange
`lsp_version` increment (+ `saturating_add` at `i32::MAX`); opaque `doc_uri` (save = plain
didChange, no reopen); generation attribution (absent-uri publish dropped); `Cmd::Close` emits the
terminal before removing state; the reload/recover race (await for gen g → Close → reopen g+1 →
old-uri publish dropped); omitted-`version` publish accepted via generation; codeAction verified
shape → `ReplaceWith` attached + command-only dropped; assembly generation superseded mid-fetch →
discarded; empty publish emits immediately; watchdogs; `flush_outstanding` covers awaiting +
assembling + queued. **Respawn-with-budget flushes the latch** — a `ServerEof` with an outstanding
awaiting for version `v` and budget remaining emits an empty terminal for `v` (asserting
`Action::Emit(DiagnosticsDone{version: v, diagnostics: []})` precedes the `Respawn`) AND emits
`Restarted` — the round-1 CRITICAL wedge guard; budget exhaustion likewise flushes then
`Unavailable` + `Degraded`. **Assembly-overwrite guard (round-2 IMPORTANT)** — scripted: publish
non-empty for `v` → codeAction response emits `DiagnosticsDone{version:v, non_empty}` (and removes
`assembling[buffer_id]`) → then `Inbound::ServerEof` → assert **NO second (empty) terminal for `v`
is emitted** (the flush finds no tracked entry for `v`), so a real `apply_diagnostics_done` at doc
version `v` keeps the non-empty store instead of being clobbered to empty. Symmetric assertions for
the publish-watchdog and codeAction-watchdog paths: after each emits its terminal, a subsequent
`ServerEof` produces no duplicate terminal for that version.

### 3b. The client thread + `FlushGuard` + `HarperLs` handle

`FlushGuard` **owns `cmd_rx`** and runs the two-part flush on `Drop` (tracked + channel-drain), the
pump runs inside `catch_unwind`, `ensure_running` does not latch `started` on a spawn `Err`, and
`notify_change` returns `Accepted::No` + flips availability on a disconnected send. Structure per
spec §3.1/§3.2 (complete `HarperLs`, `Shared`, the thread fn with `Command::new("harper-ls")
.arg("--stdio")`, the `wcartel-harper-read` reader forwarding `Inbound::Server`/`ServerEof`, and
the `recv_timeout(next deadline)` pump executing `Action`s). `NotFound` spawn error → `Unavailable`
+ `Degraded(INSTALL_HINT)` + drain-drop + exit.

Tests: `FlushGuard` drop emits terminals for a queued-but-unread `Cmd::Change` (construct a guard
holding a `Receiver` with a `Change` still in it, drop it, assert the emitted `Msg`); a panic in the
pump still flushes (a `#[cfg(test)]` panic hook injection). Real-process behavior is covered by the
T1 probe + T5 integration test, both `#[ignore]`-gated.

**Green + commit** `T3: harper_ls — HarperState machine, client thread, FlushGuard, config-pull responder`.

---

## T4 — The `DiagnosticsProvider` seam, `NullProvider`, `RecordingProvider`, the `Msg` variant, Editor wiring

**Goal.** The trait + hermetic default + test mock + the `Editor` field & init, **and the
`Msg::DiagProviderEvent` variant + a minimal `reduce_dispatch` handler** — so T3 (which emits that
`Msg`) compiles and dispatch can be re-pointed in T5. (Lands before T3's `impl DiagnosticsProvider
for HarperLs`.)

**Files:** `wordcartel/src/diag_provider.rs` (fill), `wordcartel/src/editor.rs` (field + init +
`diag_hint_shown` + `set_render_mode` reset), `wordcartel/src/app.rs` (`Msg` variant + `Debug` arm
+ minimal `reduce_dispatch` handler). Inline tests in `diag_provider.rs`.

**Command-surface conformance:** N/A — the seam is not a user-settable option; no registry/menu/
palette/hint change (spec §14).

### Steps

1. **The trait + types** (complete, spec §2) — write into `diag_provider.rs`:

```rust
use crate::editor::{BufferId, Editor};
use wordcartel_core::history::Clock;

/// Status hint shown when no checker is available (spec §9).
pub const INSTALL_HINT: &str =
    "grammar checker unavailable — install harper-ls (Arch: pacman -S harper)";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Availability { Idle, Starting, Ready, Unavailable }

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Accepted { Yes, No }

#[derive(Clone, Debug)]
pub struct ProviderConfig {
    pub grammar: bool,
    pub dictionary: Option<std::path::PathBuf>,
    pub max_file_length: u64,
}

#[derive(Clone, Debug)]
pub enum ProviderEvent { Restarted, Degraded(String) }

/// The diagnostics backend seam (Effort A). Thin, mockable; results are emitted asynchronously as
/// `Msg::DiagnosticsDone` (and lifecycle as `Msg::DiagProviderEvent`) on the `Sender<Msg>` the impl
/// was constructed with. All methods are non-blocking (hot-path law).
pub trait DiagnosticsProvider: std::fmt::Debug {
    fn name(&self) -> &'static str;
    fn availability(&self) -> Availability;
    fn ensure_running(&mut self);
    fn configure(&mut self, cfg: ProviderConfig);
    /// Full-document sync. `Accepted::Yes` ⟹ at least one terminal `DiagnosticsDone` for
    /// `(buffer_id, version)` is guaranteed (spec §5.1); `Accepted::No` ⟹ nothing emitted, caller
    /// must NOT latch.
    fn notify_change(&mut self, buffer_id: BufferId, version: u64,
        path: Option<std::path::PathBuf>, text: String) -> Accepted;
    fn notify_close(&mut self, buffer_id: BufferId);
    /// Best-effort: ask the server to re-read `userDictPath` (a config resend). NOT a writer.
    fn reload_dictionary(&mut self);
    fn shutdown(&mut self);
}

/// Hermetic default (production): no thread, no process, no emissions.
#[derive(Debug, Default)]
pub struct NullProvider;
impl DiagnosticsProvider for NullProvider {
    fn name(&self) -> &'static str { "none" }
    fn availability(&self) -> Availability { Availability::Idle }
    fn ensure_running(&mut self) {}
    fn configure(&mut self, _cfg: ProviderConfig) {}
    fn notify_change(&mut self, _b: BufferId, _v: u64, _p: Option<std::path::PathBuf>, _t: String)
        -> Accepted { Accepted::No }
    fn notify_close(&mut self, _b: BufferId) {}
    fn reload_dictionary(&mut self) {}
    fn shutdown(&mut self) {}
}

/// Thin reduce/prompts delegation (spec §2). `clock` is needed for the Restarted re-arm.
pub fn apply_provider_event(editor: &mut Editor, ev: ProviderEvent, clock: &dyn Clock) {
    match ev {
        ProviderEvent::Restarted => {
            editor.status = "grammar checker restarted".into();
            if crate::diagnostics_run::should_run_diagnostics(editor) {
                let now = clock.now_ms();
                let debounce = editor.diag_cfg.debounce_ms;
                editor.active_mut().diagnostics.arm(now, debounce);
            }
        }
        ProviderEvent::Degraded(hint) => { editor.status = hint; }
    }
}
```

2. **`RecordingProvider`** — `#[cfg(test)]` mock recording every call with a settable `Accepted`
   return and `Availability` (used by T3/T5 dispatch tests without harper-ls).

3. **Editor wiring** (`editor.rs`):
   - Add field after `pub diag: Option<...>`:
     ```rust
     /// Active diagnostics provider (Effort A). NullProvider by default → hermetic construction;
     /// run() installs HarperLs once the msg channel exists.
     pub diag_provider: Box<dyn crate::diag_provider::DiagnosticsProvider>,
     /// One install-hint per deliberate Review entry (reset in set_render_mode). Spec §9.
     pub diag_hint_shown: bool,
     ```
   - In the `new_from_text` struct literal, beside `diag: None,`:
     ```rust
     diag_provider: Box::new(crate::diag_provider::NullProvider),
     diag_hint_shown: false,
     ```
   - In `set_render_mode`, when `mode == RenderMode::Review`, add `self.diag_hint_shown = false;`
     (before the arm-on-enter block; leaves the existing arm logic unchanged).

4. **The `Msg` variant + minimal handler** (`app.rs`) — so T3 compiles (CRITICAL 1: no forward
   reference). Add to `enum Msg`:
   ```rust
   DiagProviderEvent(crate::diag_provider::ProviderEvent),
   ```
   Add a hand-written arm to `impl Debug for Msg` (mirroring the existing hand-written impl):
   ```rust
   Msg::DiagProviderEvent(ev) => f.debug_tuple("DiagProviderEvent").field(ev).finish(),
   ```
   Add the arm to **`reduce_dispatch`'s message match** beside `Msg::DiagnosticsDone` — already the
   FULL handler, since `apply_provider_event` exists in this task:
   ```rust
   Msg::DiagProviderEvent(ev) => crate::diag_provider::apply_provider_event(editor, ev, clock),
   ```
   (`clock: &dyn Clock` is a `reduce_dispatch` parameter — verified.) This makes T3's
   `Action::Emit(Msg::DiagProviderEvent(..))` compile; T5 only adds the second delivery site,
   `prompts::intercept`.

**Command-surface note:** `set_render_mode` remains the single law-6 setter; unchanged behavior.

**Green + commit** `T4: DiagnosticsProvider seam + NullProvider + Msg::DiagProviderEvent + Editor wiring`.

---

## T5 — Integration at every touched production site

**Goal.** Wire the seam into the live loop, config, dispatch/apply, save/close, dictionary, and
status — spec §4. Depends on T3+T4.

**Files:** `wordcartel/src/app.rs` (Msg variant + Debug arm + reduce_dispatch arm + install +
shutdown + delete warm thread), `wordcartel/src/prompts.rs` (intercept arm),
`wordcartel/src/diagnostics_run.rs` (dispatch/apply re-point + `retain_unignored`),
`wordcartel/src/timers.rs` (on_tick simplify), `wordcartel/src/search_ui.rs` (add-dict/ignore
bodies), `wordcartel/src/workspace.rs` (close_buffer_now three shapes), `wordcartel/src/save.rs`
(reload/recover notify_close), `wordcartel/src/render_status.rs` (attribution),
`wordcartel/src/config.rs` (doc comment only).

**Command-surface conformance:** Existing commands (`quick_fix`/`diag_next`/`diag_prev`/
`recheck_diagnostics`/`view_review`/`cycle_render_mode`) and `set_render_mode` are untouched.
`render_status` attribution is display-only, not command-reachable state. No registry/menu/palette
change. **Conformant; the command surface is not modified.**

### Steps (each with its test)

1. **`prompts::intercept`** — the `Msg::DiagProviderEvent` variant, its `Debug` arm, and the
   `reduce_dispatch` handler already landed in **T4**. T5 adds only the SECOND delivery site: the
   SAME arm in `prompts::intercept`'s `match msg` (it threads `clock`), beside its
   `DiagnosticsDone` arm:
   ```rust
   Msg::DiagProviderEvent(ev) => crate::diag_provider::apply_provider_event(editor, ev, clock),
   ```
   so `Degraded`/`Restarted` reach the status line even under an open modal.
3. **`dispatch_diagnostics` re-point** (`diagnostics_run.rs`) — replace the current body with the
   seam version (spec §4.3), latching only on `Accepted::Yes`; add `retain_unignored`; extend
   `apply_diagnostics_done` with the apply-time ignore filter over `dictionary ∪ session_ignores`;
   **delete `append_word_to_dict`? NO — keep it** (single writer). Complete `dispatch_diagnostics`:
   ```rust
   pub fn dispatch_diagnostics(editor: &mut Editor) {
       let b = editor.active();
       let (buffer_id, version) = (b.id, b.document.version);
       let path = b.document.path.clone();
       let text = b.document.buffer.snapshot().to_string();
       editor.active_mut().diagnostics.recheck_due_at = None;
       if text.len() as u64 > crate::limits::DIAG_MAX_SEND_BYTES {
           editor.status = "document too large for grammar checking".into();
           return;
       }
       editor.diag_provider.ensure_running();
       use crate::diag_provider::{Availability, Accepted};
       if editor.diag_provider.availability() == Availability::Unavailable {
           if !editor.diag_hint_shown {
               editor.diag_hint_shown = true;
               editor.status = crate::diag_provider::INSTALL_HINT.into();
           }
           return;
       }
       if editor.diag_provider.availability() == Availability::Starting {
           editor.status = "starting grammar checker…".into();
       }
       match editor.diag_provider.notify_change(buffer_id, version, path, text) {
           Accepted::Yes => { editor.active_mut().diagnostics.in_flight_version = Some(version); }
           Accepted::No => {
               if !editor.diag_hint_shown {
                   editor.diag_hint_shown = true;
                   editor.status = crate::diag_provider::INSTALL_HINT.into();
               }
           }
       }
   }
   ```
   `apply_diagnostics_done` gains the union filter after the version check; `retain_unignored(editor)`
   refilters the active store in place with the same predicate.
4. **`timers::on_tick`** — simplify the diagnostics block to
   `if should_run_diagnostics(editor) && diag_due(...) { dispatch_diagnostics(editor); }`
   (drop the `ignore_words` Arc build + `diag_cfg.clone()`). `diag_deadline` + `SUBSYSTEMS`
   unchanged.
5. **`app.rs` install + shutdown + warm-thread delete** — delete the `wcartel-diag-warm` block;
   **keep** the startup `bounded_read_opt` dictionary load; after `let (msg_tx, msg_rx) = …channel`,
   install `editor.diag_provider = Box::new(crate::harper_ls::HarperLs::new(msg_tx.clone(),
   ProviderConfig{ grammar: cfg.diagnostics.grammar, dictionary: cfg.diagnostics.dictionary.clone(),
   max_file_length: crate::limits::HARPER_MAX_FILE_LENGTH }))`; both run-loop exit paths call
   `editor.diag_provider.shutdown()`.
6. **`search_ui::diag_apply_selected`** — ignore branch: `session_ignores.insert` + close +
   `retain_unignored` (drop the re-arm). add-dict branch: **`editor.dictionary.insert(word.clone())`
   FIRST — unconditional client-side suppression (no-loss, spec §7.4), so it clears the underline
   for the session even with no path.** THEN, only if a `dictionary` path is configured:
   `append_word_to_dict(dict_path, &word)` (the sole file writer) + `reload_dictionary()`; the
   `None` case sets the "no dictionary path configured" status but the word is already suppressed —
   round-1 IMPORTANT 5: the None branch must NOT be a no-op beyond a status. Then close +
   `retain_unignored`. Suggestion branch byte-for-byte unchanged. Sketch:
   ```rust
   editor.dictionary.insert(word.clone()); // client-side suppression regardless of path
   match editor.diag_cfg.dictionary.clone() {
       Some(dict_path) => match crate::diagnostics_run::append_word_to_dict(&dict_path, &word) {
           Ok(()) => editor.diag_provider.reload_dictionary(),
           Err(e) => editor.status = format!("add to dictionary failed: {e}"),
       },
       None => editor.status = "no dictionary path configured".into(),
   }
   editor.diag = None;
   crate::diagnostics_run::retain_unignored(editor);
   ```
7. **`workspace::close_buffer_now`** — `editor.diag_provider.notify_close(id)` in all three shapes
   (before `editor.buffers[i] = …` in the replace-last-ordinary branch; before each `remove`).
8. **`save.rs`** — `reload_from_disk` and `load_recovered` add `editor.diag_provider.notify_close(id)`
   where they reset `DiagStore::new()` (capture `id` first).
9. **`render_status::status_left_text`** — the `Review` arm of `mode_text` becomes attribution-aware
   (spec §10): `Ready` → `format!("REVIEW · {}", editor.diag_provider.name())`, else `"REVIEW"`;
   change `mode_text` to `Cow<'static, str>`.
10. **`config.rs`** — update the `DiagnosticsConfig.dictionary` doc comment only (no schema change).

Tests (mock provider, no harper-ls): `diagnostics_run` — `Accepted::Yes` latches, `Accepted::No`
leaves latch `None` + hint, over-cap → status + no call, ignore filter over the union,
`retain_unignored`; `diag_provider` — `apply_provider_event` both variants (Restarted re-arm with
the threaded clock), delivery through both `reduce_dispatch` and `prompts::intercept`, sweep
precision; `search_ui` — single-write add-dict (assert no second file write) + ignore refilter;
`workspace`/`save` — `notify_close` in all three close shapes + both reload/recover; `render_status`
— `REVIEW · Harper` when Ready else `REVIEW`; e2e (`e2e.rs`) — degradation + attribution journeys.

**Green + commit** `T5: integrate harper-ls provider at every touched production site`.

---

## T6 — Degradation, real-binary integration test, core removal, packaging

**Goal.** The absent-harper story end-to-end, the real-binary conversation test, and the build
changes that make the swap real. Depends on T5.

**Files:** `wordcartel-core/src/diagnostics.rs`, `wordcartel-core/Cargo.toml`,
`packaging/arch/PKGBUILD`, `packaging/arch/.SRCINFO`, new `wordcartel/tests/harper_ls_integration.rs`.

**Command-surface conformance:** N/A.

### Steps

1. **Delete the embedded Harper backend** (`wordcartel-core/src/diagnostics.rs`) — remove `check`,
   `CheckOpts`, `HarperLint`, `harper_lints`, `classify`, `char_span_to_bytes`, `map_suggestions`,
   the `harper_core` imports, and the Harper-driving tests. **Keep** `Diagnostic`, `DiagnosticKind`,
   `Suggestion` + derives; update the module doc to the pure data contract (byte ranges into the
   checked text, sorted by `range.start`).
2. **Remove `harper-core`** from `wordcartel-core/Cargo.toml`. Confirm `Cargo.lock` sheds the
   `harper-*`/`burn-*` tree (record the before/after crate count + build-time note in the effort
   report — closes H2).
3. **Real-binary integration test** (`wordcartel/tests/harper_ls_integration.rs`, `#[ignore]`-gated,
   skips when harper-ls absent) — drives the full `HarperLs` provider (not raw stdio): construct it
   with a `Sender<Msg>`, `ensure_running`, `configure`, `notify_change` a doc containing "teh" and a
   dictionary word, pump the `Receiver<Msg>` until a `DiagnosticsDone` with a `Spelling`
   `ReplaceWith("the")` for "teh" and NO diagnostic for the dictionary word arrives — exercising the
   config-pull answer path (without which harper emits nothing).
4. **Packaging (BOTH files — round-1 MINOR 7).** Add to `packaging/arch/PKGBUILD`:
   `optdepends+=('harper: grammar/spelling diagnostics in Review mode (harper-ls language server)')`,
   AND update the tracked generated metadata `packaging/arch/.SRCINFO` to match (regenerate via
   `makepkg --printsrcinfo > .SRCINFO` from `packaging/arch/`, or hand-add the matching
   `optdepends = harper: …` line) so Arch/AUR consumers see the optional dep. Both files are
   committed together.

Tests: `wordcartel-core` lib compiles + its remaining tests green after the deletion; e2e
degradation journey (from T5) still green; the integration test skips cleanly without harper-ls.

**Green + commit** `T6: remove embedded harper-core, degradation + real-binary integration test, optdepends`.

---

## T7 — H18 tail: `cargo deny` supply-chain scan

**Goal.** CVE + license + duplicate scanning against the post-swap tree (spec §12). Depends on T6
(the dependency tree is final after core removal).

**Files:** new `deny.toml` (workspace root); effort-report note; `backlog.toml` (mark H18 shipped,
H2 answered) via `scripts/backlog bless`.

**Command-surface conformance:** N/A.

### Steps

1. Write `deny.toml`: `[advisories]` (deny vulnerabilities, warn unmaintained); `[licenses]` (allow
   the permissive set the real tree uses — enumerate from a live `cargo deny check licenses` run,
   not guessed); `[bans]` (`multiple-versions = "warn"`); `[sources]` (crates.io only; the `repar`
   path dep is out of scope by nature).
2. Run `cargo deny check`; triage findings in the effort report; apply in-reach version bumps,
   record the rest. Document `cargo deny check` as a release-checklist step (CLAUDE.md hardening
   note) — **not** a merge GATE (promotion is a separate deliberate edit).
3. Backlog: edit H18 → shipped and H2 → shipped/answered-by-removal in `backlog.toml`; run
   `scripts/backlog bless`; move their prose sections to `docs/backlog-archive.md` and repoint
   `doc =` so the marker bijection stays green (`wordcartel/tests/backlog.rs`).

**Green + commit** `T7: H18 supply-chain scan (cargo deny) + backlog`.

---

## Final gates (after T7, before merge)

- `cargo test` green across all suites; `cargo build` + `cargo test --no-run` warning-free for
  touched crates; `cargo clippy --workspace --all-targets` clean (deny); `module_budgets` +
  `backlog` tests green.
- `scripts/smoke/run.sh` run; quote its one-line summary verbatim in the pre-merge report (the new
  Review+harper smoke check reports SKIP when harper-ls is absent).
- The two final review gates per CLAUDE.md: a Fable whole-branch review + a Codex pre-merge GO/NO-GO.
- Merge `--no-ff` to `main`; verify tests on the merged result; delete the branch. Push only when asked.
