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
use crate::plugin::PluginEventKind;
use crate::registry::menu_from_str;

/// Fetch the shared `wc` global table, creating it on first use. Idempotent across the
/// installers that share it — `install_registration` (load time), `install_editor_api`
/// (callback time, below), and `plugin::settings::install_config` (a sibling module — hence
/// `pub(crate)`, not module-private).
pub(crate) fn wc_table(lua: &mlua::Lua) -> mlua::Result<mlua::Table> {
    match lua.globals().get::<Option<mlua::Table>>("wc")? {
        Some(t) => Ok(t),
        None => {
            let t = lua.create_table()?;
            lua.globals().set("wc", t.clone())?;
            Ok(t)
        }
    }
}

/// Install the `wc` registration surface for ONE plugin's exec pass. `stem` (owned — moved into
/// the 'static closure; an owned String is 'static, so this compiles without the `send` feature,
/// exactly as the former `&'static str` did) fixes the namespace. Every plugin string is
/// cap-checked on its BORROWED `mlua::String` bytes (resource-bound LAW) and pushed RAW into
/// `sink`; interning + the `wc-cmd-<id>` callback write happen ONLY at commit (load_one), so a
/// plugin that fails preflight leaks nothing and overwrites no live callback key (§7b).
///
/// Every plugin-supplied string crosses into Rust via the resource-bound LAW's
/// borrowed-length-check-then-convert pattern (global constraint 1b): extracted as
/// `mlua::String` (borrows the Lua-side bytes, no Rust allocation), its `.as_bytes().len()`
/// checked against the cap FIRST, and only on pass converted/owned. `name`/`label` are checked
/// against their length caps; `menu` is parsed to `MenuCategory` on the borrowed bytes (an
/// unrecognized value is a typed error, never a silent default).
pub(crate) fn install_registration(
    lua: &mlua::Lua,
    stem: String,
    sink: Rc<RefCell<Vec<PendingReg>>>,
    count: Rc<Cell<usize>>,
) -> mlua::Result<()> {
    let wc = wc_table(lua)?;
    let reg_fn = lua.create_function(move |_lua, spec: mlua::Table| {
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
        // arg: the optional argument-prompt string (Task 5) — same borrowed-length-check-then-
        // convert pattern as label, capped against the same PLUGIN_MAX_LABEL_LEN (it too is a
        // short UI-facing prompt string, not a command payload).
        let arg_raw: Option<mlua::String> = spec.get("arg")?;
        if let Some(a) = &arg_raw {
            if a.as_bytes().len() > crate::limits::PLUGIN_MAX_LABEL_LEN {
                return Err(mlua::Error::runtime("plugin: command arg prompt too long"));
            }
        }
        let func: mlua::Function = spec.get("fn")?;
        // menu: parse-to-enum on the borrowed bytes — no owned String needed either way.
        let menu = match &menu_raw {
            None => None,
            Some(m) => Some(
                menu_from_str(m.to_str()?.as_ref())
                    .ok_or_else(|| mlua::Error::runtime("plugin: unknown menu value"))?,
            ),
        };
        // No intern, no set_named_registry_value here — commit does both (§7b). Own raw strings only.
        let name_full = format!("{stem}.{}", name_raw.to_str()?.as_ref());
        let label = label_raw.to_str()?.to_owned();
        let arg = match &arg_raw {
            Some(a) => Some(a.to_str()?.to_owned()),
            None => None,
        };
        count.set(count.get() + 1);
        sink.borrow_mut().push(PendingReg { name_full, label, menu, func, arg });
        Ok(())
    })?;
    wc.set("register_command", reg_fn)?;
    Ok(())
}

/// Install the `wc.on(event, fn)` load-time hook collector for ONE plugin's exec pass — the
/// second registration verb (P2 §3b), sitting beside [`install_registration`] on the same `wc`
/// table and closed by the SAME load→callback-phase boundary ([`install_on_closed`]). `name` is
/// parsed via [`crate::plugin::event_from_str`] (the `menu_from_str` parse-to-enum precedent —
/// an unknown event name is a typed error, nothing stored, nothing interned) and `sink` is
/// capped at [`crate::limits::PLUGIN_MAX_HOOKS_PER_PLUGIN`] BEFORE the push (resource-bound
/// LAW — each hook stores a Lua function in the VM registry plus an owned `HookEntry`).
pub(crate) fn install_on(
    lua: &mlua::Lua,
    sink: Rc<RefCell<Vec<(PluginEventKind, mlua::Function)>>>,
) -> mlua::Result<()> {
    let wc = wc_table(lua)?;
    let on_fn = lua.create_function(move |_lua, (name, func): (mlua::String, mlua::Function)| {
        if sink.borrow().len() >= crate::limits::PLUGIN_MAX_HOOKS_PER_PLUGIN {
            return Err(mlua::Error::runtime("plugin: too many hooks (max 64)"));
        }
        let kind = crate::plugin::event_from_str(name.to_str()?.as_ref())
            .ok_or_else(|| mlua::Error::runtime("plugin: unknown event name"))?;
        sink.borrow_mut().push((kind, func));
        Ok(())
    })?;
    wc.set("on", on_fn)?;
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
/// "editor busy" Lua error instead of a `RefCell` panic (§3d). Also closes registration
/// ([`install_registration_closed`]) — this call site (`PluginHost::attach_bridge`) is the
/// load→callback-phase boundary (every plugin's `load_one` has already run by the time `run()`
/// wraps the editor and attaches the bridge), so it is the one place to flip `wc.register_command`
/// from "collects into this plugin's sink" to "errors, always" (design.md §5).
pub(crate) fn install_editor_api(lua: &mlua::Lua, bridge: &Bridge) -> mlua::Result<()> {
    let wc = wc_table(lua)?;
    install_reads(lua, &wc, bridge)?;
    install_insert(lua, &wc, bridge)?;
    install_replace(lua, &wc, bridge)?;
    install_set_selection(lua, &wc, bridge)?;
    install_status(lua, &wc, bridge)?;
    install_command(lua, &wc, bridge)?;
    install_timer(lua, &wc, bridge)?;
    install_registration_closed(lua, &wc)?;
    install_on_closed(lua, &wc)?;
    install_config_cleared(&wc)?;
    Ok(())
}

/// Clear `wc.config` to nil at the load→callback-phase boundary (mirrors
/// [`install_registration_closed`], the same-shaped guard on the other half of `wc`): the
/// last-loaded plugin's config table must not linger on the shared `wc` global for callbacks —
/// a plugin that wants its config at callback time must capture it in a Lua local at load.
fn install_config_cleared(wc: &mlua::Table) -> mlua::Result<()> {
    wc.set("config", mlua::Value::Nil)
}

/// Overwrite `wc.register_command` with a stub that always raises a typed Lua error — the
/// spec's "registration functions are callable only during load … calling a registration
/// function outside load … raises a Lua error (degrade, not panic)" rule (design.md §5,
/// :326-329). Without this, the last plugin loaded leaves its real `wc.register_command`
/// closure live on the shared `wc` table forever (`wc_table` is idempotent across plugins), so a
/// post-load callback calling it would silently push into a sink no one ever drains again — a
/// silent no-op, not the specified error. Mirrors the INVERSE of `wc.status`'s load-time
/// unavailability (nil global → "attempt to call a nil value", since `install_editor_api` hasn't
/// run yet during load): here the registration half of `wc` is the one taken away, on the other
/// side of the same boundary.
fn install_registration_closed(lua: &mlua::Lua, wc: &mlua::Table) -> mlua::Result<()> {
    let stub = lua.create_function(|_, _args: mlua::MultiValue| -> mlua::Result<()> {
        Err(mlua::Error::runtime("wc.register_command is only available during plugin load"))
    })?;
    wc.set("register_command", stub)?;
    Ok(())
}

/// Overwrite `wc.on` with the same always-erroring stub shape as
/// [`install_registration_closed`] — the second registration verb closed by the same
/// load→callback-phase boundary.
fn install_on_closed(lua: &mlua::Lua, wc: &mlua::Table) -> mlua::Result<()> {
    let stub = lua.create_function(|_, _args: mlua::MultiValue| -> mlua::Result<()> {
        Err(mlua::Error::runtime("wc.on is only available during plugin load"))
    })?;
    wc.set("on", stub)?;
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
    let invoke_state = bridge.invoke_state.clone();
    wc.set(
        "insert",
        lua.create_function(move |_, text: mlua::String| {
            if invoke_state.borrow().observer {
                return Err(mlua::Error::runtime("plugin: editing is not allowed from an event hook"));
            }
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
    let invoke_state = bridge.invoke_state.clone();
    wc.set(
        "replace",
        lua.create_function(move |_, (a, b, text): (usize, usize, mlua::String)| {
            if invoke_state.borrow().observer {
                return Err(mlua::Error::runtime("plugin: editing is not allowed from an event hook"));
            }
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
    let invoke_state = bridge.invoke_state.clone();
    wc.set(
        "set_selection",
        lua.create_function(move |_, (anchor, head): (usize, usize)| {
            if invoke_state.borrow().observer {
                return Err(mlua::Error::runtime("plugin: editing is not allowed from an event hook"));
            }
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

/// `wc.command(name)` (§5a) — enqueue a fire-and-forget dispatch, resolved at pump drain
/// against the LIVE `&Registry` (never a call-time name-set snapshot — contract law 1: the
/// registry is the single source of truth). Checks, in order: observer (blocked from a hook —
/// mutation-by-proxy, and `on_save`→`save` would self-cascade), the borrowed-name length cap,
/// then the queue cap — each BEFORE any allocation past what mlua already borrowed.
fn install_command(lua: &mlua::Lua, wc: &mlua::Table, bridge: &Bridge) -> mlua::Result<()> {
    let editor = bridge.editor.clone();
    let invoke = bridge.invoke_state.clone();
    wc.set(
        "command",
        lua.create_function(move |_, (name, arg): (mlua::String, Option<mlua::String>)| {
            let st = invoke.borrow();
            if st.observer {
                return Err(mlua::Error::runtime("plugin: wc.command is not allowed from an event hook"));
            }
            if name.as_bytes().len() > crate::limits::PLUGIN_MAX_COMMAND_REF {
                return Err(mlua::Error::runtime("plugin: command name too long"));
            }
            // arg: cap on the BORROWED bytes before any Rust allocation (resource-bound LAW).
            let arg = match &arg {
                Some(a) => {
                    if a.as_bytes().len() > crate::limits::PLUGIN_MAX_COMMAND_ARG {
                        return Err(mlua::Error::runtime("plugin: command arg too long"));
                    }
                    Some(a.to_str()?.to_owned())
                }
                None => None,
            };
            let origin = st.current.clone().unwrap_or_default();
            drop(st);
            let mut e = editor.try_borrow_mut().map_err(|_| mlua::Error::runtime("plugin: editor busy"))?;
            if e.pending_plugin_dispatch.len() >= crate::limits::PLUGIN_MAX_PENDING_DISPATCH {
                return Err(mlua::Error::runtime("plugin: command queue full"));
            }
            e.pending_plugin_dispatch.push_back(crate::plugin::PluginDispatch {
                origin,
                name: name.to_str()?.to_owned(),
                arg,
            });
            Ok(())
        })?,
    )?;
    Ok(())
}

/// `wc.timer(interval_ms, fn [, repeat_bool])` (arm) + `wc.timer_cancel(handle)` (disarm) — the
/// plugin-facing timer API (P3 §3b). Positional `(interval, fn, repeat?)` mirrors `wc.replace`'s
/// `(a, b, text)` tuple-extraction idiom (a mechanical refinement of the spec's `{table}` sketch —
/// the guardrails are identical). Both verbs are observer-checked, so a hook OR a timer callback
/// (both run observer-tier) can neither arm nor cancel — closing the timer-spawns-timers spin
/// vector at the arm gate, not by trust. Arm enforces, in order: observer, the interval floor
/// ([`crate::limits::PLUGIN_TIMER_MIN_INTERVAL_MS`]), then the per-plugin count cap
/// ([`crate::limits::PLUGIN_MAX_TIMERS_PER_PLUGIN`]) — each BEFORE the callback is persisted or the
/// `PluginTimer` pushed. `handle` is a monotonic per-editor counter (never reused); the callback
/// lives in the VM named registry under `wc-timer-<handle>` (dies with the VM at reload).
///
/// Both the cap and `wc.timer_cancel` attribute a timer to its **plugin STEM** — the part of
/// `InvokeState::current` before the first `.` — never the full command id. A plugin command id is
/// always `"<stem>.<name>"`, so two commands of the SAME plugin share one stem; using the full id
/// as `origin` (the pre-fix shape) let a K-command plugin arm `8×K` timers (one private 8-budget
/// per command id) and let ANY command of ANY plugin cancel ANY other plugin's timer by handle (no
/// scoping at all). Deriving `origin` from the stem closes both at the same root cause.
fn install_timer(lua: &mlua::Lua, wc: &mlua::Table, bridge: &Bridge) -> mlua::Result<()> {
    let editor = bridge.editor.clone();
    let clock = bridge.clock.clone();
    let invoke = bridge.invoke_state.clone();
    // wc.timer(interval_ms, fn [, repeat_bool]) — one-shot unless repeat is true.
    wc.set("timer", lua.create_function(
        move |lua, (interval_ms, func, repeat): (u64, mlua::Function, Option<bool>)| {
            if invoke.borrow().observer {
                return Err(mlua::Error::runtime(
                    "plugin: wc.timer is not allowed from an event hook or a timer callback"));
            }
            if interval_ms < crate::limits::PLUGIN_TIMER_MIN_INTERVAL_MS {
                return Err(mlua::Error::runtime("plugin: timer interval below the 1000 ms floor"));
            }
            let current = invoke.borrow().current.clone().unwrap_or_default();
            let origin = plugin_stem(&current);
            let mut e = editor.try_borrow_mut()
                .map_err(|_| mlua::Error::runtime("plugin: editor busy"))?;
            if e.pending_plugin_timers.iter().filter(|t| t.origin == origin).count()
                >= crate::limits::PLUGIN_MAX_TIMERS_PER_PLUGIN {
                return Err(mlua::Error::runtime("plugin: timer limit reached (max 8)"));
            }
            e.next_timer_handle += 1;
            let handle = e.next_timer_handle;
            let key = format!("wc-timer-{handle}");
            lua.set_named_registry_value(&key, func)?;   // persist the callback (dies with the VM)
            let now = clock.now_ms();
            e.pending_plugin_timers.push(crate::plugin::PluginTimer {
                handle, origin, key, next_due_ms: now.saturating_add(interval_ms),
                interval_ms, repeat: repeat.unwrap_or(false), pending: false,
            });
            Ok(handle as i64)   // Lua integer (i64); the monotonic counter never reaches 2^63
        })?)?;
    // wc.timer_cancel(handle) — remove + free the registry key IFF the caller's plugin stem owns
    // it; unknown handle OR a handle owned by a DIFFERENT plugin → silent no-op (a plugin has no
    // way to observe another plugin's handles, so the two are indistinguishable on the Lua side —
    // same "unknown handle" degrade the spec already documents).
    let editor = bridge.editor.clone();
    let invoke = bridge.invoke_state.clone();
    wc.set("timer_cancel", lua.create_function(move |lua, handle: i64| {
        if invoke.borrow().observer {
            return Err(mlua::Error::runtime(
                "plugin: wc.timer_cancel is not allowed from an event hook or a timer callback"));
        }
        let current = invoke.borrow().current.clone().unwrap_or_default();
        let origin = plugin_stem(&current);
        let handle = handle as u64;
        let mut e = editor.try_borrow_mut()
            .map_err(|_| mlua::Error::runtime("plugin: editor busy"))?;
        if let Some(pos) = e.pending_plugin_timers.iter()
            .position(|t| t.handle == handle && t.origin == origin) {
            let key = e.pending_plugin_timers.remove(pos).key;
            lua.set_named_registry_value(&key, mlua::Value::Nil)?;   // free the callback
        }
        Ok(())
    })?)?;
    Ok(())
}

/// The plugin STEM of an `InvokeState::current` label — the part before the first `.` (a plugin
/// command id is always `"<stem>.<name>"`, per [`install_registration`]'s `name_full`
/// construction). Shared by the timer arm cap and `wc.timer_cancel`'s ownership check (both docs
/// above) so a plugin's timer budget/ownership is keyed by PLUGIN, never by command id.
fn plugin_stem(current: &str) -> String {
    current.split('.').next().unwrap_or(current).to_string()
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
