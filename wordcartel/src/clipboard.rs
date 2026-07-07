//! System-clipboard sync around the in-process Register. The Register is always
//! source of truth; everything here is best-effort and must never block the loop.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;

pub use crate::limits::{OSC52_MAX_ENCODED, PASTE_MAX_BYTES};

// ---------------------------------------------------------------------------
// Provider detection: types + resolve_provider
// ---------------------------------------------------------------------------

/// Who owns the LOCAL system clipboard (Layer 1). Selected once at worker init and on a
/// runtime provider change; `Null` = register-only (no system clipboard).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer1Choice { WlCopy, Xclip, Xsel, WinYank, ClipExe, Arboard, Null }

/// OSC 52 framing (Layer 2). `Bare` outside a multiplexer; `Tmux`/`Screen` wrap the bare
/// sequence in the multiplexer's DCS passthrough.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Osc52Wrap { Bare, Tmux, Screen }

/// The resolved plan: Layer-1 owner + whether/how to also emit OSC 52. `osc52 == None`
/// means a local owner persists the clipboard, so we suppress the redundant terminal write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderPlan { pub layer1: Layer1Choice, pub osc52: Option<Osc52Wrap> }

/// Compile-time target OS class (drives arboard-native vs helper selection).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os { Linux, MacOs, Windows }

/// Environment snapshot for provider detection. `present` probes `$PATH` for a helper
/// binary; injected so tests need no real env and no spawning.
#[derive(Clone, Copy)]
pub struct ClipEnv {
    pub tmux: bool,    // $TMUX
    pub screen: bool,  // $STY
    pub ssh: bool,     // $SSH_TTY || $SSH_CONNECTION
    pub wayland: bool, // $WAYLAND_DISPLAY
    pub x11: bool,     // $DISPLAY
    pub wsl: bool,     // $WSL_DISTRO_NAME
    pub os: Os,
    pub present: fn(&str) -> bool,
}

/// Multiplexer wrap for the current environment (tmux beats screen beats bare).
fn wrap_for(env: &ClipEnv) -> Osc52Wrap {
    if env.tmux { Osc52Wrap::Tmux } else if env.screen { Osc52Wrap::Screen } else { Osc52Wrap::Bare }
}

/// Pick the Layer-1 owner under `Auto` (first match wins).
fn auto_layer1(env: &ClipEnv) -> Layer1Choice {
    if env.wsl {
        return if (env.present)("win32yank.exe") { Layer1Choice::WinYank } else { Layer1Choice::ClipExe };
    }
    if env.wayland {
        return if (env.present)("wl-copy") { Layer1Choice::WlCopy } else { Layer1Choice::Arboard };
    }
    if env.x11 {
        return if (env.present)("xclip") { Layer1Choice::Xclip }
               else if (env.present)("xsel") { Layer1Choice::Xsel }
               else { Layer1Choice::Arboard };
    }
    match env.os {
        Os::MacOs | Os::Windows => Layer1Choice::Arboard,
        Os::Linux => Layer1Choice::Null,
    }
}

/// Whether the chosen Layer-1 owner persists the clipboard locally on its own.
fn is_local_persisting(layer1: Layer1Choice, env: &ClipEnv) -> bool {
    match layer1 {
        Layer1Choice::WlCopy | Layer1Choice::Xclip | Layer1Choice::Xsel
        | Layer1Choice::WinYank | Layer1Choice::ClipExe => true,
        // arboard persists natively only where the OS owns the clipboard.
        Layer1Choice::Arboard => matches!(env.os, Os::MacOs | Os::Windows),
        Layer1Choice::Null => false,
    }
}

/// Resolve the environment (+ any override) into a concrete plan. Pure.
pub fn resolve_provider(env: &ClipEnv, forced: crate::config::ClipboardProvider) -> ProviderPlan {
    use crate::config::ClipboardProvider as P;
    match forced {
        P::Native => ProviderPlan { layer1: Layer1Choice::Arboard, osc52: None },
        P::Osc52  => ProviderPlan { layer1: Layer1Choice::Null, osc52: Some(wrap_for(env)) },
        P::Off    => ProviderPlan { layer1: Layer1Choice::Null, osc52: None },
        P::Auto => {
            let layer1 = auto_layer1(env);
            // Precedence: multiplexer/SSH forces OSC 52 (rule 1); else a persisting local
            // owner suppresses it (rule 2); else emit (rule 3).
            let osc52 = if env.tmux || env.screen || env.ssh {
                Some(wrap_for(env))
            } else if is_local_persisting(layer1, env) {
                None
            } else {
                Some(wrap_for(env))
            };
            ProviderPlan { layer1, osc52 }
        }
    }
}

/// Build a `ClipEnv` from the real process environment.
pub fn clip_env_from_process() -> ClipEnv {
    fn var_set(k: &str) -> bool { std::env::var_os(k).is_some_and(|v| !v.is_empty()) }
    fn on_path(bin: &str) -> bool {
        let Some(paths) = std::env::var_os("PATH") else { return false };
        std::env::split_paths(&paths).any(|dir| dir.join(bin).is_file())
    }
    let os = if cfg!(target_os = "macos") { Os::MacOs }
             else if cfg!(target_os = "windows") { Os::Windows }
             else { Os::Linux };
    ClipEnv {
        tmux: var_set("TMUX"),
        screen: var_set("STY"),
        ssh: var_set("SSH_TTY") || var_set("SSH_CONNECTION"),
        wayland: var_set("WAYLAND_DISPLAY"),
        x11: var_set("DISPLAY"),
        wsl: var_set("WSL_DISTRO_NAME"),
        os,
        present: on_path,
    }
}

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
        if let Some(bytes) = osc52_set(&text, Osc52Wrap::Bare) {
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
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
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

/// OSC 52 "set clipboard" sequence for `text`, framed per `wrap`. `None` when the
/// base64 payload exceeds `OSC52_MAX_ENCODED` (caller skips emission; Layer 1 still copies).
///
/// - `Bare`:   ESC ] 52 ; c ; <b64> ESC \
/// - `Tmux`:   ESC P tmux; <bare, every 0x1B doubled> ESC \   (tmux DCS passthrough)
/// - `Screen`: ESC P <bare> ESC \                              (screen DCS passthrough)
pub fn osc52_set(text: &str, wrap: Osc52Wrap) -> Option<Vec<u8>> {
    let b64 = base64_encode(text.as_bytes());
    if b64.len() > OSC52_MAX_ENCODED {
        return None;
    }
    let mut bare = Vec::with_capacity(b64.len() + 9);
    bare.extend_from_slice(b"\x1b]52;c;");
    bare.extend_from_slice(b64.as_bytes());
    bare.extend_from_slice(b"\x1b\\");
    let framed = match wrap {
        Osc52Wrap::Bare => bare,
        Osc52Wrap::Tmux => {
            // Double every ESC (0x1B) in the inner sequence, then wrap.
            let mut v = Vec::with_capacity(bare.len() + 16);
            v.extend_from_slice(b"\x1bPtmux;");
            for &byte in &bare {
                if byte == 0x1b { v.push(0x1b); }
                v.push(byte);
            }
            v.extend_from_slice(b"\x1b\\");
            v
        }
        Osc52Wrap::Screen => {
            let mut v = Vec::with_capacity(bare.len() + 4);
            v.extend_from_slice(b"\x1bP");
            v.extend_from_slice(&bare);
            v.extend_from_slice(b"\x1b\\");
            v
        }
    };
    Some(framed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_forced_native_is_arboard_no_osc52() {
        let env = ClipEnv { tmux: false, screen: false, ssh: false, wayland: true, x11: false,
                            wsl: false, os: Os::Linux, present: |_| true };
        assert_eq!(resolve_provider(&env, crate::config::ClipboardProvider::Native),
                   ProviderPlan { layer1: Layer1Choice::Arboard, osc52: None });
    }

    #[test]
    fn resolve_forced_osc52_is_null_plus_wrapped() {
        let env = ClipEnv { tmux: true, screen: false, ssh: false, wayland: true, x11: false,
                            wsl: false, os: Os::Linux, present: |_| true };
        assert_eq!(resolve_provider(&env, crate::config::ClipboardProvider::Osc52),
                   ProviderPlan { layer1: Layer1Choice::Null, osc52: Some(Osc52Wrap::Tmux) });
    }

    #[test]
    fn resolve_forced_off_is_register_only() {
        let env = ClipEnv { tmux: false, screen: false, ssh: false, wayland: false, x11: true,
                            wsl: false, os: Os::Linux, present: |_| true };
        assert_eq!(resolve_provider(&env, crate::config::ClipboardProvider::Off),
                   ProviderPlan { layer1: Layer1Choice::Null, osc52: None });
    }

    #[test]
    fn resolve_auto_local_wayland_with_helper_suppresses_osc52() {
        let env = ClipEnv { tmux: false, screen: false, ssh: false, wayland: true, x11: false,
                            wsl: false, os: Os::Linux, present: |b| b == "wl-copy" };
        assert_eq!(resolve_provider(&env, crate::config::ClipboardProvider::Auto),
                   ProviderPlan { layer1: Layer1Choice::WlCopy, osc52: None });
    }

    #[test]
    fn resolve_auto_wayland_no_helper_falls_to_arboard_and_emits_osc52() {
        let env = ClipEnv { tmux: false, screen: false, ssh: false, wayland: true, x11: false,
                            wsl: false, os: Os::Linux, present: |_| false };
        assert_eq!(resolve_provider(&env, crate::config::ClipboardProvider::Auto),
                   ProviderPlan { layer1: Layer1Choice::Arboard, osc52: Some(Osc52Wrap::Bare) });
    }

    #[test]
    fn resolve_auto_x11_prefers_xclip_then_xsel() {
        let both = ClipEnv { tmux: false, screen: false, ssh: false, wayland: false, x11: true,
                             wsl: false, os: Os::Linux, present: |b| b == "xclip" || b == "xsel" };
        assert_eq!(resolve_provider(&both, crate::config::ClipboardProvider::Auto).layer1, Layer1Choice::Xclip);
        let only_xsel = ClipEnv { present: |b| b == "xsel", ..both };
        assert_eq!(resolve_provider(&only_xsel, crate::config::ClipboardProvider::Auto).layer1, Layer1Choice::Xsel);
    }

    #[test]
    fn resolve_auto_tmux_forces_osc52_even_with_local_helper() {
        let env = ClipEnv { tmux: true, screen: false, ssh: false, wayland: true, x11: false,
                            wsl: false, os: Os::Linux, present: |_| true };
        // helper present (would persist) but multiplexer wins: emit, tmux-wrapped.
        let plan = resolve_provider(&env, crate::config::ClipboardProvider::Auto);
        assert_eq!(plan.layer1, Layer1Choice::WlCopy);
        assert_eq!(plan.osc52, Some(Osc52Wrap::Tmux));
    }

    #[test]
    fn resolve_auto_ssh_no_display_is_null_plus_bare_osc52() {
        let env = ClipEnv { tmux: false, screen: false, ssh: true, wayland: false, x11: false,
                            wsl: false, os: Os::Linux, present: |_| false };
        assert_eq!(resolve_provider(&env, crate::config::ClipboardProvider::Auto),
                   ProviderPlan { layer1: Layer1Choice::Null, osc52: Some(Osc52Wrap::Bare) });
    }

    #[test]
    fn resolve_auto_macos_is_arboard_no_osc52() {
        let env = ClipEnv { tmux: false, screen: false, ssh: false, wayland: false, x11: false,
                            wsl: false, os: Os::MacOs, present: |_| false };
        assert_eq!(resolve_provider(&env, crate::config::ClipboardProvider::Auto),
                   ProviderPlan { layer1: Layer1Choice::Arboard, osc52: None });
    }

    #[test]
    fn resolve_auto_wsl_prefers_win32yank_then_clip_exe() {
        let yank = ClipEnv { tmux: false, screen: false, ssh: false, wayland: false, x11: false,
                             wsl: true, os: Os::Linux, present: |b| b == "win32yank.exe" };
        assert_eq!(resolve_provider(&yank, crate::config::ClipboardProvider::Auto).layer1, Layer1Choice::WinYank);
        let clip = ClipEnv { present: |_| false, ..yank };
        assert_eq!(resolve_provider(&clip, crate::config::ClipboardProvider::Auto).layer1, Layer1Choice::ClipExe);
    }

    #[test]
    fn resolve_auto_screen_wraps_screen() {
        let env = ClipEnv { tmux: false, screen: true, ssh: false, wayland: false, x11: false,
                            wsl: false, os: Os::Linux, present: |_| false };
        assert_eq!(resolve_provider(&env, crate::config::ClipboardProvider::Auto).osc52, Some(Osc52Wrap::Screen));
    }

    #[test]
    fn base64_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"hi"), "aGk=");
    }

    #[test]
    fn osc52_bare_frames_with_st() {
        // "hi" → base64 "aGk="
        assert_eq!(osc52_set("hi", Osc52Wrap::Bare).unwrap(), b"\x1b]52;c;aGk=\x1b\\".to_vec());
    }

    #[test]
    fn osc52_tmux_wraps_and_doubles_inner_esc() {
        // tmux DCS passthrough: ESC P tmux; <inner with every 0x1b doubled> ESC \
        let got = osc52_set("hi", Osc52Wrap::Tmux).unwrap();
        assert_eq!(got, b"\x1bPtmux;\x1b\x1b]52;c;aGk=\x1b\x1b\\\x1b\\".to_vec());
    }

    #[test]
    fn osc52_screen_wraps_without_doubling() {
        // screen DCS passthrough: ESC P <inner> ESC \  (no ESC-doubling)
        let got = osc52_set("hi", Osc52Wrap::Screen).unwrap();
        assert_eq!(got, b"\x1bP\x1b]52;c;aGk=\x1b\\\x1b\\".to_vec());
    }

    #[test]
    fn osc52_oversize_returns_none_for_every_wrap() {
        let big = "a".repeat(OSC52_MAX_ENCODED); // base64 grows it beyond the cap
        assert!(osc52_set(&big, Osc52Wrap::Bare).is_none());
        assert!(osc52_set(&big, Osc52Wrap::Tmux).is_none());
        assert!(osc52_set(&big, Osc52Wrap::Screen).is_none());
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
