//! System-clipboard sync around the in-process Register. The Register is always
//! source of truth; everything here is best-effort and must never block the loop.

pub const OSC52_MAX_ENCODED: usize = 100_000;

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
}
