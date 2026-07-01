//! System-clipboard sync around the in-process Register. The Register is always
//! source of truth; everything here is best-effort and must never block the loop.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;

pub use crate::limits::{OSC52_MAX_ENCODED, PASTE_MAX_BYTES};

// ---------------------------------------------------------------------------
// PasteIntent, ClipReq, next_paste_id
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub struct PasteIntent {
    pub id: u64,
    pub buffer_id: crate::editor::BufferId,
}

#[derive(Debug)]
pub enum ClipReq {
    Set(String),
    Get { id: u64, buffer_id: crate::editor::BufferId },
    Shutdown,
}

static PASTE_SEQ: AtomicU64 = AtomicU64::new(1);

pub fn next_paste_id() -> u64 {
    PASTE_SEQ.fetch_add(1, Ordering::Relaxed)
}

/// Called by run() after reduce, before the frame draw. `out` is the terminal
/// backend writer (a Vec<u8> in tests). Never blocks: the channel is unbounded.
pub fn drain_clipboard_intents(
    editor: &mut crate::editor::Editor,
    out: &mut impl std::io::Write,
    clip_tx: &Sender<ClipReq>,
    msg_tx: &Sender<crate::app::Msg>,
) {
    if let Some(text) = editor.clipboard_sync_request.take() {
        if let Some(bytes) = osc52_set(&text) {
            let _ = out.write_all(&bytes);
            let _ = out.flush();
        }
        if clip_tx.send(ClipReq::Set(text)).is_err() {
            editor.status = "clipboard unavailable".to_string();
        }
    }
    if let Some(pi) = editor.clipboard_get_pending.take() {
        if clip_tx.send(ClipReq::Get { id: pi.id, buffer_id: pi.buffer_id }).is_err() {
            // No worker (tests / shutdown): notify, then fall back to the register paste path.
            editor.status = "clipboard unavailable".to_string();
            let _ = msg_tx.send(crate::app::Msg::ClipboardPaste {
                id: pi.id,
                buffer_id: pi.buffer_id,
                text: None,
            });
        }
    }
}

/// Spawn the long-lived clipboard worker. arboard is initialized INSIDE the worker
/// (off the startup path); availability is reported once via Msg::ClipboardAvailability.
pub fn spawn_worker(msg_tx: Sender<crate::app::Msg>) -> Sender<ClipReq> {
    let (tx, rx) = std::sync::mpsc::channel::<ClipReq>();
    std::thread::Builder::new()
        .name("wcartel-clipboard".into())
        .spawn(move || {
            let mut backend: Box<dyn ClipboardBackend> = match ArboardBackend::try_new() {
                Some(b) => {
                    let _ = msg_tx.send(crate::app::Msg::ClipboardAvailability(true));
                    Box::new(b)
                }
                None => {
                    let _ = msg_tx.send(crate::app::Msg::ClipboardAvailability(false));
                    Box::new(NullBackend)
                }
            };
            while let Ok(req) = rx.recv() {
                match req {
                    ClipReq::Set(s) => backend.set(&s),
                    ClipReq::Get { id, buffer_id } => {
                        let text = backend.get().filter(|s| !s.is_empty());
                        let _ = msg_tx.send(crate::app::Msg::ClipboardPaste { id, buffer_id, text });
                    }
                    ClipReq::Shutdown => break,
                }
            }
        })
        .expect("spawn clipboard worker");
    tx
}

pub trait ClipboardBackend: Send {
    fn set(&mut self, text: &str);
    fn get(&mut self) -> Option<String>;
}

#[allow(dead_code)] // wired in Task 2/4
pub struct ArboardBackend {
    cb: arboard::Clipboard,
}

#[allow(dead_code)] // wired in Task 2/4
impl ArboardBackend {
    /// Init arboard; None if no display / unsupported (caller falls back to Null).
    pub fn try_new() -> Option<ArboardBackend> {
        arboard::Clipboard::new().ok().map(|cb| ArboardBackend { cb })
    }
}

impl ClipboardBackend for ArboardBackend {
    fn set(&mut self, text: &str) {
        let _ = self.cb.set_text(text.to_owned()); // swallow errors
    }
    fn get(&mut self) -> Option<String> {
        self.cb.get_text().ok()
    }
}

#[allow(dead_code)] // wired in Task 2/4
pub struct NullBackend;

impl ClipboardBackend for NullBackend {
    fn set(&mut self, _text: &str) {}
    fn get(&mut self) -> Option<String> {
        None
    }
}

pub struct FakeBackend {
    pub slot: Option<String>,
}

impl ClipboardBackend for FakeBackend {
    fn set(&mut self, text: &str) {
        self.slot = Some(text.to_owned());
    }
    fn get(&mut self) -> Option<String> {
        self.slot.clone()
    }
}

const B64: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub(crate) fn base64_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        let n = ((b0 as u32) << 16) | ((b1 as u32) << 8) | (b2 as u32);
        out.push(B64[((n >> 18) & 63) as usize] as char);
        out.push(B64[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            B64[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// OSC 52 "set clipboard" sequence (ST-terminated). None when over the encoded cap.
#[allow(dead_code)] // wired in Task 4
pub fn osc52_set(text: &str) -> Option<Vec<u8>> {
    let b64 = base64_encode(text.as_bytes());
    if b64.len() > OSC52_MAX_ENCODED {
        return None;
    }
    let mut v = Vec::with_capacity(b64.len() + 9);
    v.extend_from_slice(b"\x1b]52;c;");
    v.extend_from_slice(b64.as_bytes());
    v.extend_from_slice(b"\x1b\\");
    Some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"hi"), "aGk=");
    }

    #[test]
    fn osc52_frames_with_st_terminator() {
        assert_eq!(osc52_set("hi").unwrap(), b"\x1b]52;c;aGk=\x1b\\".to_vec());
    }

    #[test]
    fn osc52_skips_oversize_payload() {
        // A raw string whose base64 exceeds the cap → None (skip OSC 52).
        let big = "a".repeat(OSC52_MAX_ENCODED); // base64 ~4/3 larger → over cap
        assert!(osc52_set(&big).is_none());
    }

    #[test]
    fn fake_backend_round_trips() {
        let mut b = FakeBackend { slot: None };
        assert_eq!(b.get(), None);
        b.set("x");
        assert_eq!(b.get(), Some("x".to_string()));
    }

    #[test]
    fn null_backend_is_inert() {
        let mut b = NullBackend;
        b.set("x");
        assert_eq!(b.get(), None);
    }

    #[test]
    fn drain_copy_emits_osc52_and_set_request() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        e.clipboard_sync_request = Some("hi".into());
        let (clip_tx, clip_rx) = std::sync::mpsc::channel();
        let (msg_tx, _msg_rx) = std::sync::mpsc::channel();
        let mut out: Vec<u8> = Vec::new();
        drain_clipboard_intents(&mut e, &mut out, &clip_tx, &msg_tx);
        assert_eq!(out, b"\x1b]52;c;aGk=\x1b\\".to_vec(), "OSC 52 written to the terminal writer");
        match clip_rx.try_recv() { Ok(ClipReq::Set(s)) => assert_eq!(s, "hi"), o => panic!("{o:?}") }
        assert!(e.clipboard_sync_request.is_none(), "intent cleared");
    }

    #[test]
    fn drain_paste_sends_get_request() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        let bid = e.active().id;
        e.clipboard_get_pending = Some(PasteIntent { id: 7, buffer_id: bid });
        let (clip_tx, clip_rx) = std::sync::mpsc::channel();
        let (msg_tx, _msg_rx) = std::sync::mpsc::channel();
        let mut out: Vec<u8> = Vec::new();
        drain_clipboard_intents(&mut e, &mut out, &clip_tx, &msg_tx);
        match clip_rx.try_recv() {
            Ok(ClipReq::Get { id, buffer_id }) => { assert_eq!(id, 7); assert_eq!(buffer_id, bid); }
            o => panic!("{o:?}"),
        }
        assert!(e.clipboard_get_pending.is_none());
    }

    #[test]
    fn drain_paste_with_dead_worker_falls_back_to_none_msg() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        let bid = e.active().id;
        e.clipboard_get_pending = Some(PasteIntent { id: 1, buffer_id: bid });
        let (clip_tx, clip_rx) = std::sync::mpsc::channel::<ClipReq>();
        drop(clip_rx); // worker gone → send fails
        let (msg_tx, msg_rx) = std::sync::mpsc::channel();
        let mut out: Vec<u8> = Vec::new();
        drain_clipboard_intents(&mut e, &mut out, &clip_tx, &msg_tx);
        match msg_rx.try_recv() {
            Ok(crate::app::Msg::ClipboardPaste { text: None, buffer_id, .. }) => assert_eq!(buffer_id, bid),
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn a_dead_clipboard_worker_sets_the_status_notice() {
        let (clip_tx, clip_rx) = std::sync::mpsc::channel::<ClipReq>();
        drop(clip_rx); // worker is gone: every send now errors
        let (msg_tx, _msg_rx) = std::sync::mpsc::channel::<crate::app::Msg>();
        let mut out: Vec<u8> = Vec::new();
        let mut ed = crate::editor::Editor::new_from_text("x\n", None, (80, 24));
        let bid = ed.active().id;

        // A pending Get with no worker -> notice + None fallback.
        ed.clipboard_get_pending = Some(PasteIntent { id: 1, buffer_id: bid });
        drain_clipboard_intents(&mut ed, &mut out, &clip_tx, &msg_tx);
        assert_eq!(ed.status, "clipboard unavailable");

        // A pending Set with no worker -> notice.
        ed.status.clear();
        ed.clipboard_sync_request = Some("hello".to_string());
        drain_clipboard_intents(&mut ed, &mut out, &clip_tx, &msg_tx);
        assert_eq!(ed.status, "clipboard unavailable");
    }
}
