//! The `wc.*` Lua-facing surface: registration (`wc.register_command`, Task 4) and the editor
//! API (`wc.text`/`wc.insert`/`wc.replace`/`wc.status`/…, Task 5), both honoring the
//! input-validation LAW (`plugin_check_range`) and the resource-bound LAW (borrowed-length-
//! check-then-convert on every plugin-supplied string).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use wordcartel_core::buffer::TextBuffer;
use wordcartel_core::change::{ChangeSet, EditError, Op};
use wordcartel_core::history::Transaction;
use wordcartel_core::selection::Selection;

use crate::plugin::host::{Bridge, PendingReg};
use crate::registry::{menu_from_str, CommandId};

/// Fetch the shared `wc` global table, creating it on first use. Idempotent across the two
/// installers that share it — `install_registration` (load time) and `install_editor_api`
/// (callback time, below).
fn wc_table(lua: &mlua::Lua) -> mlua::Result<mlua::Table> {
    match lua.globals().get::<Option<mlua::Table>>("wc")? {
        Some(t) => Ok(t),
        None => {
            let t = lua.create_table()?;
            lua.globals().set("wc", t.clone())?;
            Ok(t)
        }
    }
}

/// Install the `wc` registration surface for ONE plugin's exec pass. `stem` (already interned)
/// fixes the namespace; `sink` collects [`PendingReg`] (drained into the `Registry` after exec —
/// the loader's atomic-per-plugin commit); `count` enforces the per-plugin command cap
/// (`limits::PLUGIN_MAX_COMMANDS_PER_PLUGIN`).
///
/// Every plugin-supplied string crosses into Rust via the resource-bound LAW's
/// borrowed-length-check-then-convert pattern (global constraint 1b): extracted as
/// `mlua::String` (borrows the Lua-side bytes, no Rust allocation), its `.as_bytes().len()`
/// checked against the cap FIRST, and only on pass converted/owned/interned. `name`/`label` are
/// checked against their length caps; `menu` is parsed to `MenuCategory` on the borrowed bytes
/// (an unrecognized value is a typed error, never a silent default). `intern` (a permanent leak)
/// is reached only after every check on this call has passed.
pub(crate) fn install_registration(
    lua: &mlua::Lua,
    stem: &'static str,
    sink: Rc<RefCell<Vec<PendingReg>>>,
    count: Rc<Cell<usize>>,
) -> mlua::Result<()> {
    let wc = wc_table(lua)?;
    let reg_fn = lua.create_function(move |lua, spec: mlua::Table| {
        if count.get() >= crate::limits::PLUGIN_MAX_COMMANDS_PER_PLUGIN {
            return Err(mlua::Error::runtime("plugin: too many commands (max 256)"));
        }
        // name: borrowed mlua::String, cap on the borrowed byte length BEFORE any Rust alloc.
        let name_raw: mlua::String = spec.get("name")?;
        if name_raw.as_bytes().len() > crate::limits::PLUGIN_MAX_NAME_LEN {
            return Err(mlua::Error::runtime("plugin: command name too long"));
        }
        // label: same borrowed-length-check-then-convert pattern.
        let label_raw: mlua::String = spec.get("label")?;
        if label_raw.as_bytes().len() > crate::limits::PLUGIN_MAX_LABEL_LEN {
            return Err(mlua::Error::runtime("plugin: label too long"));
        }
        let menu_raw: Option<mlua::String> = spec.get("menu")?;
        let func: mlua::Function = spec.get("fn")?;
        // menu: parse-to-enum on the borrowed bytes — no owned String needed either way.
        let menu = match &menu_raw {
            None => None,
            Some(m) => Some(
                menu_from_str(m.to_str()?.as_ref())
                    .ok_or_else(|| mlua::Error::runtime("plugin: unknown menu value"))?,
            ),
        };
        // Every cap passed — own + intern (the ONLY Rust allocs, all AFTER the checks above).
        let full = format!("{stem}.{}", name_raw.to_str()?.as_ref());
        let id = CommandId(crate::plugin::intern(&full));
        let label_s = crate::plugin::intern(label_raw.to_str()?.as_ref());
        // Persistent callback storage (WezTerm-style): keyed per command id in the named
        // registry so the pump (Task 5) can look it up by `CommandId` alone.
        lua.set_named_registry_value(&format!("wc-cmd-{}", id.0), func)?;
        count.set(count.get() + 1);
        sink.borrow_mut().push(PendingReg { id, label: label_s, menu });
        Ok(())
    })?;
    wc.set("register_command", reg_fn)?;
    Ok(())
}

/// Why a plugin-supplied offset/range was rejected by [`plugin_check_range`] — the
/// input-validation LAW's one chokepoint (spec §3). Converts to a typed Lua error via
/// `From<PluginRangeError> for mlua::Error`, so `plugin_check_range(..)?` composes directly in
/// any `wc.*` closure returning `mlua::Result<_>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PluginRangeError {
    /// `from > to`.
    Reversed { from: usize, to: usize },
    /// `to > buf.len()`.
    OutOfBounds { to: usize, len: usize },
    /// `pos` is not on a char boundary (`buf.clamp_to_boundary(pos) != pos`).
    NotBoundary { pos: usize },
}

impl std::fmt::Display for PluginRangeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PluginRangeError::Reversed { from, to } =>
                write!(f, "range reversed (from {from} > to {to})"),
            PluginRangeError::OutOfBounds { to, len } =>
                write!(f, "range out of bounds (to {to} > buffer length {len})"),
            PluginRangeError::NotBoundary { pos } =>
                write!(f, "offset {pos} is not a char boundary"),
        }
    }
}

impl From<PluginRangeError> for mlua::Error {
    fn from(e: PluginRangeError) -> Self {
        mlua::Error::runtime(format!("plugin: {e}"))
    }
}

/// The input-validation LAW chokepoint (spec §3). Shared by `wc.text` (read) and `wc.replace`
/// (edit) — the ONLY two `wc.*` APIs that take a caller-supplied range; `wc.insert` calls this
/// too, with `from == to == cursor`. Enforces, in order: ordered (`from <= to`), in-bounds
/// (`to <= buf.len()`), both endpoints on a char boundary. No raw plugin-supplied offset may
/// reach an asserting core primitive (`ChangeSet::from_ops`/`apply`/`insert`, `TextBuffer::slice`)
/// without passing this check first.
pub(crate) fn plugin_check_range(buf: &TextBuffer, from: usize, to: usize) -> Result<(), PluginRangeError> {
    if from > to {
        return Err(PluginRangeError::Reversed { from, to });
    }
    if to > buf.len() {
        return Err(PluginRangeError::OutOfBounds { to, len: buf.len() });
    }
    if buf.clamp_to_boundary(from) != from {
        return Err(PluginRangeError::NotBoundary { pos: from });
    }
    if buf.clamp_to_boundary(to) != to {
        return Err(PluginRangeError::NotBoundary { pos: to });
    }
    Ok(())
}

/// Render a core `EditError` (raised only if a plugin edit races a concurrent buffer change —
/// `submit_transaction`'s staleness re-check, orthogonal to `plugin_check_range`'s construction
/// guard) as a typed Lua error message.
fn edit_error_message(e: EditError) -> String {
    match e {
        EditError::StaleLength { expected, actual } => format!(
            "plugin: buffer changed underneath the edit (expected length {expected}, actual {actual})"
        ),
        EditError::OpBoundary { pos } => format!("plugin: edit boundary {pos} is not a char boundary"),
    }
}

/// Install the `wc.*` editor API (callback time — reads, edits, status) into `bridge`'s VM.
/// Every closure captures a clone of `bridge.editor` (`Rc<RefCell<Editor>>`) and, for edits, a
/// clone of `bridge.clock` (`Rc<dyn Clock>`); every borrow is a short `try_borrow`/
/// `try_borrow_mut` — never `borrow`/`borrow_mut` — so a genuine nested re-entry degrades to an
/// "editor busy" Lua error instead of a `RefCell` panic (§3d).
pub(crate) fn install_editor_api(lua: &mlua::Lua, bridge: &Bridge) -> mlua::Result<()> {
    let wc = wc_table(lua)?;
    install_reads(lua, &wc, bridge)?;
    install_insert(lua, &wc, bridge)?;
    install_replace(lua, &wc, bridge)?;
    install_set_selection(lua, &wc, bridge)?;
    install_status(lua, &wc, bridge)?;
    Ok(())
}

/// `wc.text(a?, b?)`, `wc.selection()`, `wc.cursor()`, `wc.len()`, `wc.version()`, `wc.path()` —
/// live, synchronous, per-call reads (§3a). `wc.text`'s range is pre-validated via
/// `plugin_check_range` (the one chokepoint, shared with `wc.replace`); the rest take no offset
/// input, so there is nothing to validate.
fn install_reads(lua: &mlua::Lua, wc: &mlua::Table, bridge: &Bridge) -> mlua::Result<()> {
    let editor = bridge.editor.clone();
    wc.set(
        "text",
        lua.create_function(move |_, (a, b): (Option<usize>, Option<usize>)| {
            let e = editor.try_borrow().map_err(|_| mlua::Error::runtime("plugin: editor busy"))?;
            let buf = &e.active().document.buffer;
            let from = a.unwrap_or(0);
            let to = b.unwrap_or_else(|| buf.len());
            plugin_check_range(buf, from, to)?;
            Ok(buf.slice(from..to))
        })?,
    )?;

    let editor = bridge.editor.clone();
    wc.set(
        "selection",
        lua.create_function(move |lua, ()| {
            let e = editor.try_borrow().map_err(|_| mlua::Error::runtime("plugin: editor busy"))?;
            let r = e.active().document.selection.primary();
            let t = lua.create_table()?;
            t.set("anchor", r.anchor)?;
            t.set("head", r.head)?;
            Ok(t)
        })?,
    )?;

    let editor = bridge.editor.clone();
    wc.set(
        "cursor",
        lua.create_function(move |_, ()| {
            let e = editor.try_borrow().map_err(|_| mlua::Error::runtime("plugin: editor busy"))?;
            Ok(e.active().document.selection.primary().head)
        })?,
    )?;

    let editor = bridge.editor.clone();
    wc.set(
        "len",
        lua.create_function(move |_, ()| {
            let e = editor.try_borrow().map_err(|_| mlua::Error::runtime("plugin: editor busy"))?;
            Ok(e.active().document.buffer.len())
        })?,
    )?;

    let editor = bridge.editor.clone();
    wc.set(
        "version",
        lua.create_function(move |_, ()| {
            let e = editor.try_borrow().map_err(|_| mlua::Error::runtime("plugin: editor busy"))?;
            Ok(e.active().document.version)
        })?,
    )?;

    let editor = bridge.editor.clone();
    wc.set(
        "path",
        lua.create_function(move |_, ()| {
            let e = editor.try_borrow().map_err(|_| mlua::Error::runtime("plugin: editor busy"))?;
            Ok(e.active().document.path.as_ref().map(|p| p.to_string_lossy().into_owned()))
        })?,
    )?;

    Ok(())
}

/// `wc.insert(text)` — inserts `text` at the live primary cursor (§3b). `text` is extracted as
/// `mlua::String` and length-checked against `PASTE_MAX_BYTES` on the BORROWED bytes (resource
/// LAW) BEFORE any `Tendril` allocation; the cursor offset is pre-validated via
/// `plugin_check_range` (input-validation LAW) before `ChangeSet::insert` ever sees it. Submitted
/// via `submit_transaction` — the untrusted-edit boundary — so a concurrent-edit race
/// (`StaleLength`) degrades to a typed Lua error with zero mutation.
fn install_insert(lua: &mlua::Lua, wc: &mlua::Table, bridge: &Bridge) -> mlua::Result<()> {
    let editor = bridge.editor.clone();
    let clock = bridge.clock.clone();
    wc.set(
        "insert",
        lua.create_function(move |_, text: mlua::String| {
            if text.as_bytes().len() > crate::limits::PASTE_MAX_BYTES {
                return Err(mlua::Error::runtime("plugin: insert text too large"));
            }
            let mut e = editor.try_borrow_mut().map_err(|_| mlua::Error::runtime("plugin: editor busy"))?;
            let cur = e.active().document.selection.primary().head;
            let len = e.active().document.buffer.len();
            plugin_check_range(&e.active().document.buffer, cur, cur)?;
            let cs = ChangeSet::insert(cur, text.to_str()?.as_ref(), len);
            crate::transact::submit_transaction(&mut e, Transaction::new(cs), &*clock)
                .map_err(|err| mlua::Error::runtime(edit_error_message(err)))
        })?,
    )?;
    Ok(())
}

/// `wc.replace(a, b, text)` — pre-validates `(a, b)` via `plugin_check_range` (bad input → typed
/// error, nothing constructed), then builds the `ChangeSet` via the existing public
/// `build_range_replace` (already used by the filter merge — no logic duplication). `text` is
/// length-checked against `PASTE_MAX_BYTES` on the borrowed bytes, same as `wc.insert`.
fn install_replace(lua: &mlua::Lua, wc: &mlua::Table, bridge: &Bridge) -> mlua::Result<()> {
    let editor = bridge.editor.clone();
    let clock = bridge.clock.clone();
    wc.set(
        "replace",
        lua.create_function(move |_, (a, b, text): (usize, usize, mlua::String)| {
            if text.as_bytes().len() > crate::limits::PASTE_MAX_BYTES {
                return Err(mlua::Error::runtime("plugin: replace text too large"));
            }
            let mut e = editor.try_borrow_mut().map_err(|_| mlua::Error::runtime("plugin: editor busy"))?;
            let doc_len = e.active().document.buffer.len();
            plugin_check_range(&e.active().document.buffer, a, b)?;
            let (cs, _edit) = crate::commands::build_range_replace(a, b, text.to_str()?.as_ref(), doc_len);
            crate::transact::submit_transaction(&mut e, Transaction::new(cs), &*clock)
                .map_err(|err| mlua::Error::runtime(edit_error_message(err)))
        })?,
    )?;
    Ok(())
}

/// `wc.set_selection(anchor, head)` — an identity `ChangeSet` (no text edit) carrying the new
/// selection through `submit_transaction`, which snaps out-of-bounds/non-boundary offsets rather
/// than rejecting (the existing selection behavior) — so, uniquely among the edit APIs, this one
/// needs no `plugin_check_range` pre-check.
fn install_set_selection(lua: &mlua::Lua, wc: &mlua::Table, bridge: &Bridge) -> mlua::Result<()> {
    let editor = bridge.editor.clone();
    let clock = bridge.clock.clone();
    wc.set(
        "set_selection",
        lua.create_function(move |_, (anchor, head): (usize, usize)| {
            let mut e = editor.try_borrow_mut().map_err(|_| mlua::Error::runtime("plugin: editor busy"))?;
            let doc_len = e.active().document.buffer.len();
            // Retain(doc_len) sums to len_before, so from_ops's consumption assert holds — this
            // is a validated identity over the LIVE length, never a raw/unchecked from_ops.
            let ident = ChangeSet::from_ops(vec![Op::Retain(doc_len)], doc_len);
            let txn = Transaction::new(ident).with_selection(Selection::range(anchor, head));
            crate::transact::submit_transaction(&mut e, txn, &*clock)
                .map_err(|err| mlua::Error::runtime(edit_error_message(err)))
        })?,
    )?;
    Ok(())
}

/// `wc.status(msg)` — the only user-visible plugin output channel (no console; the app owns the
/// alternate screen). `msg` is truncated on the BORROWED Lua bytes to `PLUGIN_MAX_STATUS_LEN`
/// (resource LAW — never allocate the full oversized string first) before it is owned into
/// `editor.status`.
fn install_status(lua: &mlua::Lua, wc: &mlua::Table, bridge: &Bridge) -> mlua::Result<()> {
    let editor = bridge.editor.clone();
    wc.set(
        "status",
        lua.create_function(move |_, msg: mlua::String| {
            let mut e = editor.try_borrow_mut().map_err(|_| mlua::Error::runtime("plugin: editor busy"))?;
            e.status = crate::plugin::cap_status(&msg.as_bytes(), crate::limits::PLUGIN_MAX_STATUS_LEN);
            Ok(())
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Editor;
    use crate::test_support::TestClock;
    use wordcartel_core::history::Transaction;

    // -----------------------------------------------------------------------
    // spec §8's concurrent-`StaleLength` edit case
    // -----------------------------------------------------------------------

    /// `submit_transaction` is the ONE boundary every `wc.replace`/`wc.insert`/
    /// `wc.set_selection` routes through (§3b) — this proves ITS staleness re-check (the
    /// safety net, independent of `plugin_check_range`'s construction-time guard, which only
    /// sees the length at the moment a `wc.*` closure builds its `ChangeSet`) degrades a
    /// racing edit to a typed error with ZERO mutation — and that [`edit_error_message`], the
    /// ONLY place a core `EditError` becomes a plugin-facing Lua message, names it as a
    /// concurrent-edit race rather than a raw internal error.
    #[test]
    fn stale_length_from_a_racing_edit_formats_the_plugin_facing_error() {
        let mut e = Editor::new_from_text("hello world", None, (40, 10)); // len 11
        // A ChangeSet valid AT CONSTRUCTION TIME for length 11 (an identity retain).
        let cs = ChangeSet::from_ops(vec![Op::Retain(11)], 11);
        // A racing edit lands between construction and submission — the live buffer is now a
        // DIFFERENT length, simulating another actor's concurrent mutation underneath the plan
        // a wc.* closure would have made (each closure re-reads its own length fresh and
        // synchronously, so this race is not reachable through the closures themselves — it
        // exercises the shared boundary's defense directly, the safety net plugin edits inherit).
        e.active_mut().document.buffer = wordcartel_core::buffer::TextBuffer::from_str("short");
        let result = crate::transact::submit_transaction(&mut e, Transaction::new(cs), &TestClock::new(0));
        let err = result.expect_err("a stale claimed_len must be rejected, not silently applied");
        assert!(matches!(err, EditError::StaleLength { expected: 11, actual: 5 }), "{err:?}");
        assert_eq!(e.active().document.buffer.to_string(), "short", "zero mutation on a stale-length race");
        let msg = edit_error_message(err);
        assert!(msg.contains("buffer changed underneath"), "message: {msg}");
    }
}
