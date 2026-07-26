//! Pure/IO-light LSP plumbing (Effort A): Content-Length framing, JSON-RPC envelopes over
//! `serde_json::Value`, opaque document URIs, UTF-16→byte position conversion, and
//! codeAction `TextEdit`→`Suggestion` mapping. No process IO lives here — see harper_ls.rs.

use crate::editor::BufferId;
use crate::limits::LSP_MAX_FRAME_BYTES;
use std::io::{self, BufRead, Write};
use wordcartel_core::diagnostics::Suggestion;

/// The opaque, generation-tagged wire URI for a document. Identical form for saved and unsaved
/// buffers — harper lints the sent text + `languageId`, not the file at any path, and the
/// embedded generation is the load-bearing stale-publish discriminator (spec §3.3, §5).
pub fn doc_uri(buffer_id: BufferId, generation: u64) -> String {
    format!("untitled:wcartel-{}-{}", buffer_id.0, generation)
}

/// Serialize a JSON-RPC message and write it Content-Length-framed to `w`.
pub fn write_frame<W: Write>(w: &mut W, msg: &serde_json::Value) -> io::Result<()> {
    let body = serde_json::to_vec(msg).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    write!(w, "Content-Length: {}\r\n\r\n", body.len())?;
    w.write_all(&body)?;
    w.flush()
}

/// Read one Content-Length-framed JSON-RPC message. `Ok(None)` on clean EOF before any header;
/// `Err` on a malformed frame or a mid-frame EOF (the caller treats either as stream corruption).
pub fn read_frame<R: BufRead>(r: &mut R) -> io::Result<Option<serde_json::Value>> {
    let mut content_length: Option<usize> = None;
    let mut saw_any_header = false;
    loop {
        let mut line = String::new();
        let n = r.read_line(&mut line)?;
        if n == 0 {
            // EOF: clean iff it landed on a frame boundary (no partial headers seen).
            return if saw_any_header {
                Err(io::Error::new(io::ErrorKind::UnexpectedEof, "eof mid-header"))
            } else { Ok(None) };
        }
        let t = line.trim_end_matches(['\r', '\n']);
        if t.is_empty() { break; } // end of headers
        saw_any_header = true;
        if let Some(v) = t.strip_prefix("Content-Length:") {
            content_length = Some(v.trim().parse()
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad Content-Length"))?);
        }
    }
    let len = content_length
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length"))?;
    if len > LSP_MAX_FRAME_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "Content-Length exceeds cap"));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    let v = serde_json::from_slice(&buf)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(Some(v))
}

/// Map an LSP position (0-based `line`, UTF-16 code-unit `character`) to a byte offset into `text`.
/// Lines split on '\n' (we sent the text; wordcartel buffers are '\n'-normalized). A `character`
/// past the line end clamps to the line end; a `character` landing inside a code point's UTF-16
/// width maps to that code point's start (never splits a char). `None` when `line` exceeds the
/// text's line count.
pub fn utf16_pos_to_byte(text: &str, line: u32, character: u32) -> Option<usize> {
    let mut line_start = 0usize;
    // Find the byte offset where `line` begins.
    if line > 0 {
        let mut seen = 0u32;
        let mut idx = 0usize;
        for (i, ch) in text.char_indices() {
            if ch == '\n' {
                seen += 1;
                if seen == line { idx = i + 1; break; }
            }
        }
        if seen < line { return None; } // line past EOF
        line_start = idx;
    }
    // Walk the target line, accumulating UTF-16 units. When `character` lands AT or INSIDE the
    // current scalar's UTF-16 width — i.e. character < u16_count + width — map to that scalar's
    // START byte, so a position inside a surrogate pair (🙂, char 1) never splits it and clamps to
    // byte 0 of the scalar (round-1 IMPORTANT 3). Only advance when the target is strictly past
    // this scalar.
    let mut u16_count = 0u32;
    for (off, ch) in text[line_start..].char_indices() {
        if ch == '\n' { return Some(line_start + off); } // clamp to line end
        let width = ch.len_utf16() as u32;
        if character < u16_count.saturating_add(width) {
            return Some(line_start + off);
        }
        u16_count = u16_count.saturating_add(width);
    }
    Some(text.len()) // ran off the end (last line, no trailing '\n') → clamp to end
}

/// Half-open byte range for an LSP range; `None` if either end is unmappable or end < start.
pub fn lsp_range_to_bytes(text: &str, start: (u32, u32), end: (u32, u32))
    -> Option<std::ops::Range<usize>> {
    let s = utf16_pos_to_byte(text, start.0, start.1)?;
    let e = utf16_pos_to_byte(text, end.0, end.1)?;
    if e < s { None } else { Some(s..e) }
}

/// E11 §4: map ONE CodeAction to a `Suggestion` targeting `anchor`, accepting fix kinds via
/// the ENGINE's table (`accept_kind` — a fn so this stays engine-agnostic plumbing) and BOTH
/// edit shapes: `edit.changes[uri]` (harper and vale, both probe-verified) and
/// `edit.documentChanges[]` (ltex, probe-verified exclusive). The action's edit range is a
/// MATCHING KEY against `anchor` and is then discarded — `Suggestion` carries text only
/// (the apply-safety invariant, spec §3.3).
pub(crate) fn action_fix_suggestion(
    action: &serde_json::Value, our_uri: &str, doc_text: &str,
    anchor: &std::ops::Range<usize>, accept_kind: impl Fn(&str) -> bool,
) -> Option<Suggestion> {
    let kind = action.get("kind").and_then(|k| k.as_str())?;
    if !accept_kind(kind) { return None; }
    let edit = action.get("edit")?;
    // Shape 1: edit.changes[uri] = [TextEdit].
    if let Some(edits) = edit.get("changes").and_then(|c| c.as_object())
        .and_then(|c| c.get(our_uri)).and_then(|e| e.as_array())
    {
        return edits_to_suggestion(edits, doc_text, anchor);
    }
    // Shape 2: edit.documentChanges[] = [{textDocument{uri}, edits: [TextEdit]}].
    if let Some(dcs) = edit.get("documentChanges").and_then(|d| d.as_array()) {
        for dc in dcs {
            if dc.get("textDocument").and_then(|t| t.get("uri")).and_then(|u| u.as_str())
                != Some(our_uri) { continue; }
            if let Some(edits) = dc.get("edits").and_then(|e| e.as_array()) {
                if let Some(s) = edits_to_suggestion(edits, doc_text, anchor) { return Some(s); }
            }
        }
    }
    None
}

/// The shared TextEdit→Suggestion rules (extracted verbatim from `quickfix_suggestion`'s
/// loop): an edit range equal to `anchor` ⇒ ReplaceWith/Remove; an empty range at
/// `anchor.end` ⇒ InsertAfter; anything else ⇒ no match.
fn edits_to_suggestion(edits: &[serde_json::Value], doc_text: &str,
    anchor: &std::ops::Range<usize>) -> Option<Suggestion> {
    for te in edits {
        let new_text = te.get("newText")?.as_str()?.to_string();
        let r = te.get("range")?;
        let s = (r["start"]["line"].as_u64()? as u32, r["start"]["character"].as_u64()? as u32);
        let e = (r["end"]["line"].as_u64()? as u32, r["end"]["character"].as_u64()? as u32);
        let er = lsp_range_to_bytes(doc_text, s, e)?;
        if er == *anchor {
            return Some(if new_text.is_empty() { Suggestion::Remove }
                        else { Suggestion::ReplaceWith(new_text) });
        }
        if er.is_empty() && er.start == anchor.end {
            return Some(Suggestion::InsertAfter(new_text));
        }
    }
    None
}

/// E11 §4: ALL matching actions for `anchor`, response order, deduped (the shipped `break`
/// capped attachment at one suggestion per diagnostic — multi-candidate is real: ltex
/// 2-for-`recieve`, vale 5-for-one, both probe-verified).
pub(crate) fn collect_fix_suggestions(
    actions: &[serde_json::Value], our_uri: &str, doc_text: &str,
    anchor: &std::ops::Range<usize>, accept_kind: impl Fn(&str) -> bool,
) -> Vec<Suggestion> {
    let mut out: Vec<Suggestion> = Vec::new();
    for a in actions {
        if let Some(s) = action_fix_suggestion(a, our_uri, doc_text, anchor, &accept_kind) {
            if !out.contains(&s) { out.push(s); }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::{Cursor, Read};

    // ---- doc_uri --------------------------------------------------------------------------

    #[test]
    fn doc_uri_is_opaque_and_generation_tagged() {
        assert_eq!(doc_uri(BufferId(7), 3), "untitled:wcartel-7-3");
    }

    #[test]
    fn doc_uri_distinct_generations_yield_distinct_uris() {
        let a = doc_uri(BufferId(7), 3);
        let b = doc_uri(BufferId(7), 4);
        assert_ne!(a, b);
    }

    // ---- framing ----------------------------------------------------------------------------

    #[test]
    fn write_then_read_frame_round_trips() {
        let msg = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"});
        let mut buf: Vec<u8> = Vec::new();
        write_frame(&mut buf, &msg).expect("write_frame");
        let mut cur = Cursor::new(buf);
        let got = read_frame(&mut cur).expect("read_frame").expect("Some frame");
        assert_eq!(got, msg);
    }

    #[test]
    fn read_frame_handles_two_back_to_back_frames() {
        let a = json!({"jsonrpc": "2.0", "id": 1, "method": "foo"});
        let b = json!({"jsonrpc": "2.0", "id": 2, "method": "bar"});
        let mut buf: Vec<u8> = Vec::new();
        write_frame(&mut buf, &a).expect("write a");
        write_frame(&mut buf, &b).expect("write b");
        let mut cur = Cursor::new(buf);
        let got_a = read_frame(&mut cur).expect("read a").expect("Some a");
        let got_b = read_frame(&mut cur).expect("read b").expect("Some b");
        assert_eq!(got_a, a);
        assert_eq!(got_b, b);
    }

    #[test]
    fn read_frame_returns_none_on_empty_input() {
        let mut cur = Cursor::new(Vec::<u8>::new());
        let got = read_frame(&mut cur).expect("read_frame ok");
        assert_eq!(got, None);
    }

    #[test]
    fn read_frame_errors_on_body_shorter_than_content_length() {
        // Claim 100 bytes but supply far fewer -> read_exact hits EOF mid-body -> Err.
        let raw = b"Content-Length: 100\r\n\r\n{\"a\":1}".to_vec();
        let mut cur = Cursor::new(raw);
        let got = read_frame(&mut cur);
        assert!(got.is_err());
    }

    #[test]
    fn read_frame_rejects_absurd_content_length_without_panicking() {
        // Near usize::MAX: parses fine into a usize but must never reach `vec![0u8; len]`.
        let raw = b"Content-Length: 18446744073709551615\r\n\r\n".to_vec();
        let mut cur = Cursor::new(raw);
        let got = read_frame(&mut cur);
        assert!(got.is_err());
        assert_eq!(got.unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn read_frame_rejects_content_length_just_over_the_cap() {
        let claimed = crate::limits::LSP_MAX_FRAME_BYTES + 1;
        let raw = format!("Content-Length: {claimed}\r\n\r\n").into_bytes();
        let mut cur = Cursor::new(raw);
        let got = read_frame(&mut cur);
        assert!(got.is_err());
        assert_eq!(got.unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    /// A reader that yields only a few bytes per `read` call, to exercise `read_frame` against a
    /// split/short read (as a real pipe can deliver).
    struct ChunkReader {
        data: Vec<u8>,
        pos: usize,
        chunk: usize,
    }

    impl Read for ChunkReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let remaining = self.data.len() - self.pos;
            let n = remaining.min(self.chunk).min(buf.len());
            buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    #[test]
    fn read_frame_handles_split_reads() {
        let msg = json!({"jsonrpc": "2.0", "id": 1, "method": "split"});
        let mut raw: Vec<u8> = Vec::new();
        write_frame(&mut raw, &msg).expect("write_frame");
        let chunked = ChunkReader { data: raw, pos: 0, chunk: 3 };
        let mut buffered = io::BufReader::new(chunked);
        let got = read_frame(&mut buffered).expect("read_frame").expect("Some frame");
        assert_eq!(got, msg);
    }

    // ---- utf16_pos_to_byte ------------------------------------------------------------------

    #[test]
    fn utf16_cafe_teh_maps_to_byte_six() {
        // "café teh" — "café " = c(1) a(1) f(1) é(2 bytes,1 utf16) space(1) = 6 bytes, 5 utf16 units.
        let text = "café teh";
        assert_eq!(utf16_pos_to_byte(text, 0, 5), Some(6));
    }

    #[test]
    fn utf16_cjk_column_mapping() {
        // "中文 x" — each CJK char is 1 UTF-16 unit / 3 bytes; then a 1-byte space, then 'x'.
        let text = "中文 x";
        assert_eq!(utf16_pos_to_byte(text, 0, 0), Some(0)); // start of 中
        assert_eq!(utf16_pos_to_byte(text, 0, 1), Some(3)); // start of 文
        assert_eq!(utf16_pos_to_byte(text, 0, 2), Some(6)); // start of space
        assert_eq!(utf16_pos_to_byte(text, 0, 3), Some(7)); // start of x
    }

    #[test]
    fn utf16_astral_surrogate_interior_clamps_to_scalar_start() {
        // "🙂ab" — 🙂 is 2 UTF-16 units / 4 bytes; a landing INSIDE the pair clamps to byte 0.
        let text = "🙂ab";
        assert_eq!(utf16_pos_to_byte(text, 0, 0), Some(0));
        assert_eq!(utf16_pos_to_byte(text, 0, 1), Some(0)); // interior of surrogate pair, NOT 4
        assert_eq!(utf16_pos_to_byte(text, 0, 2), Some(4)); // 'a'
        assert_eq!(utf16_pos_to_byte(text, 0, 3), Some(5)); // 'b'
    }

    #[test]
    fn utf16_past_eol_clamps_to_line_end() {
        let text = "ab\ncd";
        assert_eq!(utf16_pos_to_byte(text, 0, 100), Some(2)); // clamps to end of "ab"
    }

    #[test]
    fn utf16_line_past_eof_is_none() {
        let text = "ab\ncd";
        assert_eq!(utf16_pos_to_byte(text, 5, 0), None);
    }

    #[test]
    fn utf16_last_line_no_trailing_newline_clamps_to_end() {
        let text = "ab\ncd";
        assert_eq!(utf16_pos_to_byte(text, 1, 100), Some(5)); // end of "cd", no trailing '\n'
    }

    // ---- lsp_range_to_bytes -------------------------------------------------------------------

    #[test]
    fn lsp_range_to_bytes_round_trips_a_simple_range() {
        // "café teh" is 9 bytes total (é is 2 bytes, 1 UTF-16 unit); "teh" starts at utf16
        // character 5 / byte 6 and the range runs to the end of the string, utf16 char 8 / byte 9.
        let text = "café teh";
        let r = lsp_range_to_bytes(text, (0, 5), (0, 8)).expect("Some range");
        assert_eq!(r, 6..9);
    }

    #[test]
    fn lsp_range_to_bytes_none_when_end_before_start() {
        let text = "café teh";
        assert_eq!(lsp_range_to_bytes(text, (0, 8), (0, 5)), None);
    }

    // ── VERBATIM probe captures (wire-regression tier; do not simplify) ─────────────────────

    #[test]
    fn verbatim_ltex_recieve_accept_action_maps() {
        // ltex-probe-results.md Q2, the `recieve` → `receive` accept-suggestion action,
        // fields as captured (documentChanges shape; diagnostics echo included).
        let doc = "# Probe Document\n\nThis is a probe document for ltex-ls-plus. It was written by us to test the checker.\n\nThe the cake was eaten by the dog.\n\nI recieve many emails every day due to the fact that people email me constantly.\n";
        let action = serde_json::json!({
            "title": "Use 'receive'",
            "kind": "quickfix.ltex.acceptSuggestions",
            "diagnostics": [{
                "range": {"start": {"line": 6, "character": 2}, "end": {"line": 6, "character": 9}},
                "severity": 3, "code": "MORFOLOGIK_RULE_EN_US",
                "codeDescription": {"href": "https://community.languagetool.org/rule/show/MORFOLOGIK_RULE_EN_US?lang=en-US"},
                "source": "LTeX", "message": "'recieve': Possible spelling mistake found."
            }],
            "edit": {"documentChanges": [{
                "textDocument": {"version": 1, "uri": "file:///probe/doc.md"},
                "edits": [{"range": {"start": {"line": 6, "character": 2},
                                     "end": {"line": 6, "character": 9}},
                           "newText": "receive"}]}]}
        });
        let anchor = lsp_range_to_bytes(doc, (6, 2), (6, 9)).expect("anchor range");
        assert_eq!(action_fix_suggestion(&action, "file:///probe/doc.md", doc, &anchor,
            |k| k == "quickfix.ltex.acceptSuggestions"),
            Some(Suggestion::ReplaceWith("receive".into())));
    }

    /// KEPT after the vale-ls provider was removed: it pins the ENGINE-GENERIC parser for a
    /// bare `"quickfix"` action carrying `edit.changes[uri]`, against a real captured server
    /// payload. That is harper's shape too (and vale's own CLI `Action` output is the same fix
    /// model), so the coverage outlives the transport — it names no engine type, only
    /// `action_fix_suggestion`.
    #[test]
    fn verbatim_vale_misspelling_action_maps() {
        // vale-probe-results.md §1+§2 verbatim: the first of the 5 spelling quickfixes, with
        // the captured diagnostic echoed IN FULL — including the native `data` object and the
        // TYPOGRAPHIC-quote title (multibyte, deliberate — the capture's exact bytes). The
        // doc string places `mispeling` at line 2, chars 20..29 (the captured positions);
        // fix the DOC string, never the captured JSON.
        let doc = "# Probe\n\nThe word here has a mispeling in it.\n";
        let uri = "file:///home/jkeim/projects/groundwords/scratchpad/e11/probe/vale/test.md";
        let action = serde_json::json!({
            "diagnostics": [{
                "code": "Vale.Spelling",
                "data": {
                    "Action": { "Name": "suggest", "Params": ["spellings"] },
                    "Check": "Vale.Spelling",
                    "Description": "",
                    "Line": 3,
                    "Link": "",
                    "Match": "mispeling",
                    "Message": "Did you really mean 'mispeling'?",
                    "Severity": "error",
                    "Span": [21, 29]
                },
                "message": "Did you really mean 'mispeling'?",
                "range": {"end": {"character": 29, "line": 2}, "start": {"character": 20, "line": 2}},
                "severity": 1,
                "source": "vale-ls"
            }],
            "edit": {"changes": { uri: [{
                "newText": "misspelling",
                "range": {"end": {"character": 29, "line": 2}, "start": {"character": 20, "line": 2}}}]}},
            "kind": "quickfix",
            "title": "Replace with \u{2018}misspelling\u{2019}"
        });
        let anchor = lsp_range_to_bytes(doc, (2, 20), (2, 29)).expect("anchor range");
        assert_eq!(&doc[anchor.clone()], "mispeling", "the captured range targets the word");
        assert_eq!(action_fix_suggestion(&action, uri, doc, &anchor, |k| k == "quickfix"),
            Some(Suggestion::ReplaceWith("misspelling".into())));
    }

    // ── E11 T2: the engine-parameterized fix mapping ────────────────────────────────────────

    /// ltex accept-suggestion action (probe Q2 verbatim shape): namespaced kind +
    /// edit.documentChanges[].
    fn ltex_accept_action(uri: &str, new_text: &str) -> serde_json::Value {
        serde_json::json!({
            "title": format!("Use '{new_text}'"),
            "kind": "quickfix.ltex.acceptSuggestions",
            "edit": {"documentChanges": [{
                "textDocument": {"version": 1, "uri": uri},
                "edits": [{"range": {"start": {"line": 0, "character": 2},
                                     "end": {"line": 0, "character": 9}},
                           "newText": new_text}]}]}
        })
    }

    #[test]
    fn ltex_documentchanges_accept_action_maps_to_replace_with() {
        let text = "a recieve b";
        let anchor = 2..9usize;
        let a = ltex_accept_action("untitled:wcartel-0-1", "receive");
        assert_eq!(
            action_fix_suggestion(&a, "untitled:wcartel-0-1", text, &anchor,
                |k| k == "quickfix.ltex.acceptSuggestions"),
            Some(Suggestion::ReplaceWith("receive".into())));
    }

    #[test]
    fn ltex_command_only_kinds_are_excluded_by_engine_knowledge() {
        // Probe Q2: addToDictionary / hideFalsePositives / disableRules are command-only.
        let a = serde_json::json!({"title": "Add 'x' to dictionary",
            "kind": "quickfix.ltex.addToDictionary",
            "command": {"title": "Add", "command": "_ltex.addToDictionary", "arguments": []}});
        assert_eq!(action_fix_suggestion(&a, "u", "abc", &(0..1),
            |k| k == "quickfix.ltex.acceptSuggestions"), None);
    }

    /// Also KEPT after the provider removal — multi-candidate attachment on the `edit.changes`
    /// shape is engine-generic (harper takes the same path; ltex is the `documentChanges` twin
    /// above), and this is the only test that pins ALL candidates attaching in response order.
    #[test]
    fn vale_changes_shape_still_maps_and_collect_attaches_all_candidates() {
        // Probe §2: bare "quickfix" + edit.changes[uri]; 5 candidates for one diagnostic.
        let text = "abc mispeling xyz";
        let anchor = 4..13usize;
        let mk = |t: &str| serde_json::json!({"title": format!("Replace with '{t}'"),
            "kind": "quickfix",
            "edit": {"changes": {"file:///t.md": [{"newText": t,
                "range": {"start": {"line": 0, "character": 4},
                          "end": {"line": 0, "character": 13}}}]}}});
        let actions: Vec<serde_json::Value> =
            ["misspelling", "dispelling", "misdealing"].iter().map(|t| mk(t)).collect();
        let got = collect_fix_suggestions(&actions, "file:///t.md", text, &anchor,
            |k| k == "quickfix");
        assert_eq!(got, vec![
            Suggestion::ReplaceWith("misspelling".into()),
            Suggestion::ReplaceWith("dispelling".into()),
            Suggestion::ReplaceWith("misdealing".into()),
        ], "ALL matching actions attach, response order kept — the `break` is gone");
    }

    #[test]
    fn collect_dedupes_identical_suggestions_and_keeps_range_equality_gate() {
        let text = "abc mispeling xyz";
        let anchor = 4..13usize;
        let same = serde_json::json!({"kind": "quickfix",
            "edit": {"changes": {"u": [{"newText": "misspelling",
                "range": {"start": {"line": 0, "character": 4},
                          "end": {"line": 0, "character": 13}}}]}}});
        let elsewhere = serde_json::json!({"kind": "quickfix",
            "edit": {"changes": {"u": [{"newText": "zzz",
                "range": {"start": {"line": 0, "character": 0},
                          "end": {"line": 0, "character": 3}}}]}}});
        let got = collect_fix_suggestions(
            &[same.clone(), same, elsewhere], "u", text, &anchor, |k| k == "quickfix");
        assert_eq!(got, vec![Suggestion::ReplaceWith("misspelling".into())],
            "duplicates collapse; an edit not targeting the ANCHOR range never attaches \
             (the apply-safety invariant: server ranges are a matching key only)");
    }

    #[test]
    fn engine_is_fix_kind_tables_match_the_probes() {
        use crate::lsp_client::LspEngine;
        assert!(crate::harper_ls::HarperEngine::is_fix_kind("quickfix"));
        assert!(!crate::harper_ls::HarperEngine::is_fix_kind("quickfix.ltex.acceptSuggestions"));
        assert!(crate::ltex_ls::LtexEngine::is_fix_kind("quickfix.ltex.acceptSuggestions"));
        assert!(!crate::ltex_ls::LtexEngine::is_fix_kind("quickfix.ltex.addToDictionary"));
        assert!(!crate::ltex_ls::LtexEngine::is_fix_kind("quickfix"));
    }
}
