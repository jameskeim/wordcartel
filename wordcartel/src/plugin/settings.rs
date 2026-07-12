//! `[plugins.config.<name>]` TOML → an opaque `mlua::Value` for `wc.config`, bounded by the
//! resource-bound LAW's config caps (depth/nodes/byte — checked BEFORE the Lua allocation).

use crate::limits::{PLUGIN_MAX_CONFIG_DEPTH, PLUGIN_MAX_CONFIG_NODES, PLUGIN_MAX_CONFIG_STR};

/// Convert one plugin's config value to a Lua value under the three caps. `Err(reason)` on any
/// cap; the caller then hands the plugin `wc.config = nil` + a warning (plugin still loads).
pub(crate) fn config_to_lua(lua: &mlua::Lua, v: &toml::Value) -> Result<mlua::Value, String> {
    let mut nodes = 0usize;
    convert(lua, v, 1, &mut nodes)
}

fn convert(lua: &mlua::Lua, v: &toml::Value, depth: usize, nodes: &mut usize) -> Result<mlua::Value, String> {
    if depth > PLUGIN_MAX_CONFIG_DEPTH {
        return Err(format!("config nesting deeper than {PLUGIN_MAX_CONFIG_DEPTH}"));
    }
    *nodes += 1;
    if *nodes > PLUGIN_MAX_CONFIG_NODES {
        return Err(format!("config exceeds {PLUGIN_MAX_CONFIG_NODES} nodes"));
    }
    Ok(match v {
        toml::Value::String(s) => {
            if s.len() > PLUGIN_MAX_CONFIG_STR {
                return Err(format!("config string exceeds {PLUGIN_MAX_CONFIG_STR} bytes"));
            }
            mlua::Value::String(lua.create_string(s).map_err(|e| e.to_string())?) // alloc AFTER the cap
        }
        toml::Value::Integer(i) => mlua::Value::Integer(*i),
        toml::Value::Float(f) => mlua::Value::Number(*f),
        toml::Value::Boolean(b) => mlua::Value::Boolean(*b),
        toml::Value::Datetime(d) => {
            let s = d.to_string();
            if s.len() > PLUGIN_MAX_CONFIG_STR {
                return Err("config datetime string too long".into());
            }
            mlua::Value::String(lua.create_string(&s).map_err(|e| e.to_string())?)
        }
        toml::Value::Array(a) => {
            let t = lua.create_table().map_err(|e| e.to_string())?;
            for (i, item) in a.iter().enumerate() {
                let lv = convert(lua, item, depth + 1, nodes)?;
                t.set(i + 1, lv).map_err(|e| e.to_string())?; // Lua 1-based sequence
            }
            mlua::Value::Table(t)
        }
        toml::Value::Table(map) => {
            let t = lua.create_table().map_err(|e| e.to_string())?;
            for (k, val) in map {
                if k.len() > PLUGIN_MAX_CONFIG_STR {
                    return Err(format!("config key exceeds {PLUGIN_MAX_CONFIG_STR} bytes"));
                }
                *nodes += 1;
                if *nodes > PLUGIN_MAX_CONFIG_NODES {
                    return Err(format!("config exceeds {PLUGIN_MAX_CONFIG_NODES} nodes"));
                }
                let lv = convert(lua, val, depth + 1, nodes)?;
                t.set(k.as_str(), lv).map_err(|e| e.to_string())?;
            }
            mlua::Value::Table(t)
        }
    })
}

/// Install `wc.config` for ONE plugin's exec pass (its converted table, or nil). Cleared at
/// attach_bridge (install_editor_api) so it does not linger on the shared `wc` global.
pub(crate) fn install_config(lua: &mlua::Lua, value: mlua::Value) -> mlua::Result<()> {
    crate::plugin::api::wc_table(lua)?.set("config", value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_to_lua_scalars_and_nesting() {
        let lua = mlua::Lua::new();
        let toml_str = r#"
            name = "wordcount"
            min_words = 100
            ratio = 1.5
            enabled = true
            tags = ["a", "b"]
            [nested]
            depth = 2
        "#;
        let v: toml::Value = toml::from_str(toml_str).unwrap();
        let lv = config_to_lua(&lua, &v).expect("under caps");
        let t = match lv {
            mlua::Value::Table(t) => t,
            other => panic!("expected a table, got {other:?}"),
        };
        assert_eq!(t.get::<String>("name").unwrap(), "wordcount");
        assert_eq!(t.get::<i64>("min_words").unwrap(), 100);
        assert_eq!(t.get::<f64>("ratio").unwrap(), 1.5);
        assert!(t.get::<bool>("enabled").unwrap());
        let tags: mlua::Table = t.get("tags").unwrap();
        assert_eq!(tags.get::<String>(1).unwrap(), "a");
        assert_eq!(tags.get::<String>(2).unwrap(), "b");
        let nested: mlua::Table = t.get("nested").unwrap();
        assert_eq!(nested.get::<i64>("depth").unwrap(), 2);
    }

    #[test]
    fn config_to_lua_rejects_over_byte_cap() {
        let lua = mlua::Lua::new();
        let long = "x".repeat(crate::limits::PLUGIN_MAX_CONFIG_STR + 1);
        let v = toml::Value::String(long);
        let err = config_to_lua(&lua, &v).expect_err("over-cap string value must be rejected");
        assert!(err.contains("bytes"), "{err}");

        let mut map = toml::map::Map::new();
        let long_key = "k".repeat(crate::limits::PLUGIN_MAX_CONFIG_STR + 1);
        map.insert(long_key, toml::Value::Integer(1));
        let v = toml::Value::Table(map);
        let err = config_to_lua(&lua, &v).expect_err("over-cap key must be rejected");
        assert!(err.contains("key exceeds"), "{err}");
    }

    #[test]
    fn config_to_lua_rejects_over_depth_and_over_nodes() {
        let lua = mlua::Lua::new();
        // Build a chain nested deeper than PLUGIN_MAX_CONFIG_DEPTH.
        let mut v = toml::Value::Integer(0);
        for _ in 0..(crate::limits::PLUGIN_MAX_CONFIG_DEPTH + 2) {
            let mut map = toml::map::Map::new();
            map.insert("n".to_string(), v);
            v = toml::Value::Table(map);
        }
        let err = config_to_lua(&lua, &v).expect_err("over-depth config must be rejected");
        assert!(err.contains("nesting"), "{err}");

        // Build a flat table with more entries than PLUGIN_MAX_CONFIG_NODES allows (each entry
        // costs 2 nodes: key + value).
        let mut map = toml::map::Map::new();
        for i in 0..(crate::limits::PLUGIN_MAX_CONFIG_NODES) {
            map.insert(format!("k{i}"), toml::Value::Integer(i as i64));
        }
        let v = toml::Value::Table(map);
        let err = config_to_lua(&lua, &v).expect_err("over-node config must be rejected");
        assert!(err.contains("nodes"), "{err}");
    }
}
