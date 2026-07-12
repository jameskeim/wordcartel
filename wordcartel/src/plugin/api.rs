//! The `wc.*` Lua-facing surface: registration (`wc.register_command`, Task 4) and the editor
//! API (`wc.text`/`wc.insert`/`wc.replace`/`wc.status`/…, Task 5), both honoring the
//! input-validation LAW (`plugin_check_range`) and the resource-bound LAW (borrowed-length-check-
//! then-convert on every plugin-supplied string). Task 2 is a stub — no `wc` table exists yet.
