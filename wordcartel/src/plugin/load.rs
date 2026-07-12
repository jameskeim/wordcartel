//! The filesystem-free load core ([`load_sources`], Task 4) and the filesystem/config
//! discovery layer (`discover`, Task 6) that drives it: exec each plugin source into the host
//! VM, cap its raw registrations, then commit them into the `Registry` atomically per plugin via
//! a two-phase commit (P2 §7b) — intern + write every `wc-cmd-<id>` callback key FALLIBLY first,
//! Registry mutation only once that fully succeeds. A failing plugin (parse error, an over-cap
//! string, a collision) registers ZERO commands, isolated from every other plugin in the batch —
//! except a commit-time `mlua` write failure (VM exhaustion), which is fatal for the WHOLE batch
//! and reverts the `Registry` to builtins-only.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::limits::{PLUGIN_MAX_COMMANDS_PER_PLUGIN, PLUGIN_MAX_SOURCE_BYTES, PLUGIN_MAX_STEM_LEN};
use crate::plugin::host::{HookEntry, PendingReg, PluginHost};
use crate::plugin::PluginEventKind;
use crate::registry::{CommandId, MenuCategory, Registry};

/// The outcome of loading ONE plugin source: `Ok(n_commands)` registered, or `Err(reason)` — a
/// parse/exec error, an over-length stem/name/label, a per-plugin count cap, or a collision.
/// A plugin is atomic: any `Err` means ZERO of its commands were committed to the `Registry`.
pub struct LoadReport {
    pub plugin: String,
    pub result: Result<usize, String>,
    /// Committed `wc.on` hook count (P2) — a sibling of `result`, not folded into it, so every
    /// existing `Ok(n)` command-count assertion stays untouched. Always `0` alongside an `Err`
    /// (a failed/skipped plugin commits zero hooks — the atomic-per-plugin guarantee now spans
    /// both registration verbs).
    pub hooks: usize,
}

/// The outcome of scanning the plugins dir: loadable (stem, source) pairs + a report of files
/// skipped (oversize / unreadable) so the caller can surface them to the status line. A
/// `disable`-listed stem is excluded silently (an intentional user choice, not a failure) — it
/// appears in neither `sources` nor `skipped`.
pub struct Discovered {
    pub sources: Vec<(String, String)>,
    pub skipped: Vec<LoadReport>,
}

/// Filesystem-facing discovery layer (Task 6) — scans `dir` for single-file `<name>.lua` and
/// `<name>/init.lua` plugins, `disable`-filters, and bounded-reads each source. Deterministic:
/// candidates are sorted lexicographically BY STEM before reading, so load order (and hence any
/// cross-plugin collision report) never depends on `read_dir`'s OS-dependent enumeration order.
///
/// A missing/unreadable `dir` (no plugins installed — the common case) is not a failure: it
/// yields an empty `Discovered`, no warning. A candidate that IS found but is over
/// [`PLUGIN_MAX_SOURCE_BYTES`] or fails to read/decode goes to `skipped` — named, never silently
/// dropped — and is excluded from `sources` (the caller never sees its content). This function
/// does not touch the `Fs` trait (write-only seam) or the string-core `load_sources`: it only
/// reads bytes and hands `(stem, source)` pairs onward.
pub fn discover(dir: &Path, disable: &[String]) -> Discovered {
    let mut candidates: Vec<(String, PathBuf)> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else { continue };
            if file_type.is_file() {
                if path.extension().and_then(|e| e.to_str()) == Some("lua") {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        candidates.push((stem.to_string(), path));
                    }
                }
            } else if file_type.is_dir() {
                let init = path.join("init.lua");
                if init.is_file() {
                    if let Some(stem) = path.file_name().and_then(|s| s.to_str()) {
                        candidates.push((stem.to_string(), init));
                    }
                }
            }
        }
    }
    candidates.sort_by(|a, b| a.0.cmp(&b.0));

    let mut sources = Vec::new();
    let mut skipped = Vec::new();
    let mut i = 0;
    while i < candidates.len() {
        let stem = candidates[i].0.clone();
        if disable.iter().any(|d| d == &stem) {
            i += 1;
            continue; // user opt-out — not a load failure, excluded from both lists.
        }
        let mut j = i + 1;
        while j < candidates.len() && candidates[j].0 == stem {
            j += 1;
        }
        if j - i > 1 {
            // Same stem resolved by both `<stem>.lua` and `<stem>/init.lua` — ambiguous.
            // Load neither; one report for the stem, not one per colliding file.
            skipped.push(LoadReport {
                plugin: stem.clone(),
                result: Err(format!(
                    "ambiguous plugin '{stem}': both {stem}.lua and {stem}/init.lua exist — remove one"
                )),
                hooks: 0,
            });
            i = j;
            continue;
        }
        let path = &candidates[i].1;
        match crate::file::bounded_read_opt(path, PLUGIN_MAX_SOURCE_BYTES) {
            Some(bytes) => match String::from_utf8(bytes) {
                Ok(src) => sources.push((stem, src)),
                Err(_) => skipped.push(LoadReport {
                    plugin: stem,
                    result: Err("plugin source is not valid UTF-8".to_string()),
                    hooks: 0,
                }),
            },
            None => skipped.push(LoadReport {
                plugin: stem,
                result: Err(format!(
                    "plugin source unreadable or over {PLUGIN_MAX_SOURCE_BYTES} bytes"
                )),
                hooks: 0,
            }),
        }
        i = j;
    }
    Discovered { sources, skipped }
}

/// Why loading ONE plugin failed. `Validation` → skip this plugin, batch continues (P1
/// isolation). `VmExhausted` → a commit-time mlua write failed: the VM is exhausted, fatal for
/// the whole load_phase (§7b) — the caller nulls the host + reverts the registry.
pub enum LoadFailure {
    /// Skip this plugin, batch continues (P1 per-plugin isolation) — a parse/exec error, an
    /// over-cap string, or a collision.
    Validation(String),
    /// A commit-time `mlua` write failed: the VM is exhausted, fatal for the whole batch —
    /// [`load_sources`] nulls the host and reverts the registry to builtins-only.
    VmExhausted(String),
}

/// Filesystem-free load core: exec each `(stem, source)` into the host VM, collect
/// registrations, commit into `reg` ATOMICALLY per plugin. Per-plugin VALIDATION failure is
/// isolated — one bad plugin does not stop the batch — AND all-or-nothing — a failing plugin
/// leaves ZERO commands registered. The null host (`host.lua()` is `None`) is a no-op: nothing
/// loads, an empty report list.
///
/// A commit-time `mlua` write failure (VM exhaustion), by contrast, is FATAL for the whole
/// batch (§7b): `host` is `&mut` so this case can null the VM AND revert `reg` to
/// builtins-only (`retain_builtins`) — no earlier-committed plugin from this batch is left
/// pointing at a dead VM.
///
/// Every host→Lua entry point is wrapped in [`crate::panicx::catch`] — the Task 1 spike found
/// `mlua` does NOT convert a Rust panic to an `Err` at the call site (it resumes the raw
/// panic), so `catch_unwind` here is the SOLE backstop, not a redundant one.
///
/// `config_map` is each plugin's `[plugins.config.<name>]` TOML table, looked up by stem —
/// installed as `wc.config` for that plugin's own load (P2 Task 5). An over-cap config value
/// (depth/nodes/byte — `plugin::settings::config_to_lua`) does not fail the plugin: it degrades
/// to `wc.config = nil` plus a warning pushed onto `warns`, the caller-owned channel this
/// function already shares with skipped/failed plugins.
pub fn load_sources(
    reg: &mut Registry,
    host: &mut PluginHost,
    sources: &[(String, String)],
    config_map: &BTreeMap<String, toml::Value>,
    warns: &mut Vec<String>,
) -> Vec<LoadReport> {
    if host.lua().is_none() {
        return Vec::new();
    }
    let mut reports = Vec::new();
    let mut fatal = false;
    for (stem_raw, src) in sources {
        let lua = host.lua().expect("checked above"); // per-iteration borrow, released before loop end
        let config = config_map.get(stem_raw.as_str());
        let outcome = crate::panicx::catch(|| load_one(reg, lua, stem_raw, src, config, warns))
            .unwrap_or_else(|panic_msg| Err(LoadFailure::Validation(panic_msg)));
        match outcome {
            Ok((n, hookvec)) => {
                // Captured BEFORE the move, per §7b discipline: the `lua` per-iteration borrow
                // above is released by the time `load_one` returns, so `&mut host` is free here.
                let n_hooks = hookvec.len();
                host.append_hooks(hookvec);
                reports.push(LoadReport { plugin: stem_raw.clone(), result: Ok(n), hooks: n_hooks });
            }
            Err(LoadFailure::Validation(msg)) =>
                reports.push(LoadReport { plugin: stem_raw.clone(), result: Err(msg), hooks: 0 }),
            Err(LoadFailure::VmExhausted(msg)) => {
                reports.push(LoadReport { plugin: stem_raw.clone(), result: Err(msg), hooks: 0 });
                fatal = true;
                break; // stop the batch — the VM is unusable
            }
        }
    }
    if fatal {
        *host = PluginHost::null(); // drops the whole VM + every wc-cmd-* key/closure
        reg.retain_builtins(); // registry-level revert — discards EVERY plugin committed so far
    }
    reports
}

#[cfg(test)]
thread_local! {
    /// Test-only override for [`load_budget`] — lets a test drive `load_one`'s exec-phase
    /// guard with a tiny budget instead of the real `LOAD_TIME_BUDGET`, so a runaway-abort test
    /// completes in milliseconds rather than waiting out the real 1s budget. Mirrors the
    /// `FAIL_NEXT_COMMIT_WRITE` fault-seam pattern in `plugin::host`.
    pub(crate) static LOAD_BUDGET_OVERRIDE: Cell<Option<std::time::Duration>> = const { Cell::new(None) };
}

/// The exec-phase time budget for [`load_one`]: [`crate::plugin::host::LOAD_TIME_BUDGET`] in
/// release, or the `#[cfg(test)]` override when a test has armed one via
/// [`LOAD_BUDGET_OVERRIDE`].
fn load_budget() -> std::time::Duration {
    #[cfg(test)]
    {
        if let Some(d) = LOAD_BUDGET_OVERRIDE.with(std::cell::Cell::get) {
            return d;
        }
    }
    crate::plugin::host::LOAD_TIME_BUDGET
}

/// Load + register ONE plugin, atomically, via a two-phase commit (P2 §7b).
///
/// 1. Cap the stem (a Rust-level `&str` — the caller's, not plugin-Lua-supplied — so it needs
///    no `mlua::String` extraction) against `PLUGIN_MAX_STEM_LEN`. NOT interned here — the stem
///    intern moved into the commit phase below, alongside every other interned string.
/// 2. Install a FRESH registration sink (`wc.register_command`) scoped to this plugin, then
///    exec its source (guarded by [`load_budget`]'s [`with_time_guard`](crate::plugin::host::with_time_guard)
///    — a runaway top-level loop aborts rather than hanging startup, P2 §7a). A parse/exec/guard
///    error bails with nothing committed — `install_registration` does no interning and no
///    `set_named_registry_value` during exec, so there is nothing to unwind: the sink's raw
///    `PendingReg` entries (if any were pushed before the error) are simply dropped, harmless.
/// 3. Preflight EVERY pending registration on its RAW `name_full` string — a collision with the
///    live `Registry` OR with another entry in this same batch, and a re-confirmed per-plugin
///    count cap — BEFORE any Lua-side write happens.
/// 4. Commit phase 1 (FALLIBLE): intern the stem-qualified name + label and write each
///    `wc-cmd-<id>` callback key, for every survivor, collecting `(id, label, menu)` — the
///    `Registry` is untouched throughout. A write failing here (VM exhaustion) returns
///    `LoadFailure::VmExhausted` immediately, before any `Registry` mutation.
/// 5. Commit phase 2 (INFALLIBLE): only reached if phase 1 fully succeeded — `register_plugin`
///    for every committed entry. The preflight already ruled out the sole possible `Duplicate`,
///    so a stray `Err` there would be a logic bug, not a plugin-author mistake — an `.expect`
///    documents that invariant.
///
/// Between registration and exec, installs `wc.config` (P2 Task 5) from `config` — the
/// plugin's own `[plugins.config.<stem>]` TOML table, or `None` if it has none. Converting it
/// (`plugin::settings::config_to_lua`) can fail on the depth/nodes/byte caps; that failure does
/// NOT abort the plugin — it pushes a warning onto `warns` and installs `wc.config = nil`
/// instead, so an over-cap config degrades gracefully rather than losing the plugin's commands.
fn load_one(
    reg: &mut Registry,
    lua: &mlua::Lua,
    stem_raw: &str,
    src: &str,
    config: Option<&toml::Value>,
    warns: &mut Vec<String>,
) -> Result<(usize, Vec<HookEntry>), LoadFailure> {
    if stem_raw.len() > PLUGIN_MAX_STEM_LEN {
        return Err(LoadFailure::Validation(format!(
            "plugin stem too long ({} bytes, max {PLUGIN_MAX_STEM_LEN})",
            stem_raw.len()
        )));
    }
    let sink: Rc<RefCell<Vec<PendingReg>>> = Rc::new(RefCell::new(Vec::new()));
    let count = Rc::new(Cell::new(0usize));
    // `stem` is OWNED into the closure now; set_name/collision-check take &str, so nothing
    // before commit needs &'static (the stem intern moved into the commit phase below).
    crate::plugin::api::install_registration(lua, stem_raw.to_owned(), sink.clone(), count.clone())
        .map_err(|e| LoadFailure::Validation(format!("plugin {stem_raw}: {e}")))?;
    // P2 §3b: the second registration verb, same shape — a fresh per-plugin sink, capped at
    // push time (install_on enforces PLUGIN_MAX_HOOKS_PER_PLUGIN).
    let hook_sink: Rc<RefCell<Vec<(PluginEventKind, mlua::Function)>>> = Rc::new(RefCell::new(Vec::new()));
    crate::plugin::api::install_on(lua, hook_sink.clone())
        .map_err(|e| LoadFailure::Validation(format!("plugin {stem_raw}: {e}")))?;
    let cfg_value = match config {
        Some(v) => match crate::plugin::settings::config_to_lua(lua, v) {
            Ok(lv) => lv,
            Err(reason) => {
                // over-cap → nil, plugin STILL loads, but WARN loudly.
                warns.push(format!("plugin {stem_raw}: [plugins.config.{stem_raw}] ignored — {reason}"));
                mlua::Value::Nil
            }
        },
        None => mlua::Value::Nil,
    };
    crate::plugin::settings::install_config(lua, cfg_value)
        .map_err(|e| LoadFailure::Validation(format!("plugin {stem_raw}: {e}")))?;
    crate::plugin::host::with_time_guard(lua, load_budget(), || lua.load(src).set_name(stem_raw).exec())
        .map_err(|e| LoadFailure::Validation(format!("plugin {stem_raw}: {e}")))?;

    let pending: Vec<PendingReg> = sink.borrow_mut().drain(..).collect();
    let hook_pending: Vec<(PluginEventKind, mlua::Function)> = hook_sink.borrow_mut().drain(..).collect();
    if pending.len() > PLUGIN_MAX_COMMANDS_PER_PLUGIN {
        return Err(LoadFailure::Validation(format!(
            "plugin {stem_raw}: too many commands ({}, max {PLUGIN_MAX_COMMANDS_PER_PLUGIN})",
            pending.len()
        )));
    }
    // Preflight on RAW strings — every id must be free of the LIVE registry AND unique within
    // this batch — checked BEFORE any Lua-side write or Registry mutation (the atomic-per-plugin
    // guarantee).
    let mut seen = HashSet::new();
    for p in &pending {
        if reg.resolve_name(&p.name_full).is_some() || !seen.insert(p.name_full.as_str()) {
            return Err(LoadFailure::Validation(format!(
                "plugin {stem_raw}: duplicate command id {}",
                p.name_full
            )));
        }
    }
    // ── Commit phase 1 (FALLIBLE): intern + write every wc-cmd-<id> key, Registry untouched. ──
    let mut committed: Vec<(CommandId, &'static str, Option<MenuCategory>, Option<&'static str>)> =
        Vec::with_capacity(pending.len());
    for p in &pending {
        let id = CommandId(crate::plugin::intern(&p.name_full));
        let label: &'static str = crate::plugin::intern(&p.label);
        // Fault seam: deterministic exhaustion test (no real OOM needed).
        #[cfg(test)]
        if crate::plugin::host::FAIL_NEXT_COMMIT_WRITE.with(|c| c.replace(false)) {
            return Err(LoadFailure::VmExhausted(format!("plugin {stem_raw}: VM exhausted (test)")));
        }
        lua.set_named_registry_value(&format!("wc-cmd-{}", id.0), p.func.clone())
            .map_err(|e| LoadFailure::VmExhausted(format!("plugin {stem_raw}: {e}")))?;
        committed.push((id, label, p.menu, p.arg.as_deref().map(crate::plugin::intern)));
    }
    // ── Commit phase 1 continued: intern + write every wc-ev-<stem>-<i> hook key, same
    // fault-seam-guarded fallible path as the command keys above (P2 §3b). Hooks have no
    // collision axis (anonymous by per-plugin index), so nothing to preflight beyond the
    // push-time count cap `install_on` already enforced.
    let mut committed_hooks: Vec<HookEntry> = Vec::with_capacity(hook_pending.len());
    for (i, (kind, func)) in hook_pending.into_iter().enumerate() {
        let key = format!("wc-ev-{stem_raw}-{i}");
        // Fault seam: deterministic exhaustion test (no real OOM needed).
        #[cfg(test)]
        if crate::plugin::host::FAIL_NEXT_COMMIT_WRITE.with(|c| c.replace(false)) {
            return Err(LoadFailure::VmExhausted(format!("plugin {stem_raw}: VM exhausted (test)")));
        }
        lua.set_named_registry_value(&key, func)
            .map_err(|e| LoadFailure::VmExhausted(format!("plugin {stem_raw}: {e}")))?;
        let label = format!("{stem_raw}.on_{}", crate::plugin::kind_str(kind));
        committed_hooks.push(HookEntry { kind, key, label });
    }
    // ── Commit phase 2 (INFALLIBLE): Registry mutation only — preflight ruled out Duplicate. ──
    let n = committed.len();
    for (id, label, menu, arg) in committed {
        reg.register_plugin(id, label, menu, arg)
            .expect("preflight already ruled out every possible Duplicate for this plugin");
    }
    Ok((n, committed_hooks))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_time_budget_aborts_runaway_toplevel() {
        // A tiny test budget so a genuinely runaway top-level loop is caught in milliseconds,
        // not the real 1s LOAD_TIME_BUDGET.
        LOAD_BUDGET_OVERRIDE.with(|c| c.set(Some(std::time::Duration::from_millis(20))));
        let mut reg = Registry::builtins();
        let mut host = PluginHost::new().unwrap();
        let runaway = "while true do end";
        let good = "wc.register_command{ name='hello', label='Hello', fn=function() end }";
        let reports = load_sources(
            &mut reg, &mut host,
            &sources(&[("runaway", runaway), ("good", good)]),
            &BTreeMap::new(), &mut Vec::new(),
        );
        LOAD_BUDGET_OVERRIDE.with(|c| c.set(None)); // disarm defensively — no cross-test leak
        assert_eq!(reports.len(), 2);
        let err = reports[0].result.as_ref().expect_err("a runaway top-level loop must be aborted");
        assert!(err.to_lowercase().contains("budget"), "error should mention a budget: {err}");
        assert_eq!(
            reports[1].result,
            Ok(1),
            "batch continues — a good plugin after the runaway one still registers"
        );
        assert!(reg.resolve_name("good.hello").is_some());
    }

    #[test]
    fn discover_reads_single_file_and_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.lua"), "-- a").unwrap();
        std::fs::create_dir(dir.path().join("b")).unwrap();
        std::fs::write(dir.path().join("b").join("init.lua"), "-- b").unwrap();
        let disc = discover(dir.path(), &[]);
        assert!(disc.skipped.is_empty());
        let stems: Vec<&str> = disc.sources.iter().map(|(s, _)| s.as_str()).collect();
        assert_eq!(stems, vec!["a", "b"], "lexicographic by stem");
        assert_eq!(disc.sources[0].1, "-- a");
        assert_eq!(disc.sources[1].1, "-- b");
    }

    #[test]
    fn discover_rejects_ambiguous_same_stem() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("foo.lua"), "-- foo file").unwrap();
        std::fs::create_dir(dir.path().join("foo")).unwrap();
        std::fs::write(dir.path().join("foo").join("init.lua"), "-- foo dir").unwrap();
        std::fs::write(dir.path().join("bar.lua"), "-- bar").unwrap();
        let disc = discover(dir.path(), &[]);
        let stems: Vec<&str> = disc.sources.iter().map(|(s, _)| s.as_str()).collect();
        assert!(!stems.contains(&"foo"), "an ambiguous stem loads neither candidate");
        assert_eq!(stems, vec!["bar"], "an unrelated stem still loads");
        assert_eq!(disc.skipped.len(), 1, "one report for the ambiguous stem, not one per file");
        assert_eq!(disc.skipped[0].plugin, "foo");
        let err = disc.skipped[0].result.as_ref().expect_err("ambiguous stem is a failure");
        let err_lower = err.to_lowercase();
        assert!(err_lower.contains("ambiguous"), "error should say ambiguous: {err}");
        assert!(err_lower.contains("remove one"), "error should say remove one: {err}");
    }

    #[test]
    fn discover_skips_disabled() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.lua"), "-- a").unwrap();
        std::fs::write(dir.path().join("z.lua"), "-- z").unwrap();
        let disc = discover(dir.path(), &["a".to_string()]);
        assert!(disc.skipped.is_empty(), "a disabled name is excluded, not reported as a failure");
        let stems: Vec<&str> = disc.sources.iter().map(|(s, _)| s.as_str()).collect();
        assert_eq!(stems, vec!["z"]);
    }

    #[test]
    fn discover_reports_skipped_oversize() {
        let dir = tempfile::tempdir().unwrap();
        let huge = "x".repeat(crate::limits::PLUGIN_MAX_SOURCE_BYTES as usize + 1);
        std::fs::write(dir.path().join("big.lua"), &huge).unwrap();
        std::fs::write(dir.path().join("ok.lua"), "-- ok").unwrap();
        let disc = discover(dir.path(), &[]);
        assert_eq!(disc.sources.len(), 1);
        assert_eq!(disc.sources[0].0, "ok");
        assert_eq!(disc.skipped.len(), 1);
        assert_eq!(disc.skipped[0].plugin, "big");
        assert!(disc.skipped[0].result.is_err(), "named in skipped, not silently dropped");
    }

    #[test]
    fn discover_missing_dir_is_empty_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist");
        let disc = discover(&missing, &[]);
        assert!(disc.sources.is_empty());
        assert!(disc.skipped.is_empty());
    }
    use crate::registry::MenuCategory;

    fn sources(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(s, src)| (s.to_string(), src.to_string())).collect()
    }

    #[test]
    fn load_registers_command_into_registry() {
        let mut reg = Registry::builtins();
        let mut host = PluginHost::new().unwrap();
        let src = "wc.register_command{ name='hello', label='Hello', fn=function() end }";
        let reports = load_sources(&mut reg, &mut host, &sources(&[("greet", src)]), &BTreeMap::new(), &mut Vec::new());
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].result, Ok(1));
        let id = reg.resolve_name("greet.hello").expect("registered");
        let meta = reg.meta(id).unwrap();
        assert_eq!(meta.label, "Hello");
        assert_eq!(meta.menu, None);
        // callback stored under the id's registry key and callable.
        let cb: mlua::Function = host.lua().unwrap()
            .named_registry_value(&format!("wc-cmd-{}", id.0)).expect("callback stored");
        cb.call::<()>(()).expect("callback runs");
    }

    #[test]
    fn load_namespaces_id_by_stem() {
        let mut reg = Registry::builtins();
        let mut host = PluginHost::new().unwrap();
        let src = "wc.register_command{ name='insert', label='Insert', fn=function() end }";
        load_sources(&mut reg, &mut host, &sources(&[("date", src)]), &BTreeMap::new(), &mut Vec::new());
        assert!(reg.resolve_name("date.insert").is_some());
        assert!(reg.resolve_name("insert").is_none());
    }

    #[test]
    fn load_rejects_over_length_name() {
        let mut reg = Registry::builtins();
        let before = reg.commands().count();
        let mut host = PluginHost::new().unwrap();
        let long = "a".repeat(crate::limits::PLUGIN_MAX_NAME_LEN + 1);
        let src = format!("wc.register_command{{ name='{long}', label='L', fn=function() end }}");
        let reports = load_sources(&mut reg, &mut host, &sources(&[("p", &src)]), &BTreeMap::new(), &mut Vec::new());
        assert!(reports[0].result.is_err());
        assert_eq!(reg.commands().count(), before, "nothing interned into the registry");
    }

    #[test]
    fn load_rejects_over_length_label() {
        let mut reg = Registry::builtins();
        let before = reg.commands().count();
        let mut host = PluginHost::new().unwrap();
        let long = "a".repeat(crate::limits::PLUGIN_MAX_LABEL_LEN + 1);
        let src = format!("wc.register_command{{ name='x', label='{long}', fn=function() end }}");
        let reports = load_sources(&mut reg, &mut host, &sources(&[("p", &src)]), &BTreeMap::new(), &mut Vec::new());
        assert!(reports[0].result.is_err());
        assert_eq!(reg.commands().count(), before);
    }

    #[test]
    fn load_rejects_over_length_stem() {
        let mut reg = Registry::builtins();
        let before = reg.commands().count();
        let mut host = PluginHost::new().unwrap();
        let long_stem = "s".repeat(crate::limits::PLUGIN_MAX_STEM_LEN + 1);
        let src = "wc.register_command{ name='x', label='X', fn=function() end }";
        let reports = load_sources(&mut reg, &mut host, &sources(&[(&long_stem, src)]), &BTreeMap::new(), &mut Vec::new());
        assert!(reports[0].result.is_err());
        assert_eq!(reg.commands().count(), before);
    }

    #[test]
    fn load_rejects_257th_command() {
        let mut reg = Registry::builtins();
        let before = reg.commands().count();
        let mut host = PluginHost::new().unwrap();
        let src = "for i=1,257 do \
                       wc.register_command{ name='cmd'..i, label='L'..i, fn=function() end } \
                   end";
        let reports = load_sources(&mut reg, &mut host, &sources(&[("many", src)]), &BTreeMap::new(), &mut Vec::new());
        assert!(reports[0].result.is_err());
        assert_eq!(reg.commands().count(), before, "nothing interned into the registry");
    }

    #[test]
    fn load_rejects_bad_menu() {
        let mut reg = Registry::builtins();
        let before = reg.commands().count();
        let mut host = PluginHost::new().unwrap();
        let src = "wc.register_command{ name='x', label='X', menu='Nonsense', fn=function() end }";
        let reports = load_sources(&mut reg, &mut host, &sources(&[("p", src)]), &BTreeMap::new(), &mut Vec::new());
        assert!(reports[0].result.is_err());
        assert_eq!(reg.commands().count(), before);
    }

    #[test]
    fn load_accepts_known_menu() {
        let mut reg = Registry::builtins();
        let mut host = PluginHost::new().unwrap();
        let src = "wc.register_command{ name='x', label='X', menu='Edit', fn=function() end }";
        let reports = load_sources(&mut reg, &mut host, &sources(&[("p", src)]), &BTreeMap::new(), &mut Vec::new());
        assert_eq!(reports[0].result, Ok(1));
        let id = reg.resolve_name("p.x").unwrap();
        assert_eq!(reg.meta(id).unwrap().menu, Some(MenuCategory::Edit));
    }

    /// `wc.register_command{ …, arg='Minutes:' }` → `reg.meta(id).arg == Some("Minutes:")`;
    /// absent → `None` (Task 5).
    #[test]
    fn register_command_with_arg_prompt() {
        let mut reg = Registry::builtins();
        let mut host = PluginHost::new().unwrap();
        let src = "wc.register_command{ name='x', label='X', arg='Minutes:', fn=function(arg) end }";
        let reports = load_sources(&mut reg, &mut host, &sources(&[("p", src)]), &BTreeMap::new(), &mut Vec::new());
        assert_eq!(reports[0].result, Ok(1));
        let id = reg.resolve_name("p.x").unwrap();
        assert_eq!(reg.meta(id).unwrap().arg, Some("Minutes:"));

        // Absent `arg` → None (the existing no-arg registration path is unaffected).
        let mut reg2 = Registry::builtins();
        let mut host2 = PluginHost::new().unwrap();
        let src2 = "wc.register_command{ name='y', label='Y', fn=function() end }";
        let reports2 = load_sources(&mut reg2, &mut host2, &sources(&[("p", src2)]), &BTreeMap::new(), &mut Vec::new());
        assert_eq!(reports2[0].result, Ok(1));
        let id2 = reg2.resolve_name("p.y").unwrap();
        assert_eq!(reg2.meta(id2).unwrap().arg, None);
    }

    /// An `arg` prompt string longer than `PLUGIN_MAX_LABEL_LEN` is rejected at load time,
    /// nothing interned into the registry (mirrors `load_rejects_over_length_label`).
    #[test]
    fn load_rejects_over_length_arg_prompt() {
        let mut reg = Registry::builtins();
        let before = reg.commands().count();
        let mut host = PluginHost::new().unwrap();
        let long = "a".repeat(crate::limits::PLUGIN_MAX_LABEL_LEN + 1);
        let src = format!("wc.register_command{{ name='x', label='X', arg='{long}', fn=function(arg) end }}");
        let reports = load_sources(&mut reg, &mut host, &sources(&[("p", &src)]), &BTreeMap::new(), &mut Vec::new());
        assert!(reports[0].result.is_err());
        assert_eq!(reg.commands().count(), before);
    }

    #[test]
    fn load_reports_collision() {
        let mut reg = Registry::builtins();
        let mut host = PluginHost::new().unwrap();
        let src = "wc.register_command{ name='x', label='X', fn=function() end }";
        let reports = load_sources(
            &mut reg, &mut host,
            &sources(&[("p", src), ("p", src)]),
            &BTreeMap::new(), &mut Vec::new(),
        );
        assert_eq!(reports.len(), 2);
        assert_eq!(reports[0].result, Ok(1));
        assert!(reports[1].result.is_err(), "second plugin collides on the same id");
        assert_eq!(reg.commands().filter(|(id, _)| id.0 == "p.x").count(), 1);
    }

    #[test]
    fn load_is_atomic_per_plugin() {
        let mut reg = Registry::builtins();
        let before = reg.commands().count();
        let mut host = PluginHost::new().unwrap();
        // Same plugin, same name registered twice — a self-collision within one exec pass.
        let src = "wc.register_command{ name='x', label='X1', fn=function() end }\n\
                   wc.register_command{ name='x', label='X2', fn=function() end }";
        let reports = load_sources(&mut reg, &mut host, &sources(&[("atomic", src)]), &BTreeMap::new(), &mut Vec::new());
        assert!(reports[0].result.is_err());
        assert_eq!(reg.commands().count(), before, "neither command committed");
        assert!(reg.resolve_name("atomic.x").is_none());
    }

    #[test]
    fn load_parse_error_is_reported_not_fatal() {
        let mut reg = Registry::builtins();
        let mut host = PluginHost::new().unwrap();
        let good = "wc.register_command{ name='hello', label='Hello', fn=function() end }";
        let bad = "end"; // syntax error — unexpected 'end'
        let reports = load_sources(
            &mut reg, &mut host,
            &sources(&[("bad", bad), ("good", good)]),
            &BTreeMap::new(), &mut Vec::new(),
        );
        assert_eq!(reports.len(), 2);
        assert!(reports[0].result.is_err());
        assert_eq!(reports[1].result, Ok(1));
        assert!(reg.resolve_name("good.hello").is_some());
    }

    #[test]
    fn load_validation_failure_interns_nothing_and_writes_no_callback_key() {
        let mut reg = Registry::builtins();
        let mut host = PluginHost::new().unwrap();
        let commands_before = reg.commands().count();
        // Same plugin, same name registered twice — preflight rejects the collision on RAW
        // strings BEFORE the commit phase runs, so this plugin never reaches phase 1's intern /
        // set_named_registry_value at all (§7b: a validation failure leaks nothing). "atomic.x"
        // is used ONLY by this test and `load_is_atomic_per_plugin` (also a preflight-rejected
        // scenario that never interns it either), so a membership probe on it stays correct
        // regardless of what other tests intern concurrently under `cargo test`'s default
        // parallel execution — unlike a whole-pool-size before/after comparison, which is racy
        // (see `intern_pool_contains`'s doc).
        let src = "wc.register_command{ name='x', label='X1', fn=function() end }\n\
                   wc.register_command{ name='x', label='X2', fn=function() end }";
        let reports = load_sources(&mut reg, &mut host, &sources(&[("atomic", src)]), &BTreeMap::new(), &mut Vec::new());
        assert!(reports[0].result.is_err());
        assert_eq!(reg.commands().count(), commands_before, "nothing committed to the registry");
        assert!(
            !crate::plugin::intern_pool_contains("atomic.x"),
            "a validation failure must intern zero strings — the leak the two-phase commit closes"
        );
        assert!(
            host.lua().unwrap().named_registry_value::<mlua::Function>("wc-cmd-atomic.x").is_err(),
            "no wc-cmd-<id> callback key may exist for a plugin that failed preflight"
        );
    }

    #[test]
    fn commit_time_exhaustion_reverts_registry_and_nulls_host() {
        let mut reg = Registry::builtins();
        let mut host = PluginHost::new().unwrap();
        let builtins_count = reg.commands().count();
        let src_a = "wc.register_command{ name='cmd', label='A', fn=function() end }";
        let src_b = "wc.register_command{ name='cmd', label='B', fn=function() end }";

        // Plugin a loads and commits cleanly, in its own batch.
        let reports_a = load_sources(&mut reg, &mut host, &sources(&[("a", src_a)]), &BTreeMap::new(), &mut Vec::new());
        assert_eq!(reports_a[0].result, Ok(1), "plugin a must commit cleanly before the fault fires");
        assert!(reg.resolve_name("a.cmd").is_some(), "a's command is live before b's fatal load");

        // Arm the fault seam — the NEXT fallible commit-phase write (plugin b's only pending
        // entry) synthesizes a VM-exhaustion error, deterministically, no real OOM needed.
        crate::plugin::host::FAIL_NEXT_COMMIT_WRITE.with(|c| c.set(true));
        let reports_b = load_sources(&mut reg, &mut host, &sources(&[("b", src_b)]), &BTreeMap::new(), &mut Vec::new());
        assert!(reports_b[0].result.is_err(), "plugin b's commit-phase write is forced to fail");

        // The fatal revert is registry-WIDE (retain_builtins), not scoped to the failing batch —
        // a's already-committed entry from the EARLIER call must be gone too.
        assert!(reg.resolve_name("a.cmd").is_none(), "a's entry must be gone too, not just b's");
        assert_eq!(reg.commands().count(), builtins_count, "registry is builtins-only after the revert");
        assert!(!host.has_vm(), "the VM must be nulled on a commit-time exhaustion");
    }

    #[test]
    fn config_reaches_wc_config() {
        // The load-time pattern a config-consuming plugin uses: capture `wc.config` into a Lua
        // LOCAL at load (it is only valid during that plugin's own load — see api.rs's
        // install_config_cleared), then read the captured local from inside the deferred
        // register_command `fn`.
        let src = "\
            local cfg = wc.config\n\
            wc.register_command{ name='check', label='Check', fn=function()\n\
                if cfg == nil then RESULT = 'nil' else RESULT = tostring(cfg.min_words) end\n\
            end }";

        let mut reg = Registry::builtins();
        let mut host = PluginHost::new().unwrap();
        let mut config_map: BTreeMap<String, toml::Value> = BTreeMap::new();
        let mut table = toml::map::Map::new();
        table.insert("min_words".to_string(), toml::Value::Integer(100));
        config_map.insert("withcfg".to_string(), toml::Value::Table(table));
        let mut warns = Vec::new();
        let reports = load_sources(
            &mut reg, &mut host,
            &sources(&[("withcfg", src), ("nocfg", src)]),
            &config_map, &mut warns,
        );
        assert_eq!(reports[0].result, Ok(1));
        assert_eq!(reports[1].result, Ok(1));
        assert!(warns.is_empty());

        let lua = host.lua().unwrap();
        let id_with = reg.resolve_name("withcfg.check").unwrap();
        let cb: mlua::Function =
            lua.named_registry_value(&format!("wc-cmd-{}", id_with.0)).unwrap();
        cb.call::<()>(()).unwrap();
        assert_eq!(
            lua.globals().get::<String>("RESULT").unwrap(), "100",
            "withcfg's captured local reads back its [plugins.config.withcfg] value"
        );

        let id_no = reg.resolve_name("nocfg.check").unwrap();
        let cb: mlua::Function =
            lua.named_registry_value(&format!("wc-cmd-{}", id_no.0)).unwrap();
        cb.call::<()>(()).unwrap();
        assert_eq!(
            lua.globals().get::<String>("RESULT").unwrap(), "nil",
            "a plugin absent from config_map sees wc.config == nil"
        );

        // Over-cap config → the plugin still loads (its command registers), wc.config is nil,
        // and load_sources warns loudly via the caller-owned `warns` channel.
        let mut reg2 = Registry::builtins();
        let mut host2 = PluginHost::new().unwrap();
        let mut overcap_map: BTreeMap<String, toml::Value> = BTreeMap::new();
        let long = "x".repeat(crate::limits::PLUGIN_MAX_CONFIG_STR + 1);
        overcap_map.insert("overcap".to_string(), toml::Value::String(long));
        let mut warns2 = Vec::new();
        let reports2 = load_sources(
            &mut reg2, &mut host2,
            &sources(&[("overcap", src)]),
            &overcap_map, &mut warns2,
        );
        assert_eq!(reports2[0].result, Ok(1), "plugin still loads despite an over-cap config");
        assert_eq!(warns2.len(), 1, "an over-cap config warns loudly, not silently");
        assert!(warns2[0].contains("overcap"), "{}", warns2[0]);
        let lua2 = host2.lua().unwrap();
        let id_overcap = reg2.resolve_name("overcap.check").unwrap();
        let cb2: mlua::Function =
            lua2.named_registry_value(&format!("wc-cmd-{}", id_overcap.0)).unwrap();
        cb2.call::<()>(()).unwrap();
        assert_eq!(
            lua2.globals().get::<String>("RESULT").unwrap(), "nil",
            "an over-cap config degrades to wc.config == nil, not a load failure"
        );
    }

    /// The KEY-cap mirror of `config_reaches_wc_config`'s over-cap VALUE case (spec §12): a
    /// config table with a KEY past `PLUGIN_MAX_CONFIG_STR` must degrade the same way —
    /// `wc.config == nil`, a loud warning, and the plugin's command still registers.
    #[test]
    fn overcap_config_key_degrades_to_nil_config_command_still_registers() {
        let src = "\
            local cfg = wc.config\n\
            wc.register_command{ name='check', label='Check', fn=function()\n\
                if cfg == nil then RESULT = 'nil' else RESULT = 'present' end\n\
            end }";
        let mut reg = Registry::builtins();
        let mut host = PluginHost::new().unwrap();
        let mut overcap_key_map: BTreeMap<String, toml::Value> = BTreeMap::new();
        let long_key = "k".repeat(crate::limits::PLUGIN_MAX_CONFIG_STR + 1);
        let mut table = toml::map::Map::new();
        table.insert(long_key, toml::Value::Integer(1));
        overcap_key_map.insert("overcapkey".to_string(), toml::Value::Table(table));
        let mut warns = Vec::new();
        let reports = load_sources(
            &mut reg, &mut host,
            &sources(&[("overcapkey", src)]),
            &overcap_key_map, &mut warns,
        );
        assert_eq!(reports[0].result, Ok(1), "plugin still loads despite an over-cap config KEY");
        assert_eq!(warns.len(), 1, "an over-cap config key warns loudly, not silently");
        assert!(warns[0].contains("overcapkey"), "{}", warns[0]);
        let lua = host.lua().unwrap();
        let id = reg.resolve_name("overcapkey.check").unwrap();
        let cb: mlua::Function = lua.named_registry_value(&format!("wc-cmd-{}", id.0)).unwrap();
        cb.call::<()>(()).unwrap();
        assert_eq!(
            lua.globals().get::<String>("RESULT").unwrap(), "nil",
            "an over-cap config key degrades to wc.config == nil, not a load failure"
        );
    }
}
