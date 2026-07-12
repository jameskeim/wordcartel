//! The `wc.*` Lua-facing surface: registration (`wc.register_command`, Task 4) and the editor
//! API (`wc.text`/`wc.insert`/`wc.replace`/`wc.status`/…, Task 5), both honoring the
//! input-validation LAW (`plugin_check_range`) and the resource-bound LAW (borrowed-length-
//! check-then-convert on every plugin-supplied string).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::plugin::host::PendingReg;
use crate::registry::{menu_from_str, CommandId};

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
    let wc: mlua::Table = match lua.globals().get::<Option<mlua::Table>>("wc")? {
        Some(t) => t,
        None => {
            let t = lua.create_table()?;
            lua.globals().set("wc", t.clone())?;
            t
        }
    };
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
