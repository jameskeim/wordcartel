//! Pure/IO-light LSP plumbing (Effort A): Content-Length framing, JSON-RPC envelopes over
//! `serde_json::Value`, opaque document URIs, UTF-16→byte position conversion, and
//! codeAction `TextEdit`→`Suggestion` mapping. No process IO lives here — see harper_ls.rs.
