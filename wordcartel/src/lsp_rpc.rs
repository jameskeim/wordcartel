//! Pure/IO-light LSP plumbing (Effort A): Content-Length framing, JSON-RPC envelopes over
//! `serde_json::Value`, opaque document URIs, UTF-16→byte position conversion, and
//! codeAction `TextEdit`→`Suggestion` mapping. No process IO lives here — see harper_ls.rs.

use crate::editor::BufferId;
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
    let mut cur_line = 0u32;
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
        cur_line = line;
    }
    let _ = cur_line;
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

/// Extract a `Suggestion::ReplaceWith` from a harper quickfix `CodeAction` value, matched to a
/// diagnostic whose byte range is `d`. Returns `None` for command-only actions (`kind != "quickfix"`
/// or no `edit`), for edits on a different uri, or for an edit that does not correspond to `d`.
/// (harper 2.1.0 verified: `edit.changes[uri] = [{newText, range}]` with clean `newText`.)
pub fn quickfix_suggestion(
    action: &serde_json::Value, our_uri: &str, doc_text: &str, d: &std::ops::Range<usize>,
) -> Option<Suggestion> {
    if action.get("kind").and_then(|k| k.as_str()) != Some("quickfix") { return None; }
    let changes = action.get("edit")?.get("changes")?.as_object()?;
    let edits = changes.get(our_uri)?.as_array()?;
    for te in edits {
        let new_text = te.get("newText")?.as_str()?.to_string();
        let r = te.get("range")?;
        let s = (r["start"]["line"].as_u64()? as u32, r["start"]["character"].as_u64()? as u32);
        let e = (r["end"]["line"].as_u64()? as u32, r["end"]["character"].as_u64()? as u32);
        let er = lsp_range_to_bytes(doc_text, s, e)?;
        // Map to our three-variant Suggestion the exact inverse of build_range_replace (spec §6.2).
        if er == *d {
            return Some(if new_text.is_empty() { Suggestion::Remove }
                        else { Suggestion::ReplaceWith(new_text) });
        }
        if er.is_empty() && er.start == d.end {
            return Some(Suggestion::InsertAfter(new_text));
        }
    }
    None
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

    // ---- quickfix_suggestion ------------------------------------------------------------------

    #[test]
    fn quickfix_matching_range_yields_replace_with() {
        let uri = "untitled:wcartel-1-0";
        let text = "I has a cat.";
        let d = 2..5; // "has"
        let action = json!({
            "kind": "quickfix",
            "edit": {
                "changes": {
                    uri: [{"newText": "the", "range": {
                        "start": {"line": 0, "character": 2},
                        "end": {"line": 0, "character": 5}
                    }}]
                }
            }
        });
        let got = quickfix_suggestion(&action, uri, text, &d);
        assert_eq!(got, Some(Suggestion::ReplaceWith("the".to_string())));
    }

    #[test]
    fn quickfix_empty_new_text_at_d_yields_remove() {
        let uri = "untitled:wcartel-1-0";
        let text = "a  b";
        let d = 1..2; // one of the double spaces
        let action = json!({
            "kind": "quickfix",
            "edit": {
                "changes": {
                    uri: [{"newText": "", "range": {
                        "start": {"line": 0, "character": 1},
                        "end": {"line": 0, "character": 2}
                    }}]
                }
            }
        });
        let got = quickfix_suggestion(&action, uri, text, &d);
        assert_eq!(got, Some(Suggestion::Remove));
    }

    #[test]
    fn quickfix_empty_range_at_d_end_yields_insert_after() {
        let uri = "untitled:wcartel-1-0";
        let text = "cat sat";
        let d = 0..3; // "cat"
        let action = json!({
            "kind": "quickfix",
            "edit": {
                "changes": {
                    uri: [{"newText": ",", "range": {
                        "start": {"line": 0, "character": 3},
                        "end": {"line": 0, "character": 3}
                    }}]
                }
            }
        });
        let got = quickfix_suggestion(&action, uri, text, &d);
        assert_eq!(got, Some(Suggestion::InsertAfter(",".to_string())));
    }

    #[test]
    fn quickfix_command_only_action_is_none() {
        let uri = "untitled:wcartel-1-0";
        let text = "cat sat";
        let d = 0..3;
        let action = json!({"kind": null, "command": {"title": "Add to dictionary"}});
        assert_eq!(quickfix_suggestion(&action, uri, text, &d), None);
    }

    #[test]
    fn quickfix_foreign_uri_is_none() {
        let uri = "untitled:wcartel-1-0";
        let foreign = "untitled:wcartel-2-0";
        let text = "cat sat";
        let d = 0..3;
        let action = json!({
            "kind": "quickfix",
            "edit": {
                "changes": {
                    foreign: [{"newText": "dog", "range": {
                        "start": {"line": 0, "character": 0},
                        "end": {"line": 0, "character": 3}
                    }}]
                }
            }
        });
        assert_eq!(quickfix_suggestion(&action, uri, text, &d), None);
    }
}
