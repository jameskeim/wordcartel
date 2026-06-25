# Wordcartel Effort 5a — Config + Data-Driven Keymap + Session State (design)

**Status:** design (brainstormed 2026-06-24)
**Parent:** Effort 5 (App) — decomposed into 5a–5g (coverage ledger row 5). **5a is the foundation** everything else reads.
**Spec source:** main design §12 (Configuration & Keybindings), §12.1 (modeless/CUA-now, mode-capable underneath), §12.2 (palette/menu resolve through the registry — consumed in 5b), §12.4/§12.5 (key constraints, config file), §10.4 (name-keyed command registry — built in 4b).

---

## 1. Goal

Make Wordcartel **configurable and rebindable**, and make it **remember where you were**:
1. **Config** — a TOML config file with layered precedence; the typed substrate every later 5x sub-effort extends.
2. **Data-driven keymap** — replace the hardcoded `input::key_to_command_id` with a `KeyTrie` that resolves key chords (including **multi-key sequences**) through the 4b command registry; user-overridable; ships a CUA default and a bundled opt-in **WordStar** preset.
3. **Session state** — path-keyed persistence so reopening a file **restores the cursor/scroll** (resume-at-position) and holds the **marks store** (the mark/jump *commands* are 5c).

**Binding principle:** degrade, never abort — a bad/missing config or state file warns and falls back to defaults; the editor always starts and always edits.

## 2. Architecture

Shell-crate work (`wordcartel`); `wordcartel-core` is untouched (config/keymap/state are IO + app-policy concerns). New focused modules:

- **`config.rs`** — the typed `Config` struct, TOML load, the 3-layer precedence merge, `--config`/`--no-config` handling.
- **`keymap.rs`** — `KeyChord`, `KeyTrie`, `Resolution`, chord parsing, the bundled preset tables (CUA + WordStar), patch-merge over a preset base, `resolve(mode, &pending)`.
- **`state.rs`** — the path-keyed session store (cursor/scroll + marks), load/save in the XDG state dir.
- **Modified:** `input.rs` (resolution now goes through the keymap, not the hardcoded match), `app.rs` (`run()` loads config+state at startup, holds pending-keys, dispatches via the keymap → registry, persists state debounced; the pending-sequence status indicator), `registry.rs` (name-based resolution/validation — see §5.1), `main.rs` (**a real CLI parser — Codex spec-review fix:** today `main.rs` treats argv[1] as a bare file path with NO option parsing, so `--config foo.toml` would try to open a file literally named `--config`. Add a small hand-rolled `Cli { path: Option<PathBuf>, config_path: Option<PathBuf>, no_config: bool }` parser — no new dep — passed into `run()`/startup), `editor.rs` (pending-keys + loaded-config handle if needed).
- **New deps (shell crate only; confirm versions at impl time):** `toml` (parse), `serde` + `serde_derive` (derive `Deserialize` on `Config`). `dirs = "5"` is already present. **No new path crate** — config dir via `dirs::config_dir()`, state dir reuses `swap::state_dir()`.

## 3. Config: format, precedence, loading

### 3.1 Precedence (§12.5)
Effective config = built-in defaults **<** XDG global `~/.config/wordcartel/config.toml` (via `dirs::config_dir()`) **<** project-local config (nearest `.wordcartel.toml` found by walking up from the **anchor** directory) **<** `--config <path>` (explicit, highest). **The anchor (Codex spec-review fix — pin it):** the parent directory of the **initial CLI file** if one was given, else the **current working directory**. (Per-buffer project-config discovery when multiple files are open is **out of scope for 5a** — Effort 6 multi-buffer; v1 resolves project-local config once at startup from the single anchor.) `--no-config` → built-in defaults only (skip all file lookup). A missing file at any layer is normal (that layer contributes nothing). **Merge is per-key deep**: a later layer overrides only the keys it sets; unset keys inherit. (The keymap layer's merge is a patch, §4.3.)

### 3.2 The typed `Config`
`Config` derives `serde::Deserialize` with `#[serde(default)]` so any subset is valid. 5a defines the loader + only the schema **it actually consumes** (`[keymap]` + `[state]`); **each later 5x sub-effort adds its own keys** to the same struct (5d wrap-column + focus settings, 5f harper toggles, 5b palette chord, theme/§13 theming, …) — no speculative keys land before they have a consumer. 5a's schema — **all serde-native types, no custom deserializer** (Codex spec-review fix: the earlier "`[keymap]` table mixing a reserved `preset` with open `chord = string|false`" values does not deserialize cleanly; split into typed fields):
```toml
[keymap]
preset = "cua"                     # "cua" (default) | "wordstar" — typed field
bind  = { "ctrl-x" = "cut", "ctrl-k ctrl-s" = "save" }   # chord → command-id (BTreeMap<String,String>)
unbind = ["ctrl-l"]                # chords to remove from the base (Vec<String>)

[state]
resume = true                      # restore cursor/scroll on open
max_entries = 200                  # session-state size cap (§6)
```
So `KeymapConfig { preset: Option<String>, bind: BTreeMap<String,String>, unbind: Vec<String> }` — `preset` is a typed field (no longer mixed into the chord map), `bind` values are plain strings, and unbinding is a separate list (no string-or-`false` union). This needs no custom serde and no two-pass `toml::Value` split. **Unknown keys/sections are silently ignored** (serde default — forward-compat for later sub-efforts' keys; `deny_unknown_fields` is NOT used). A typo-detection warning for unknown keys is a **deferred nicety** (it would require a `toml::Value` pre-walk; not worth it in v1 against the forward-compat requirement). A TOML **parse** error (malformed syntax) → startup status warning naming the file, then fall back to defaults for that layer (never crash).

## 4. Keymap (data-driven, multi-key)

### 4.1 Model
- `KeyChord` = a single key + modifiers (e.g. `Ctrl-K`). A binding is a **sequence** of one or more chords (`["ctrl-k","ctrl-s"]`).
- `KeyTrie` is keyed by **mode** (v1 ships one mode: `normal`). Each mode is a trie of chord-sequences → `CommandId`.
- `resolve(mode, pending: &[KeyChord]) -> Resolution` where
  ```rust
  enum Resolution { Command(CommandId), Pending, None }
  ```
  - `Command(id)` — the pending sequence completed a binding → dispatch it, clear pending.
  - `Pending` — the pending sequence is a valid prefix of ≥1 binding → keep accumulating; show the prefix in the status line (`Ctrl-K …`).
  - `None` — no binding and not a prefix → clear pending; if the pending was a single printable key with no modifiers, fall through to literal insert (the §10.4 printable fallthrough); otherwise a brief "unbound" status.
- **No timeout** (terminal key timing is unreliable). A pending sequence is resolved by the next key or cancelled by **Esc**.

**Esc precedence (Codex spec-review fix — CRITICAL; pin it against the live modal stack).** A pending key sequence can ONLY exist in normal mode — while a prompt or minibuffer is open, keys are intercepted by those blocks *before* the keymap resolver runs, so a sequence can never start under a modal. The full Esc order is therefore:

> **prompt-dismiss Esc  >  minibuffer-dismiss Esc  >  pending-sequence-cancel Esc  >  filter-cancel Esc  >  (normal keymap dispatch of Esc)**

and, as an invariant, **opening a prompt or minibuffer clears any in-flight `pending_keys`** (so a half-typed sequence can't leak into or survive a modal). Concretely: the existing prompt/minibuffer Esc handling is unchanged and runs first; in normal mode, if `pending_keys` is non-empty, Esc clears it (and the status indicator) and is consumed; only if there is no pending sequence does Esc fall through to the existing filter-cancel / normal handling. This keeps pending-cancel from ever swallowing a modal-dismiss Esc, and vice-versa.

### 4.2 Pending state
The loop/editor holds `pending_keys: Vec<KeyChord>` (on `Editor` or the `reduce` path). On each key: append, `resolve`; act on `Resolution`. The status line shows the pending prefix while `Resolution::Pending`. Esc clears pending. This is the only new per-keystroke state.

### 4.3 Config syntax + patch-merge
- **`bind`** is a map `"<chord-sequence>" = "<command-id>"`; chord-sequence = space-separated chords; chord grammar: `ctrl-`, `alt-`, `shift-` modifier prefixes + a key name (`a`..`z`, `f1`..`f12`, `enter`, `tab`, `esc`, `space`, `left`/`right`/`up`/`down`, `backspace`, `\\` etc.). Examples: `"ctrl-x" = "cut"`, `"ctrl-k ctrl-s" = "save"`.
- **`unbind`** is a list of chord-sequences to remove from the base.
- **Preset selection is resolved BEFORE patching** (Codex spec-review fix): the **final merged** `keymap.preset` (across all layers — a later layer's `preset` wins) selects the base map; then ALL layers' `bind`/`unbind` patches are applied in precedence order (XDG → project → `--config`) on top of that one base. The `preset` key itself is NOT part of the patch pass. (So XDG-layer binds still apply even if a later layer changed the preset.)
- Each patch overrides/adds (`bind`) or removes (`unbind`) individual sequences; it does NOT replace the whole map.
- Mode forward-compat: a future modal mode uses `[keymap.normal]`/`[keymap.insert]` subtables; v1's bare `[keymap]` is sugar for `[keymap.normal]`. The trie carries the mode dimension now so this is non-breaking.
- **Validation:** every command-id string in the effective keymap is checked against the registry's known command ids at startup (via §5.1 name resolution); an unknown id → a startup warning naming the chord+id, and that binding is dropped (never crash). A malformed chord string → same treatment.

### 4.4 Presets
Two bundled base keymaps as in-code tables (data, not behavior):
- **`cua`** (default) — the current CUA bindings (Ctrl+C/X/V/S/Z/Y, Ctrl+E filter, Ctrl+T transform, the existing chords) expressed as keymap data. This makes the existing behavior data-driven with no user-visible change.
- **`wordstar`** (opt-in via `keymap.preset = "wordstar"`) — a WordStar-flavored base using the multi-key machinery (the `Ctrl-K`/`Ctrl-Q` two-key families) mapped onto existing command ids. Ships as a bundled preset; CUA stays the default so new users aren't surprised. (Faithfulness is best-effort over the commands that exist in v1; it grows as later sub-efforts add commands.)

## 5. Dispatch integration

### 5.1 Name-based registry resolution (the `CommandId` wrinkle)
`registry::CommandId(pub &'static str)` keys the registry, but config supplies **runtime** command-id strings. **Concrete approach (Codex spec-review): `impl std::borrow::Borrow<str> for CommandId`** (return the inner `&'static str`), then `resolve_name(&self, name: &str) -> Option<CommandId>` via `self.map.get_key_value(name).map(|(id, _)| *id)` — this recovers the existing `&'static str`-backed `CommandId` for a runtime `&str` **without leaking memory or allocating a new static** (the `HashMap<CommandId, Handler>` is queried by `&str` thanks to the `Borrow` impl; `get_key_value` hands back the stored static key). `Registry` exposes `resolve_name` (and may add a `dispatch_by_name` thin wrapper). **At startup, the keymap resolves every binding's command-id string to a `CommandId` via `resolve_name`**, dropping (with a warning) any that don't resolve — so the in-memory keymap stores resolved `CommandId`s and can never hold an unknown id at dispatch time.

### 5.2 Replacing the hardcoded path
`input::key_to_command_id` (the hardcoded match → `KeyAction` = `Id(CommandId)` | `Insert(char)`) is replaced by keymap resolution: a `KeyEvent` becomes a `KeyChord` (**`KeyChord::from_key_event` returns `None` unless `key.kind == KeyEventKind::Press`** — preserving the existing Press-only guard so key repeats/releases don't enter the trie), appended to `pending_keys`, resolved against the active mode's trie; a `Resolution::Command(id)` dispatches via the registry (the existing `reduce` → `reg.dispatch` boundary). The **printable-fallthrough (literal `Insert(char)`)** is preserved for an unmodified single printable key that resolves to `None`. **The legacy test-only `key_to_command` (`Command`-enum) keymap and the `step` test path** must be retired or rewritten to go through the new keymap-backed reducer (so the duplicate CUA table doesn't go stale) — flagged for the plan.

## 6. Session state (resume + marks store)

- Stored in the XDG state dir (reuse `swap::state_dir()`, 0700), as a single state file (a map of **canonical absolute path** → `{ cursor: usize, scroll: usize, marks: Map<char,usize>, mtime: i64, size: u64 }`). Single file (not per-document) keeps it simple and prunable.
- **Staleness guard (Codex spec-review fix):** keying by path alone can restore a stale position/marks into a *different* file at the same path (delete+recreate, branch checkout, rename reuse). So each record stores the file's **mtime + size** at save time; on open, restore **only if the current file's mtime+size match** the record (best-effort identity check, mirroring swap.rs's content-fingerprint precedent). On mismatch → discard that record's position/marks (start fresh), don't restore stale state.
- **Resume-at-position:** on opening a file with a matching stored entry and `state.resume = true`, restore cursor + scroll (still clamped to the current document length as a belt-and-suspenders). Out-of-range → clamp, don't error.
- **Marks store** lives here; the set-mark / jump-to-mark *commands* + nav are **5c** (5a only persists/loads the store).
- **Write policy:** debounced on save and on close/quit — NOT per keystroke (no hot-path IO). Best-effort: a failed state write warns once, never blocks or loses the document.
- **Size cap / prune:** `state.max_entries` (default 200); on write, evict the oldest entries beyond the cap (LRU by last-touched). Scratch buffers (no path) are never persisted.
- Atomic write reuses the established `save_atomic_bytes` discipline (temp→rename) so a crash can't corrupt the state file.

## 7. Error handling & edge cases

(Per §15: degrade, don't abort; never `unwrap`; never lose the document.)
- Missing config/state file → defaults / empty store (normal).
- Malformed config TOML → startup warning (file + line) + defaults.
- Unknown command-id or malformed chord in `[keymap]` → warning + that binding dropped.
- Unknown `keymap.preset` → warning + fall back to `cua`.
- Corrupt state file → warning + start with an empty store (don't lose the ability to edit).
- `--config <missing path>` → warning + defaults (explicit path that doesn't exist is a user error worth surfacing, but not fatal).
- Resume position past EOF (file changed on disk) → clamp.
- A pending key sequence open at quit → discarded (no effect).

## 8. Components / boundaries

| Unit | Responsibility | Depends on |
|------|----------------|------------|
| `config.rs::Config` + `load(cli_overrides)` | typed config, layered precedence, `--config`/`--no-config` | `toml`, `serde`, `dirs` |
| `keymap.rs::{KeyChord, KeyTrie, Resolution}` + `resolve` | data-driven multi-key resolution per mode | registry (name validation) |
| `keymap.rs` presets (`cua`, `wordstar`) + patch-merge | preset base + config patches → effective trie | config |
| `state.rs::SessionState` + load/save | path-keyed cursor/scroll/marks, prune, atomic write | `swap::state_dir`, `file::save_atomic_bytes` |
| `registry.rs` name resolution | resolve/validate a runtime command-id string | — |
| `app.rs` `run()` + reduce | load config/state at startup, pending-keys, dispatch via keymap, persist debounced, resume-on-open | all of the above |

## 9. Testing (fakes; no real $HOME)

- **Config precedence:** built-in < XDG < project < `--config` (each overrides only its keys); `--no-config` → defaults only; malformed TOML → defaults + warning; unknown keys **silently ignored** (forward-compat). (Inject config dir/paths so tests don't touch real `$HOME`.)
- **CLI parser:** bare path → opens it; `--config <p>` sets the config path (not opened as a document); `--no-config` skips file lookup; `--config` of a missing path → warning + defaults.
- **Keymap:** chord parsing (modifiers, key names, sequences, bad strings); `KeyChord::from_key_event` returns `None` for non-`Press` (release/repeat) events; multi-key resolution (`Pending` → `Command`, Esc-cancel clears pending, unknown continuation → `None` + clear); `bind` override/add + `unbind` remove; preset resolved-before-patch (XDG `preset=wordstar` + project `preset=cua` → cua base with XDG binds still applied); `cua` vs `wordstar` produce different effective maps; unknown command-id dropped + warned; printable fallthrough preserved.
- **Esc precedence:** with a pending sequence in normal mode, Esc cancels it (consumed); opening a prompt/minibuffer clears `pending_keys`; a prompt/minibuffer Esc still dismisses the modal (pending-cancel never swallows it).
- **Preset integrity (shipped-data test):** every command-id in BOTH bundled presets (`cua` and `wordstar`) resolves through `Registry::builtins()` — a bundled preset silently dropping a binding is a ship bug, not a user-config warning.
- **Registry name resolution:** `resolve_name` returns the stored `CommandId` for a known id and `None` for an unknown id; no allocation/leak.
- **Session state:** save→load round-trip restores cursor/scroll; **mtime+size mismatch → record discarded (no stale restore)**; resume clamps a past-EOF position; prune evicts beyond `max_entries` (LRU); scratch buffers not persisted; corrupt state file → empty store + warning; atomic write leaves no temp litter.
- No prior test weakened; `cargo build --workspace` zero warnings; `wordcartel-core` untouched.

## 10. Non-goals (explicit)

- **Command palette + hideable menu** → 5b (they consume this keymap/config to display each command's chord).
- **Mark/jump commands + word/page nav + mouse + text objects** → 5c (5a only persists the marks store).
- **Live config reload on file change** → load at startup only in v1 (reload is a later nicety).
- **Modal (vim) modes** → the trie carries the mode dimension, but v1 ships only `normal`/CUA; an actual modal mode is post-v1 config.
- **Per-sub-effort settings keys** (wrap column, focus, harper, palette chord) → each later 5x sub-effort adds its own keys to `Config`.
- **Theme / colors** → not in 5a (no consumer yet); a `theme` key + theming is §13 / a later sub-effort, added to `Config` when it has a consumer.
