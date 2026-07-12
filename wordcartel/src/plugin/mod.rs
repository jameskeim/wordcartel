//! In-process Lua plugin commands (Effort P1). A single `mlua` VM per app instance, hosted by
//! [`host::PluginHost`], registers commands into the existing [`crate::registry::Registry`]
//! (`host`/`api`) and is populated by the filesystem/config loader (`load`). Task 2 wires the
//! dependency and an inert skeleton only — nothing calls into this module yet, so the app
//! behaves identically with no plugins loaded.
pub mod host;
pub mod api;
pub mod load;
