//! The harper-ls client (Effort A, imperative shell): the `HarperLs` provider handle, the
//! long-lived client thread + `FlushGuard`, child spawn/respawn/shutdown, the pure `HarperState`
//! protocol state machine (incl. the `workspace/configuration` PULL responder), and eager-assembly.
