//! System-clipboard sync around the in-process Register. The Register is always
//! source of truth; everything here is best-effort and must never block the loop.

use std::io::Write as _;
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
    SelectProvider(Layer1Choice),
    Shutdown,
}

static PASTE_SEQ: AtomicU64 = AtomicU64::new(1);

pub fn next_paste_id() -> u64 {
    PASTE_SEQ.fetch_add(1, Ordering::Relaxed)
}

/// Called by run() after reduce, before the frame draw. `env` is the process clipboard
/// environment (cached at startup); `plan` is the cached resolved plan, recomputed here ONLY
/// on a provider change (the env snapshot is immutable, so the plan is otherwise stable —
/// this keeps the PATH-probing resolve off the per-keystroke hot path). Sends a provider
/// rebuild BEFORE any queued Set/Get so a same-frame provider change takes effect
/// immediately; emits OSC 52 only when the resolved plan calls for it, wrapped for the
/// environment. Never blocks (unbounded channel).
pub fn drain_clipboard_intents(
    editor: &mut crate::editor::Editor,
    env: &ClipEnv,
    plan: &mut ProviderPlan,
    out: &mut impl std::io::Write,
    clip_tx: &Sender<ClipReq>,
    msg_tx: &Sender<crate::app::Msg>,
) {
    // Recompute the plan ONLY on a provider change — the env snapshot is immutable, so the
    // plan is otherwise stable. This keeps resolve_provider (which PATH-probes for helper
    // binaries under Auto) off the per-keystroke hot path.
    if editor.clipboard_provider_dirty {
        *plan = resolve_provider(env, editor.clipboard_provider);
        if clip_tx.send(ClipReq::SelectProvider(plan.layer1)).is_err() {
            editor.set_status_full(crate::status::StatusKind::Warning, "clipboard unavailable".to_string(),
                crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
        }
        editor.clear_clipboard_provider_dirty();
    }
    if let Some(text) = editor.clipboard_sync_request.take() {
        if let Some(wrap) = plan.osc52 {
            if let Some(bytes) = osc52_set(&text, wrap) {
                let _ = out.write_all(&bytes);
                let _ = out.flush();
            }
        }
        if clip_tx.send(ClipReq::Set(text)).is_err() {
            editor.set_status_full(crate::status::StatusKind::Warning, "clipboard unavailable".to_string(),
                crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
        }
    }
    if let Some(pi) = editor.clipboard_get_pending.take() {
        if clip_tx.send(ClipReq::Get { id: pi.id, buffer_id: pi.buffer_id }).is_err() {
            // No worker (tests / shutdown): notify, then fall back to the register paste path.
            editor.set_status_full(crate::status::StatusKind::Warning, "clipboard unavailable".to_string(),
                crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
            let _ = msg_tx.send(crate::app::Msg::ClipboardPaste {
                id: pi.id,
                buffer_id: pi.buffer_id,
                text: None,
            });
        }
    }
}

/// Spawn the long-lived clipboard worker with an initial resolved plan. The Layer-1
/// backend is built from `initial.layer1`; availability reflects the whole plan
/// (`layer1 != Null || osc52.is_some()`), so a plain-SSH plan (Null + OSC 52) reports
/// available. `SelectProvider` rebuilds the backend live on a runtime provider change.
pub fn spawn_worker(msg_tx: Sender<crate::app::Msg>, initial: ProviderPlan) -> Sender<ClipReq> {
    let (tx, rx) = std::sync::mpsc::channel::<ClipReq>();
    std::thread::Builder::new()
        .name("wcartel-clipboard".into())
        .spawn(move || {
            let available = initial.layer1 != Layer1Choice::Null || initial.osc52.is_some();
            let _ = msg_tx.send(crate::app::Msg::ClipboardAvailability(available));
            let mut backend: Box<dyn ClipboardBackend> = backend_for(initial.layer1);
            while let Ok(req) = rx.recv() {
                match req {
                    ClipReq::Set(s) => backend.set(&s),
                    ClipReq::Get { id, buffer_id } => {
                        let text = backend.get().filter(|s| !s.is_empty());
                        let _ = msg_tx.send(crate::app::Msg::ClipboardPaste { id, buffer_id, text });
                    }
                    ClipReq::SelectProvider(choice) => { backend = backend_for(choice); }
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

/// A Layer-1 backend that shells out to an external clipboard helper. `set` writes the
/// text to the child's stdin and reaps it (helpers self-background, so the foreground
/// child exits promptly). `get` reads stdout; `None` get_argv means set-only (e.g. clip.exe),
/// so paste falls back to the register.
pub struct CommandBackend {
    set_argv: Vec<String>,
    get_argv: Option<Vec<String>>,
}

impl CommandBackend {
    pub fn wl_copy() -> Self {
        CommandBackend { set_argv: vec!["wl-copy".into()],
                         get_argv: Some(vec!["wl-paste".into(), "--no-newline".into()]) }
    }
    pub fn xclip() -> Self {
        CommandBackend { set_argv: vec!["xclip".into(), "-selection".into(), "clipboard".into()],
                         get_argv: Some(vec!["xclip".into(), "-selection".into(), "clipboard".into(), "-o".into()]) }
    }
    pub fn xsel() -> Self {
        CommandBackend { set_argv: vec!["xsel".into(), "-b".into(), "-i".into()],
                         get_argv: Some(vec!["xsel".into(), "-b".into(), "-o".into()]) }
    }
    pub fn win_yank() -> Self {
        CommandBackend { set_argv: vec!["win32yank.exe".into(), "-i".into(), "--crlf".into()],
                         get_argv: Some(vec!["win32yank.exe".into(), "-o".into(), "--lf".into()]) }
    }
    pub fn clip_exe() -> Self {
        CommandBackend { set_argv: vec!["clip.exe".into()], get_argv: None }
    }
}

impl ClipboardBackend for CommandBackend {
    fn set(&mut self, text: &str) {
        use std::process::{Command, Stdio};
        let Some((bin, args)) = self.set_argv.split_first() else { return };
        let child = Command::new(bin).args(args)
            .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
        if let Ok(mut ch) = child {
            if let Some(mut stdin) = ch.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
                // drop stdin → EOF so the helper commits and its foreground child exits.
            }
            let _ = ch.wait(); // reap the (promptly-exiting) foreground child.
        }
    }
    fn get(&mut self) -> Option<String> {
        use std::process::{Command, Stdio};
        let argv = self.get_argv.as_ref()?;
        let (bin, args) = argv.split_first()?;
        let out = Command::new(bin).args(args)
            .stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
        if !out.status.success() { return None; }
        let s = String::from_utf8_lossy(&out.stdout).into_owned();
        if s.is_empty() { None } else { Some(s) }
    }
}

/// Map a resolved Layer-1 choice to a boxed backend. `Arboard`/`Null` reuse the existing
/// backends; helpers use `CommandBackend`. arboard init failure degrades to `NullBackend`.
pub fn backend_for(choice: Layer1Choice) -> Box<dyn ClipboardBackend> {
    match choice {
        Layer1Choice::WlCopy  => Box::new(CommandBackend::wl_copy()),
        Layer1Choice::Xclip   => Box::new(CommandBackend::xclip()),
        Layer1Choice::Xsel    => Box::new(CommandBackend::xsel()),
        Layer1Choice::WinYank => Box::new(CommandBackend::win_yank()),
        Layer1Choice::ClipExe => Box::new(CommandBackend::clip_exe()),
        Layer1Choice::Arboard => match ArboardBackend::try_new() {
            Some(b) => Box::new(b),
            None => Box::new(NullBackend),
        },
        Layer1Choice::Null => Box::new(NullBackend),
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

    // A NeedsOsc52 environment (plain SSH, no display) → plan.osc52 == Some(Bare). Keeps the
    // existing "copy emits bare OSC 52" assertion valid under the new plan-gated emission.
    fn bare_env() -> ClipEnv {
        ClipEnv { tmux: false, screen: false, ssh: true, wayland: false, x11: false,
                  wsl: false, os: Os::Linux, present: |_| false }
    }

    #[test]
    fn drain_emits_wrapped_osc52_when_plan_says_so() {
        // Env: tmux + Null layer1 → plan.osc52 == Some(Tmux). A copy emits the tmux-wrapped bytes.
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (80, 24));
        e.clipboard_provider = crate::config::ClipboardProvider::Osc52; // forces Null + wrap
        e.clipboard_sync_request = Some("hi".into());
        let env = ClipEnv { tmux: true, screen: false, ssh: false, wayland: false, x11: false,
                            wsl: false, os: Os::Linux, present: |_| false };
        let (clip_tx, _clip_rx) = std::sync::mpsc::channel();
        let (msg_tx, _msg_rx) = std::sync::mpsc::channel();
        let mut out: Vec<u8> = Vec::new();
        let mut plan = resolve_provider(&env, e.clipboard_provider);
        drain_clipboard_intents(&mut e, &env, &mut plan, &mut out, &clip_tx, &msg_tx);
        assert_eq!(out, b"\x1bPtmux;\x1b\x1b]52;c;aGk=\x1b\x1b\\\x1b\\".to_vec());
    }

    #[test]
    fn drain_suppresses_osc52_when_plan_none() {
        // Native forces layer1 Arboard, osc52 None → no terminal write.
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (80, 24));
        e.clipboard_provider = crate::config::ClipboardProvider::Native;
        e.clipboard_sync_request = Some("hi".into());
        let env = ClipEnv { tmux: false, screen: false, ssh: false, wayland: false, x11: true,
                            wsl: false, os: Os::Linux, present: |_| false };
        let (clip_tx, _clip_rx) = std::sync::mpsc::channel();
        let (msg_tx, _msg_rx) = std::sync::mpsc::channel();
        let mut out: Vec<u8> = Vec::new();
        let mut plan = resolve_provider(&env, e.clipboard_provider);
        drain_clipboard_intents(&mut e, &env, &mut plan, &mut out, &clip_tx, &msg_tx);
        assert!(out.is_empty(), "osc52 suppressed → nothing written to the terminal");
    }

    #[test]
    fn drain_sends_select_provider_before_set_when_dirty() {
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (80, 24));
        e.clipboard_provider = crate::config::ClipboardProvider::Native;
        e.clipboard_provider_dirty = true;
        e.clipboard_sync_request = Some("hi".into());
        let env = ClipEnv { tmux: false, screen: false, ssh: false, wayland: false, x11: true,
                            wsl: false, os: Os::Linux, present: |_| false };
        let (clip_tx, clip_rx) = std::sync::mpsc::channel();
        let (msg_tx, _msg_rx) = std::sync::mpsc::channel();
        let mut out: Vec<u8> = Vec::new();
        let mut plan = resolve_provider(&env, e.clipboard_provider);
        drain_clipboard_intents(&mut e, &env, &mut plan, &mut out, &clip_tx, &msg_tx);
        // First message is the provider rebuild, THEN the Set.
        match clip_rx.recv().unwrap() { ClipReq::SelectProvider(_) => {}, o => panic!("want SelectProvider first, got {o:?}") }
        match clip_rx.recv().unwrap() { ClipReq::Set(s) => assert_eq!(s, "hi"), o => panic!("want Set, got {o:?}") }
        assert!(!e.clipboard_provider_dirty, "dirty cleared after send");
    }

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
        let mut plan = resolve_provider(&bare_env(), e.clipboard_provider);
        drain_clipboard_intents(&mut e, &bare_env(), &mut plan, &mut out, &clip_tx, &msg_tx);
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
        let mut plan = resolve_provider(&bare_env(), e.clipboard_provider);
        drain_clipboard_intents(&mut e, &bare_env(), &mut plan, &mut out, &clip_tx, &msg_tx);
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
        let mut plan = resolve_provider(&bare_env(), e.clipboard_provider);
        drain_clipboard_intents(&mut e, &bare_env(), &mut plan, &mut out, &clip_tx, &msg_tx);
        match msg_rx.try_recv() {
            Ok(crate::app::Msg::ClipboardPaste { text: None, buffer_id, .. }) => assert_eq!(buffer_id, bid),
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn command_backend_argv_constructors() {
        assert_eq!(CommandBackend::wl_copy().set_argv, vec!["wl-copy".to_string()]);
        assert_eq!(CommandBackend::wl_copy().get_argv,
                   Some(vec!["wl-paste".to_string(), "--no-newline".to_string()]));
        assert_eq!(CommandBackend::xclip().set_argv,
                   ["xclip", "-selection", "clipboard"].iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(CommandBackend::xsel().set_argv,
                   ["xsel", "-b", "-i"].iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(CommandBackend::win_yank().get_argv,
                   Some(vec!["win32yank.exe".to_string(), "-o".to_string(), "--lf".to_string()]));
        assert!(CommandBackend::clip_exe().get_argv.is_none()); // set-only
    }

    #[test]
    fn backend_for_maps_choices() {
        // Smoke: mapping does not panic and Null yields an inert backend.
        let mut null = backend_for(Layer1Choice::Null);
        null.set("x");
        assert_eq!(null.get(), None);
    }

    #[test]
    fn command_backend_roundtrips_via_cat_like_helper() {
        // Use a POSIX shell to emulate a clipboard: set writes to a temp file, get reads it.
        // Skips cleanly where /bin/sh is unavailable (non-unix CI).
        if !std::path::Path::new("/bin/sh").exists() { return; }
        let dir = std::env::temp_dir().join(format!("wcartel-clip-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let slot = dir.join("slot");
        let slot_s = slot.to_string_lossy().to_string();
        let mut b = CommandBackend {
            set_argv: vec!["/bin/sh".into(), "-c".into(), format!("cat > {slot_s}")],
            get_argv: Some(vec!["/bin/sh".into(), "-c".into(), format!("cat {slot_s}")]),
        };
        b.set("hello");
        assert_eq!(b.get().as_deref(), Some("hello"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_dead_clipboard_worker_sets_the_status_notice() {
        let (clip_tx, clip_rx) = std::sync::mpsc::channel::<ClipReq>();
        drop(clip_rx); // worker is gone: every send now errors
        let (msg_tx, _msg_rx) = std::sync::mpsc::channel::<crate::app::Msg>();
        let mut out: Vec<u8> = Vec::new();
        let mut ed = crate::editor::Editor::new_from_text("x\n", None, (80, 24));
        let bid = ed.active().id;
        let mut plan = resolve_provider(&bare_env(), ed.clipboard_provider);

        // A pending Get with no worker -> notice + None fallback.
        ed.clipboard_get_pending = Some(PasteIntent { id: 1, buffer_id: bid });
        drain_clipboard_intents(&mut ed, &bare_env(), &mut plan, &mut out, &clip_tx, &msg_tx);
        assert_eq!(ed.status_text(), "clipboard unavailable");
        // A17 T5 (F4 Warning table): a Sticky Warning — `clear_transient_status` is a no-op
        // below, so dismiss it explicitly before the next case.
        assert_eq!(ed.status().unwrap().kind(), crate::status::StatusKind::Warning);
        assert_eq!(ed.status().unwrap().lifetime(), crate::status::StatusLifetime::Sticky);
        ed.dismiss_status();

        // A pending Set with no worker -> notice.
        ed.clipboard_sync_request = Some("hello".to_string());
        drain_clipboard_intents(&mut ed, &bare_env(), &mut plan, &mut out, &clip_tx, &msg_tx);
        assert_eq!(ed.status_text(), "clipboard unavailable");
        assert_eq!(ed.status().unwrap().kind(), crate::status::StatusKind::Warning);
        assert_eq!(ed.status().unwrap().lifetime(), crate::status::StatusLifetime::Sticky);
    }

    /// A17 T5 (F4 Warning table): the third `clipboard unavailable` site — a provider-rebuild
    /// send failure on a dirty provider change — is also a Sticky Warning.
    #[test]
    fn a_dead_clipboard_worker_provider_rebuild_sets_the_status_notice() {
        let (clip_tx, clip_rx) = std::sync::mpsc::channel::<ClipReq>();
        drop(clip_rx); // worker is gone: every send now errors
        let (msg_tx, _msg_rx) = std::sync::mpsc::channel::<crate::app::Msg>();
        let mut out: Vec<u8> = Vec::new();
        let mut ed = crate::editor::Editor::new_from_text("x\n", None, (80, 24));
        let mut plan = resolve_provider(&bare_env(), ed.clipboard_provider);
        ed.set_clipboard_provider(ed.clipboard_provider); // arms clipboard_provider_dirty
        drain_clipboard_intents(&mut ed, &bare_env(), &mut plan, &mut out, &clip_tx, &msg_tx);
        assert_eq!(ed.status_text(), "clipboard unavailable");
        assert_eq!(ed.status().unwrap().kind(), crate::status::StatusKind::Warning);
        assert_eq!(ed.status().unwrap().lifetime(), crate::status::StatusLifetime::Sticky);
    }

    #[test]
    fn spawn_worker_reports_available_for_null_plus_osc52() {
        // plain-SSH plan: Null layer1 but OSC 52 enabled → available == true.
        let (tx, rx) = std::sync::mpsc::channel::<crate::app::Msg>();
        let plan = ProviderPlan { layer1: Layer1Choice::Null, osc52: Some(Osc52Wrap::Bare) };
        let clip = spawn_worker(tx, plan);
        match rx.recv().expect("availability msg") {
            crate::app::Msg::ClipboardAvailability(a) => assert!(a, "Null+OSC52 is available"),
            other => panic!("expected availability, got {other:?}"),
        }
        let _ = clip.send(ClipReq::Shutdown);
    }

    #[test]
    fn spawn_worker_reports_unavailable_for_null_no_osc52() {
        let (tx, rx) = std::sync::mpsc::channel::<crate::app::Msg>();
        let plan = ProviderPlan { layer1: Layer1Choice::Null, osc52: None };
        let clip = spawn_worker(tx, plan);
        match rx.recv().expect("availability msg") {
            crate::app::Msg::ClipboardAvailability(a) => assert!(!a, "Null+no-OSC52 is unavailable"),
            other => panic!("expected availability, got {other:?}"),
        }
        let _ = clip.send(ClipReq::Shutdown);
    }

    #[test]
    fn select_provider_is_consumed_and_worker_keeps_serving() {
        // Loop-continuity coverage: after a SelectProvider, the worker still answers a Get (a panicking
        // arm or a broken loop would hang or fail this). This does NOT by itself prove the backend value
        // swapped — that is unit-covered by `backend_for_maps_choices` (Task 3), which needs no worker.
        let (tx, rx) = std::sync::mpsc::channel::<crate::app::Msg>();
        let clip = spawn_worker(tx, ProviderPlan { layer1: Layer1Choice::Null, osc52: None });
        let _ = rx.recv(); // availability
        clip.send(ClipReq::SelectProvider(Layer1Choice::Arboard)).unwrap();
        clip.send(ClipReq::Get { id: 7, buffer_id: crate::editor::BufferId(0) }).unwrap();
        match rx.recv().expect("paste msg after rebuild") {
            crate::app::Msg::ClipboardPaste { id, .. } => assert_eq!(id, 7, "worker still serves post-rebuild"),
            other => panic!("expected paste, got {other:?}"),
        }
        let _ = clip.send(ClipReq::Shutdown);
    }
}
