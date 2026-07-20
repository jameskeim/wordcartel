//! Layered TOML config + CLI parsing. Built-in defaults < XDG < project-local < --config.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use serde::Deserialize;

#[derive(Debug, Default, Clone)]
pub struct Cli {
    pub path: Option<PathBuf>,
    pub config_path: Option<PathBuf>,
    pub no_config: bool,
    /// `-V` / `--version` was passed. The caller (`main`) prints the version and exits 0;
    /// the parser only records the request so it stays pure and testable.
    pub version: bool,
    /// `--no-splash` was passed: suppress the startup splash for THIS launch only
    /// (the persistent opt-out is `view.splash`; the flag never writes config).
    pub no_splash: bool,
    /// `--no-plugins` was passed: force the plugin host off for THIS session only,
    /// regardless of `[plugins] enabled` (a safe-mode escape hatch; never writes config).
    pub no_plugins: bool,
}

/// Hand-rolled (no clap dep): `[--version|-V] [--config <path>] [--no-config] [--no-splash]
/// [--no-plugins] [file]`.
pub fn parse_cli<I: IntoIterator<Item = String>>(args: I) -> Cli {
    let mut cli = Cli::default();
    let mut it = args.into_iter();
    let _ = it.next(); // argv[0]
    while let Some(a) = it.next() {
        match a.as_str() {
            "--version" | "-V" => cli.version = true,
            "--no-config" => cli.no_config = true,
            "--no-splash" => cli.no_splash = true,
            "--no-plugins" => cli.no_plugins = true,
            "--config" => cli.config_path = it.next().map(PathBuf::from),
            _ => {
                if cli.path.is_none() {
                    cli.path = Some(PathBuf::from(a));
                }
            }
        }
    }
    cli
}

// --- Resolved (folded) config the rest of the app consumes ---
#[derive(Debug, Default, Clone)]
pub struct Config {
    pub keymap: KeymapConfig,
    pub state: StateConfig,
    pub mouse: MouseConfig,
    pub view: ViewConfig,
    pub diagnostics: DiagnosticsConfig,
    pub theme: ThemeConfig,
    pub export: ExportConfig,
    pub menu: MenuConfig,
    pub clipboard: ClipboardConfig,
    pub plugins: PluginsConfig,
    pub files: FilesConfig,
}

#[derive(Debug, Default, Clone)]
pub struct ThemeConfig {
    pub name: Option<String>,
    pub file: Option<PathBuf>,           // ~-expanded, resolved relative to declaring config
    pub depth: Option<String>,           // "truecolor"|"256"|"16"|"none"
    pub chrome: Option<String>,          // "full"|"zen" — parsed at resolve
    pub canvas: Option<String>,          // "opaque"|"transparent" — parsed at resolve
    pub heading_level_glyph: Option<bool>,
    pub styles: BTreeMap<String, RawFace>,
}

#[derive(Debug, Clone)]
pub struct DiagnosticsConfig {
    pub enabled: bool,
    pub grammar: bool,
    pub debounce_ms: u64,
    /// Personal dictionary path — one word per line. Dual role (Effort A): the client-side
    /// suppression seed (loaded into `editor.dictionary` at startup and unioned into the ignore
    /// filter so a user's saved words are never re-flagged) AND harper-ls's `userDictPath` (the
    /// SAME file, same format). `append_word_to_dict` is the sole writer; harper is nudged to
    /// re-read it, never to append. `None` → harper falls back to its own default path.
    pub dictionary: Option<std::path::PathBuf>,
    pub linters: Option<Vec<String>>,
}
impl Default for DiagnosticsConfig {
    fn default() -> Self {
        // Fix A7: resolve a sensible default dictionary path (<config_dir>/wordcartel/dictionary.txt)
        // so add-to-dictionary works out of the box without explicit configuration.
        let dictionary = dirs::config_dir().map(|d| d.join("wordcartel").join("dictionary.txt"));
        DiagnosticsConfig { enabled: true, grammar: true, debounce_ms: 400, dictionary, linters: None }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusGranularity { Paragraph, Sentence }

/// Menu bar visibility mode (`[menu] bar`). Auto reveals on a pointer dwell at
/// the top row and hides after a leave-grace; Pinned keeps the bar always
/// visible-inactive; Hidden shows it only while the dropdown is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuBarMode { Hidden, Auto, Pinned }

/// Reveal policy for a transient chrome element (status line, scrollbar; the menu
/// bar keeps its own `MenuBarMode` mapped onto this). `Off` = never shown, `On` =
/// always shown, `Auto` = revealed on pointer dwell near the element plus a
/// context trigger (scroll activity / a status message), hidden after a leave grace.
///
/// The status line has no true `Off`: a message force-reveals it even under `Auto`
/// (no-silent-UI). Only the scrollbar uses `Off`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransientMode { Off, Auto, On }

/// Writing-caret shape. `Default` = never emit DECSCUSR (terminal's own shape) — the shipped
/// default. The three concrete shapes map to DECSCUSR when composed with blink (see cursor_style).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CaretShape { #[default] Default, Block, Beam, Underline }

/// Clipboard provider selection (`[clipboard] provider`). `Auto` runs the detection
/// chain; `Native` forces arboard; `Osc52` forces the terminal path; `Off` disables
/// the system clipboard (in-process register only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardProvider { Auto, Native, Osc52, Off }

/// Which file types the picker lists. Two-state rather than a bool so it carries NAMED
/// states for the `MenuMark::Value` representative and the two set-per-state commands
/// (command-surface contract, law 8).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FileTypeFilter {
    /// What `file::open` can actually open, plus output siblings in destination mode.
    #[default]
    Documents,
    All,
}

/// Menu bar configuration section.
#[derive(Debug, Clone)]
pub struct MenuConfig { pub bar: MenuBarMode }
impl Default for MenuConfig {
    fn default() -> Self { MenuConfig { bar: MenuBarMode::Auto } }
}

/// File-browser filter configuration section (`[files]`). Persisted (command-surface
/// contract decision 8 — "show all files" must stick): `snapshot_of` seeds the two
/// `Editor` fields from here at startup; `set_show_clutter`/`set_file_type_filter` are
/// the sole runtime mutators.
#[derive(Debug, Clone)]
pub struct FilesConfig {
    pub show_clutter: bool,
    pub type_filter: FileTypeFilter,
}
impl Default for FilesConfig {
    fn default() -> Self {
        FilesConfig { show_clutter: false, type_filter: FileTypeFilter::Documents }
    }
}

/// Clipboard configuration section (`[clipboard]`).
#[derive(Debug, Clone)]
pub struct ClipboardConfig { pub provider: ClipboardProvider }
impl Default for ClipboardConfig {
    fn default() -> Self { ClipboardConfig { provider: ClipboardProvider::Auto } }
}

/// Plugin host configuration section (`[plugins]`, P1 spec §4 + P2 Task 5). `enabled = false`
/// (or the session-only `--no-plugins` CLI flag, which forces this off regardless) skips the
/// whole plugin-load phase — no VM, no `discover`, no `wc.*` surface. `disable` names stems
/// (file/dir names under the plugins dir, no `.lua`) to skip during `discover` without
/// removing the file — distinct from a plugin that fails to load (which is reported, not
/// silently excluded). `dir` overrides the default `<config_dir>/wordcartel/plugins` scan
/// root. `config` holds each plugin's `[plugins.config.<name>]` TOML subtable, keyed by stem
/// — namespaced (not flattened) so a plugin named `enabled`/`disable`/`dir` is still valid.
#[derive(Debug, Clone)]
pub struct PluginsConfig {
    pub enabled: bool,
    pub disable: Vec<String>,
    pub dir: Option<PathBuf>,
    pub config: BTreeMap<String, toml::Value>,
}
impl Default for PluginsConfig {
    fn default() -> Self {
        PluginsConfig { enabled: true, disable: Vec::new(), dir: None, config: BTreeMap::new() }
    }
}

#[derive(Debug, Clone)]
pub struct ViewConfig {
    pub typewriter: bool,
    pub typewriter_anchor: f32,
    pub focus: bool,
    pub focus_granularity: FocusGranularity,
    pub measure: bool,
    pub wrap_column: u16,
    pub wrap_guide: bool,
    pub word_count: bool,
    pub scrollbar: TransientMode,
    pub status_line: TransientMode,
    /// Startup splash / welcome screen (`[view] splash`). On by default; the splash is
    /// painted over the first frame and dismissed by the first key press or mouse click.
    pub splash: bool,
    pub caret_shape: CaretShape,
    pub caret_blink: bool,
    /// Message verbosity floor (Q6, A17 T10): `[view] messages_min_kind`, `"info"` (default,
    /// show Info & above) or `"warning"` (Warnings & Errors only). Feeds `Editor::set_messages_min_kind`.
    pub messages_min_kind: crate::status::StatusKind,
}
impl Default for ViewConfig {
    fn default() -> Self {
        ViewConfig { typewriter: false, typewriter_anchor: 0.5, focus: false,
            focus_granularity: FocusGranularity::Paragraph, measure: false,
            wrap_column: 72, wrap_guide: false, word_count: false,
            // status_line defaults On (idle info line always shown out of the box —
            // preserves the pre-density behavior); Zen (chrome = zen) flips it to Auto.
            scrollbar: TransientMode::Auto, status_line: TransientMode::On, splash: true,
            caret_shape: CaretShape::Default, caret_blink: true,
            messages_min_kind: crate::status::StatusKind::Info }
    }
}

#[derive(Debug, Clone)]
pub struct ExportConfig {
    /// Pandoc PDF engine (`--pdf-engine=…`). Default xelatex (deliberate; see the spec).
    pub pdf_engine: String,
    /// Export-time smart punctuation. true → `-f markdown` (pandoc's smart default);
    /// false → `-f markdown-smart` (strict literal). Applies to all export formats.
    pub typography: bool,
}
impl Default for ExportConfig {
    fn default() -> Self {
        ExportConfig { pdf_engine: "xelatex".into(), typography: true }
    }
}

#[derive(Debug, Clone)]
pub struct MouseConfig {
    pub mouse_capture: bool,
}
impl Default for MouseConfig {
    fn default() -> Self {
        MouseConfig { mouse_capture: true }
    }
}

#[derive(Debug, Clone)]
pub struct KeymapConfig {
    pub preset: String,
    pub patches: Vec<KeymapPatch>,
}
impl Default for KeymapConfig {
    fn default() -> Self {
        KeymapConfig {
            preset: "cua".into(),
            patches: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct KeymapPatch {
    pub bind: BTreeMap<String, String>,
    pub unbind: Vec<String>,
    pub cua: Option<ScopedPatch>,
    pub wordstar: Option<ScopedPatch>,
}

/// Per-preset binding overrides, captured from a `[keymap.<preset>]` sub-table.
/// Within a patch layer, these are applied AFTER the layer's global `bind`/`unbind`,
/// so a scoped entry beats a global entry in the same layer ("specific wins").
#[derive(Debug, Clone, Default)]
pub struct ScopedPatch {
    pub bind: BTreeMap<String, String>,
    pub unbind: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct StateConfig {
    pub resume: bool,
    pub max_entries: usize,
}
impl Default for StateConfig {
    fn default() -> Self {
        StateConfig {
            resume: true,
            max_entries: 200,
        }
    }
}

// --- Raw per-layer deserialize: every field optional so an OMITTED key inherits
//     the lower layer rather than resetting it to a default (Codex plan-review fix) ---
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawConfig {
    keymap: RawKeymap,
    state: RawState,
    mouse: RawMouse,
    view: RawView,
    diagnostics: RawDiagnostics,
    theme: RawTheme,
    export: RawExport,
    menu: RawMenu,
    clipboard: RawClipboard,
    plugins: RawPlugins,
    files: RawFiles,
}

#[derive(Debug, Default, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct RawFace {
    pub fg: Option<String>,
    pub bg: Option<String>,
    pub underline_color: Option<String>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub strike: Option<bool>,
    pub reverse: Option<bool>,
    pub dim: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawTheme {
    name: Option<String>,
    file: Option<String>,
    depth: Option<String>,
    chrome: Option<String>,
    canvas: Option<String>,
    heading_level_glyph: Option<bool>,
    styles: BTreeMap<String, RawFace>,
}
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawDiagnostics {
    enabled: Option<bool>,
    grammar: Option<bool>,
    debounce_ms: Option<u64>,
    dictionary: Option<String>,
    linters: Option<Vec<String>>,
    harper: RawHarperEngine,
}
/// `[diagnostics.harper]` — the per-engine table for the harper linter (SPINE Task 8, spec §9.1).
/// `grammar` here is a config-file *spelling* of the already-global `diagnostics.grammar` option
/// (folded on TOP of it, so `[diagnostics.harper].grammar` wins when set) — not a distinct
/// setting, so no separate command surface obligation (§12).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawHarperEngine {
    grammar: Option<bool>,
}
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawKeymap {
    preset: Option<String>,
    bind: BTreeMap<String, String>,
    unbind: Vec<String>,
    /// NOTE: once a [keymap.cua] header appears, [keymap]-intended keys typed below it
    /// silently belong to the scoped table (TOML section semantics).
    cua: Option<RawScoped>,
    /// NOTE: once a [keymap.wordstar] header appears, [keymap]-intended keys typed below it
    /// silently belong to the scoped table (TOML section semantics).
    wordstar: Option<RawScoped>,
}
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawScoped {
    bind: BTreeMap<String, String>,
    unbind: Vec<String>,
}
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawState {
    resume: Option<bool>,
    max_entries: Option<usize>,
}
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawMouse {
    capture: Option<bool>,
}
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawExport {
    pdf_engine: Option<String>,
    typography: Option<bool>,
}
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawMenu {
    bar: Option<String>,
}
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawClipboard {
    provider: Option<String>,
}
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawFiles {
    show_clutter: Option<bool>,
    type_filter: Option<String>,
}
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawPlugins {
    enabled: Option<bool>,
    disable: Option<Vec<String>>,
    /// `[plugins] dir` — overrides the default `<config_dir>/wordcartel/plugins` scan root.
    dir: Option<PathBuf>,
    /// `[plugins.config.<name>]` subtables — namespaced under `config` (NOT `#[serde(flatten)]`)
    /// so a plugin literally named `enabled`/`disable`/`dir` cannot collide with a typed field.
    config: BTreeMap<String, toml::Value>,
}
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawView {
    typewriter: Option<bool>,
    typewriter_anchor: Option<f32>,
    focus: Option<bool>,
    focus_granularity: Option<String>,
    measure: Option<bool>,
    wrap_column: Option<u16>,
    wrap_guide: Option<bool>,
    word_count: Option<bool>,
    scrollbar: Option<String>,
    status_line: Option<String>,
    splash: Option<bool>,
    caret_shape: Option<String>,
    caret_blink: Option<bool>,
    messages_min_kind: Option<String>,
}

/// Ordered existing config files, lowest→highest precedence. Empty when --no-config.
pub fn config_layer_paths(
    cli: &Cli,
    xdg_config_dir: Option<&Path>,
    anchor_dir: &Path,
) -> Vec<PathBuf> {
    // fs-chokepoint-allow: (w) the `RealFs` wrapper itself — its `*_with_fs` seam is what injected callers use
    config_layer_paths_with_fs(&crate::fsx::RealFs, cli, xdg_config_dir, anchor_dir)
}

/// Seam-taking core of [`config_layer_paths`]. Kept `pub(crate)` so tests can inject a `FaultFs`.
pub(crate) fn config_layer_paths_with_fs(
    fs: &dyn crate::fsx::Fs,
    cli: &Cli,
    xdg_config_dir: Option<&Path>,
    anchor_dir: &Path,
) -> Vec<PathBuf> {
    if cli.no_config {
        return Vec::new();
    }
    let mut v = Vec::new();
    if let Some(x) = xdg_config_dir {
        let p = x.join("wordcartel").join("config.toml");
        if crate::fsx::is_file_via(fs, &p) {
            v.push(p);
        }
    }
    // project-local: nearest .wordcartel.toml walking up from anchor_dir
    let mut dir = Some(anchor_dir);
    while let Some(d) = dir {
        let p = d.join(".wordcartel.toml");
        if crate::fsx::is_file_via(fs, &p) {
            v.push(p);
            break;
        }
        dir = d.parent();
    }
    if let Some(c) = &cli.config_path {
        if crate::fsx::is_file_via(fs, c) {
            v.push(c.clone());
        }
        // (a missing --config path is surfaced as a warning by the caller in Task 5)
    }
    v
}

/// Parse + fold layers (lowest→highest precedence) into a resolved Config.
/// PER-FIELD merge: `preset` & each `state` field override only when the layer
/// SETS them (Option); `patches` keeps one ordered entry per layer so
/// build_keymap applies them in precedence order (Codex plan-review fix).
#[allow(clippy::too_many_lines)] // config parse — one arm per config key
pub fn load(paths: &[PathBuf]) -> (Config, Vec<String>) {
    // fs-chokepoint-allow: (w) the `RealFs` wrapper itself — its `*_with_fs` seam is what injected callers use
    load_with_fs(&crate::fsx::RealFs, paths)
}

#[allow(clippy::too_many_lines)] // config parse — one arm per config key
pub(crate) fn load_with_fs(fs: &dyn crate::fsx::Fs, paths: &[PathBuf]) -> (Config, Vec<String>) {
    let mut cfg = Config::default();
    let mut warns = Vec::new();
    for p in paths {
        let bytes = match fs.read_capped(p, crate::limits::MAX_CONFIG_BYTES) {
            Ok(Some(b)) => b,
            Ok(None) => {
                warns.push(format!("config: {} is too large (> {} bytes) — ignored",
                    p.display(), crate::limits::MAX_CONFIG_BYTES));
                continue;
            }
            Err(e) => {
                warns.push(format!("config: cannot read {}: {e}", p.display()));
                continue;
            }
        };
        let text = match String::from_utf8(bytes) {
            Ok(t) => t,
            Err(_) => {
                warns.push(format!("config: {} is not valid UTF-8 — ignored", p.display()));
                continue;
            }
        };
        let raw: RawConfig = match toml::from_str(&text) {
            Ok(r) => r,
            Err(e) => {
                warns.push(format!("config: parse error in {}: {e}", p.display()));
                continue;
            }
        };
        // keymap: preset overrides only if set; each layer contributes ONE ordered patch.
        if let Some(preset) = raw.keymap.preset {
            cfg.keymap.preset = preset;
        }
        cfg.keymap.patches.push(KeymapPatch {
            bind: raw.keymap.bind,
            unbind: raw.keymap.unbind,
            cua: raw.keymap.cua.map(|s| ScopedPatch { bind: s.bind, unbind: s.unbind }),
            wordstar: raw.keymap.wordstar.map(|s| ScopedPatch { bind: s.bind, unbind: s.unbind }),
        });
        // state: per-field override (omitted field inherits the lower layer).
        if let Some(r) = raw.state.resume {
            cfg.state.resume = r;
        }
        if let Some(m) = raw.state.max_entries {
            cfg.state.max_entries = m;
        }
        // mouse: per-field override (omitted field inherits the lower layer).
        if let Some(c) = raw.mouse.capture {
            cfg.mouse.mouse_capture = c;
        }
        // view: per-field override + validation.
        if let Some(v) = raw.view.typewriter { cfg.view.typewriter = v; }
        if let Some(v) = raw.view.focus { cfg.view.focus = v; }
        if let Some(v) = raw.view.measure { cfg.view.measure = v; }
        if let Some(v) = raw.view.wrap_guide { cfg.view.wrap_guide = v; }
        if let Some(v) = raw.view.word_count { cfg.view.word_count = v; }
        if let Some(v) = raw.view.splash { cfg.view.splash = v; }
        if let Some(a) = raw.view.typewriter_anchor {
            if (0.0..=1.0).contains(&a) { cfg.view.typewriter_anchor = a; }
            else { cfg.view.typewriter_anchor = a.clamp(0.0, 1.0);
                   warns.push(format!("view.typewriter_anchor {a} out of 0.0..=1.0; clamped")); }
        }
        if let Some(c) = raw.view.wrap_column {
            if c < 20 { cfg.view.wrap_column = 20;
                        warns.push(format!("view.wrap_column {c} below min 20; clamped to 20")); }
            else if c > 9999 { cfg.view.wrap_column = 9999;
                        // repar's frozen parse ceiling (NUMERIC_PARSE_MAX) — a larger
                        // persisted value would make every transform fail cryptically.
                        warns.push(format!("view.wrap_column {c} above max 9999; clamped to 9999")); }
            else { cfg.view.wrap_column = c; }
        }
        if let Some(g) = raw.view.focus_granularity {
            match g.as_str() {
                "paragraph" => cfg.view.focus_granularity = FocusGranularity::Paragraph,
                "sentence"  => cfg.view.focus_granularity = FocusGranularity::Sentence,
                other => warns.push(format!("view.focus_granularity \"{other}\" invalid; using paragraph")),
            }
        }
        if let Some(s) = raw.view.scrollbar {
            match s.as_str() {
                "off"  => cfg.view.scrollbar = TransientMode::Off,
                "auto" => cfg.view.scrollbar = TransientMode::Auto,
                "on"   => cfg.view.scrollbar = TransientMode::On,
                other => warns.push(format!("view.scrollbar \"{other}\" invalid; using auto")),
            }
        }
        if let Some(s) = raw.view.status_line {
            match s.as_str() {
                // No true Off: reject "off" (coerce to Auto) so a message can always paint.
                "off"  => { cfg.view.status_line = TransientMode::Auto;
                            warns.push("view.status_line \"off\" not allowed (no-silent-UI); using auto".to_string()); }
                "auto" => cfg.view.status_line = TransientMode::Auto,
                "on"   => cfg.view.status_line = TransientMode::On,
                other => warns.push(format!("view.status_line \"{other}\" invalid; using auto")),
            }
        }
        if let Some(s) = raw.view.caret_shape {
            match crate::config::caret_shape_from_str(&s) {
                Some(cs) => cfg.view.caret_shape = cs,
                None => warns.push(format!("view.caret_shape \"{s}\" invalid; using default")),
            }
        }
        if let Some(b) = raw.view.caret_blink { cfg.view.caret_blink = b; }
        if let Some(s) = raw.view.messages_min_kind {
            match crate::status::StatusKind::from_str(&s) {
                Some(k @ (crate::status::StatusKind::Info | crate::status::StatusKind::Warning)) =>
                    cfg.view.messages_min_kind = k,
                _ => warns.push(format!("view.messages_min_kind \"{s}\" invalid; using info")),
            }
        }
        // menu: per-field override; enum-valued string with a warning on unknowns.
        if let Some(b) = raw.menu.bar {
            match b.as_str() {
                "hidden" => cfg.menu.bar = MenuBarMode::Hidden,
                "auto"   => cfg.menu.bar = MenuBarMode::Auto,
                "pinned" => cfg.menu.bar = MenuBarMode::Pinned,
                other => warns.push(format!("menu.bar \"{other}\" invalid; using auto")),
            }
        }
        // clipboard: per-field override; enum-valued string with a warning on unknowns.
        if let Some(p) = raw.clipboard.provider {
            match p.as_str() {
                "auto"   => cfg.clipboard.provider = ClipboardProvider::Auto,
                "native" => cfg.clipboard.provider = ClipboardProvider::Native,
                "osc52"  => cfg.clipboard.provider = ClipboardProvider::Osc52,
                "off"    => cfg.clipboard.provider = ClipboardProvider::Off,
                other => warns.push(format!("clipboard.provider \"{other}\" invalid; using auto")),
            }
        }
        // files: per-field override; enum-valued string with a warning on unknowns.
        if let Some(v) = raw.files.show_clutter { cfg.files.show_clutter = v; }
        if let Some(t) = raw.files.type_filter {
            match t.as_str() {
                "documents" => cfg.files.type_filter = FileTypeFilter::Documents,
                "all"       => cfg.files.type_filter = FileTypeFilter::All,
                other => warns.push(format!("files.type_filter \"{other}\" invalid; using documents")),
            }
        }
        // plugins: per-field override (omitted field inherits the lower layer); `disable`
        // replaces wholesale when a layer sets it (not accumulated — a higher layer's list
        // is the complete intended set, mirroring diagnostics.linters).
        if let Some(v) = raw.plugins.enabled { cfg.plugins.enabled = v; }
        if let Some(v) = raw.plugins.disable { cfg.plugins.disable = v; }
        if let Some(v) = raw.plugins.dir { cfg.plugins.dir = Some(v); }
        // per-name REPLACE per layer — a higher layer's [plugins.config.foo] wholly replaces
        // a lower layer's for "foo", leaving other names' config untouched (mirrors the
        // per-field-override discipline above, applied per plugin name instead of per field).
        for (k, v) in raw.plugins.config { cfg.plugins.config.insert(k, v); }
        // export: per-field override (omitted field inherits the lower layer).
        if let Some(v) = raw.export.pdf_engine { cfg.export.pdf_engine = v; }
        if let Some(v) = raw.export.typography { cfg.export.typography = v; }
        // diagnostics: per-field override + debounce_ms floor validation.
        if let Some(v) = raw.diagnostics.enabled { cfg.diagnostics.enabled = v; }
        if let Some(v) = raw.diagnostics.grammar { cfg.diagnostics.grammar = v; }
        // [diagnostics.harper].grammar is a per-engine spelling of the same option — overrides
        // the top-level value above when set (folded after it, on purpose).
        if let Some(v) = raw.diagnostics.harper.grammar { cfg.diagnostics.grammar = v; }
        if let Some(v) = raw.diagnostics.debounce_ms {
            if v < 100 {
                warns.push(format!("config: diagnostics.debounce_ms {v} below floor 100; clamped"));
                cfg.diagnostics.debounce_ms = 100;
            } else {
                cfg.diagnostics.debounce_ms = v;
            }
        }
        if let Some(s) = raw.diagnostics.dictionary {
            // Fix A7: expand a leading `~/` (or bare `~`) to the home directory so
            // paths like `~/foo/dict.txt` work correctly.  Without expansion a raw
            // PathBuf would write to a literal `~` directory.
            let expanded = if s == "~" {
                dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("~"))
            } else if let Some(rest) = s.strip_prefix("~/") {
                dirs::home_dir()
                    .map(|h| h.join(rest))
                    .unwrap_or_else(|| std::path::PathBuf::from(&s))
            } else {
                std::path::PathBuf::from(&s)
            };
            cfg.diagnostics.dictionary = Some(expanded);
        }
        if let Some(v) = raw.diagnostics.linters { cfg.diagnostics.linters = Some(v); }
        // unknown linter names are validated against the core catalog in
        // `diagnostics_run::install_core_providers` — warned there (SPINE Task 8), once the
        // engine catalog itself exists to validate against.

        // ---- [theme] (discriminated source; file resolved vs the declaring config) ----
        let rt = raw.theme;
        let layer_dir = p.parent().unwrap_or_else(|| std::path::Path::new("."));
        // Resolve a layer's `file` (~ expand + relative-to-this-config) if present.
        let resolved_file = rt.file.as_ref().map(|s| {
            if s == "~" {
                dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"))
            } else if let Some(rest) = s.strip_prefix("~/") {
                dirs::home_dir().map(|h| h.join(rest)).unwrap_or_else(|| PathBuf::from(s))
            } else {
                let pb = PathBuf::from(s);
                if pb.is_absolute() { pb } else { layer_dir.join(pb) }
            }
        });
        match (rt.name.clone(), resolved_file) {
            (Some(_), Some(f)) => {
                warns.push(format!(
                    "theme: both `name` and `file` set in {} — `file` wins", p.display()));
                cfg.theme.name = None;
                cfg.theme.file = Some(f);
            }
            (Some(n), None) => { cfg.theme.name = Some(n); cfg.theme.file = None; } // name clears file
            (None, Some(f)) => { cfg.theme.file = Some(f); cfg.theme.name = None; } // file clears name
            (None, None) => {} // neither set this layer → inherit accumulated
        }
        if let Some(d) = rt.depth { cfg.theme.depth = Some(d); }
        if let Some(c) = rt.chrome { cfg.theme.chrome = Some(c); }
        if let Some(c) = rt.canvas { cfg.theme.canvas = Some(c); }
        if let Some(h) = rt.heading_level_glyph { cfg.theme.heading_level_glyph = Some(h); }
        for (k, v) in rt.styles { cfg.theme.styles.insert(k, v); } // accumulate across layers
    }
    (cfg, warns)
}

/// "off"/"auto"/"on" — round-trips `TransientMode` for the overrides mirror.
pub fn transient_mode_str(m: TransientMode) -> &'static str {
    match m { TransientMode::Off => "off", TransientMode::Auto => "auto", TransientMode::On => "on" }
}

/// "info"/"warning" — round-trips the `messages_min_kind` verbosity floor for the overrides
/// mirror (mirrors `transient_mode_str`). Callers only ever pass `Info`/`Warning` (the two
/// config-valid states); `Error`/`Log` are unreachable here but matched for exhaustiveness.
pub fn messages_min_kind_str(k: crate::status::StatusKind) -> &'static str {
    match k {
        crate::status::StatusKind::Warning => "warning",
        crate::status::StatusKind::Info | crate::status::StatusKind::Error | crate::status::StatusKind::Log => "info",
    }
}

pub fn caret_shape_str(s: CaretShape) -> &'static str {
    match s { CaretShape::Default => "default", CaretShape::Block => "block",
              CaretShape::Beam => "beam", CaretShape::Underline => "underline" }
}

pub fn caret_shape_from_str(s: &str) -> Option<CaretShape> {
    match s { "default" => Some(CaretShape::Default), "block" => Some(CaretShape::Block),
              "beam" => Some(CaretShape::Beam), "underline" => Some(CaretShape::Underline),
              _ => None }
}

/// "auto"/"native"/"osc52"/"off" — round-trips `ClipboardProvider` for the overrides mirror.
pub fn clipboard_provider_str(p: ClipboardProvider) -> &'static str {
    match p {
        ClipboardProvider::Auto => "auto",
        ClipboardProvider::Native => "native",
        ClipboardProvider::Osc52 => "osc52",
        ClipboardProvider::Off => "off",
    }
}

/// "documents"/"all" — round-trips `FileTypeFilter` for the overrides mirror.
pub fn file_type_filter_str(f: FileTypeFilter) -> &'static str {
    match f { FileTypeFilter::Documents => "documents", FileTypeFilter::All => "all" }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &std::path::Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn parse_cli_separates_path_config_and_noconfig() {
        let c = parse_cli(["wcartel", "notes.md"].map(String::from));
        assert_eq!(c.path.as_deref(), Some(std::path::Path::new("notes.md")));
        assert!(c.config_path.is_none() && !c.no_config);

        let c = parse_cli(["wcartel", "--config", "my.toml", "notes.md"].map(String::from));
        assert_eq!(
            c.config_path.as_deref(),
            Some(std::path::Path::new("my.toml"))
        );
        assert_eq!(c.path.as_deref(), Some(std::path::Path::new("notes.md")));

        let c = parse_cli(["wcartel", "--no-config"].map(String::from));
        assert!(c.no_config && c.path.is_none());
    }

    #[test]
    fn later_layers_override_per_field_and_keep_ordered_patches() {
        let d = tempdir();
        // lo sets BOTH state fields + a bind; hi sets ONLY max_entries (omits resume) + preset + a bind.
        let lo = write(&d, "lo.toml", "[state]\nresume=false\nmax_entries=50\n[keymap]\npreset='cua'\nbind={ \"ctrl-a\"='move_line_start' }\n");
        let hi = write(&d, "hi.toml", "[state]\nmax_entries=99\n[keymap]\npreset='wordstar'\nbind={ \"ctrl-b\"='move_left' }\n");
        let (cfg, warns) = load(&[lo, hi]);
        assert!(warns.is_empty());
        assert_eq!(cfg.state.max_entries, 99, "hi set it → wins");
        assert!(!cfg.state.resume, "hi OMITTED resume → lo's false is preserved (NOT reset to default true)");
        assert_eq!(cfg.keymap.preset, "wordstar", "final-merged preset");
        assert_eq!(cfg.keymap.patches.len(), 2, "one ordered patch per layer");
        assert!(cfg.keymap.patches[0].bind.contains_key("ctrl-a"));
        assert!(cfg.keymap.patches[1].bind.contains_key("ctrl-b"));
    }

    #[test]
    fn defaults_when_no_layers() {
        let (cfg, warns) = load(&[]);
        assert!(warns.is_empty());
        assert!(cfg.state.resume);
        assert_eq!(cfg.state.max_entries, 200);
        assert_eq!(cfg.keymap.preset, "cua");
        assert!(cfg.keymap.patches.is_empty());
    }

    #[test]
    fn malformed_toml_warns_and_skips_layer() {
        let d = tempdir();
        let bad = write(&d, "bad.toml", "[state]\nmax_entries = = =\n");
        let (cfg, warns) = load(&[bad]);
        assert_eq!(warns.len(), 1, "one warning for the bad layer");
        assert_eq!(cfg.state.max_entries, 200, "fell back to default");
    }

    #[test]
    fn view_splash_defaults_on_and_folds_from_a_layer() {
        let (cfg, warns) = load(&[]);
        assert!(warns.is_empty());
        assert!(cfg.view.splash, "built-in default is on");
        let d = tempdir();
        let p = write(&d, "splash.toml", "[view]\nsplash = false\n");
        let (cfg, warns) = load(&[p]);
        assert!(warns.is_empty());
        assert!(!cfg.view.splash, "a layer that SETS the field overrides the default");
    }

    #[test]
    fn parse_cli_no_splash_flag() {
        let c = parse_cli(["wcartel", "--no-splash", "notes.md"].map(String::from));
        assert!(c.no_splash);
        assert_eq!(c.path.as_deref(), Some(std::path::Path::new("notes.md")));
        let c = parse_cli(["wcartel"].map(String::from));
        assert!(!c.no_splash, "defaults off");
    }

    #[test]
    fn parse_cli_no_plugins_flag() {
        let c = parse_cli(["wcartel", "--no-plugins", "notes.md"].map(String::from));
        assert!(c.no_plugins);
        assert_eq!(c.path.as_deref(), Some(std::path::Path::new("notes.md")));
        let c = parse_cli(["wcartel"].map(String::from));
        assert!(!c.no_plugins, "defaults off");
    }

    #[test]
    fn plugins_section_parses() {
        let (cfg, warns) = load(&[]);
        assert!(warns.is_empty());
        assert!(cfg.plugins.enabled, "built-in default is on");
        assert!(cfg.plugins.disable.is_empty(), "built-in default is empty");

        let d = tempdir();
        let p = write(&d, "plugins.toml", "[plugins]\nenabled = false\ndisable = [\"x\"]\n");
        let (cfg, warns) = load(&[p]);
        assert!(warns.is_empty());
        assert!(!cfg.plugins.enabled);
        assert_eq!(cfg.plugins.disable, vec!["x".to_string()]);
    }

    #[test]
    fn plugins_config_namespaced_parses() {
        let (cfg, warns) = load(&[]);
        assert!(warns.is_empty());
        assert!(cfg.plugins.config.is_empty(), "default config map is empty");

        let d = tempdir();
        let p = write(
            &d,
            "plugins-config.toml",
            "[plugins.config.wordcount]\nmin_words = 100\n[plugins.config.dir]\nfoo = \"bar\"\n",
        );
        let (cfg, warns) = load(&[p]);
        assert!(warns.is_empty());
        let wordcount = cfg.plugins.config.get("wordcount").expect("wordcount config folded in");
        assert_eq!(wordcount.get("min_words").and_then(toml::Value::as_integer), Some(100));
        // A plugin literally named "dir" is valid — its config lives under
        // [plugins.config.dir], which does NOT collide with the typed `dir` field.
        assert!(cfg.plugins.dir.is_none(), "the [plugins.config.dir] TABLE never sets the typed dir field");
        let dir_plugin = cfg.plugins.config.get("dir").expect("a plugin named 'dir' is valid");
        assert_eq!(dir_plugin.get("foo").and_then(toml::Value::as_str), Some("bar"));
    }

    #[test]
    fn plugins_dir_parses() {
        let (cfg, warns) = load(&[]);
        assert!(warns.is_empty());
        assert!(cfg.plugins.dir.is_none(), "default dir is unset");

        let d = tempdir();
        let p = write(&d, "plugins-dir.toml", "[plugins]\ndir = \"/x/y\"\n");
        let (cfg, warns) = load(&[p]);
        assert!(warns.is_empty());
        assert_eq!(cfg.plugins.dir, Some(PathBuf::from("/x/y")));
    }

    #[test]
    fn plugins_config_replaces_per_layer() {
        let d = tempdir();
        let lo = write(
            &d,
            "lo.toml",
            "[plugins.config.foo]\na = 1\n[plugins.config.bar]\nb = 2\n",
        );
        let hi = write(&d, "hi.toml", "[plugins.config.foo]\na = 99\n");
        let (cfg, warns) = load(&[lo, hi]);
        assert!(warns.is_empty());
        let foo = cfg.plugins.config.get("foo").expect("foo present");
        assert_eq!(foo.get("a").and_then(toml::Value::as_integer), Some(99), "hi wholly replaces lo's foo");
        let bar = cfg.plugins.config.get("bar").expect("bar untouched by hi");
        assert_eq!(bar.get("b").and_then(toml::Value::as_integer), Some(2));
    }

    #[test]
    fn view_config_parses_and_validates() {
        let toml = r#"
            [view]
            measure = true
            wrap_column = 5
            typewriter_anchor = 1.5
            focus_granularity = "bogus"
        "#;
        let d = tempdir();
        let path = write(&d, "view.toml", toml);
        let (cfg, warnings) = load(&[path]);
        assert!(cfg.view.measure);
        assert_eq!(cfg.view.wrap_column, 20, "wrap_column clamped to min 20");
        assert_eq!(cfg.view.typewriter_anchor, 1.0, "anchor clamped to <=1.0");
        assert_eq!(cfg.view.focus_granularity, FocusGranularity::Paragraph, "bad granularity -> default");
        assert!(warnings.iter().any(|w| w.contains("wrap_column")));
        // The symmetric upper clamp (repar's parse ceiling — Fable whole-branch I-1).
        let d2 = tempdir();
        let hi = write(&d2, "hi.toml", "[view]\nwrap_column = 12000\n");
        let (cfg2, warns2) = load(&[hi]);
        assert_eq!(cfg2.view.wrap_column, 9999, "wrap_column clamped to max 9999");
        assert!(warns2.iter().any(|w| w.contains("above max 9999")));
        assert!(warnings.iter().any(|w| w.contains("focus_granularity")));
    }

    // tiny temp-dir helper (unique; avoids real $HOME)
    fn tempdir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "wc-cfg-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn diagnostics_config_defaults_and_validation() {
        // default: enabled, grammar on, debounce 400
        let (cfg, _warns) = load(&[]);
        assert!(cfg.diagnostics.enabled);
        assert!(cfg.diagnostics.grammar);
        assert_eq!(cfg.diagnostics.debounce_ms, 400);
    }

    #[test]
    fn diagnostics_debounce_is_clamped_with_warning() {
        let dir = tempdir();
        let p = dir.join("c.toml");
        std::fs::write(&p, "[diagnostics]\ndebounce_ms = 5\n").unwrap();
        let (cfg, warns) = load(&[p]);
        assert_eq!(cfg.diagnostics.debounce_ms, 100, "debounce_ms clamped to floor 100");
        assert!(warns.iter().any(|w| w.contains("debounce_ms")), "clamp warns");
    }

    // -----------------------------------------------------------------------
    // Fix A7: default dictionary path + tilde expansion
    // -----------------------------------------------------------------------

    /// `DiagnosticsConfig::default().dictionary` must resolve to
    /// `<config_dir>/wordcartel/dictionary.txt` (not None).
    #[test]
    fn diagnostics_default_dictionary_is_not_none() {
        let cfg = DiagnosticsConfig::default();
        // If dirs::config_dir() is available (it always is on Linux/macOS/Windows),
        // the default must be Some(<config_dir>/wordcartel/dictionary.txt).
        // We do NOT require the file to exist — just that the path is set.
        if let Some(config_dir) = dirs::config_dir() {
            let expected = config_dir.join("wordcartel").join("dictionary.txt");
            assert_eq!(cfg.dictionary, Some(expected),
                "default dictionary must point to <config_dir>/wordcartel/dictionary.txt");
        } else {
            // On exotic platforms where config_dir() returns None, None is acceptable.
            // (We can't assert Some in that case.)
        }
    }

    /// A `~/` prefix in the configured dictionary path must be expanded to the
    /// real home directory — NOT stored as a literal `~`.
    #[test]
    fn dictionary_tilde_is_expanded() {
        let dir = tempdir();
        let p = dir.join("c.toml");
        // Use a temp-dir-based path that doesn't touch the real home directory.
        // We test the expansion logic by checking whether a configured "~/foo/dict.txt"
        // expands to <home>/foo/dict.txt.
        std::fs::write(&p, "[diagnostics]\ndictionary = \"~/foo/dict.txt\"\n").unwrap();
        let (cfg, warns) = load(&[p]);
        assert!(warns.is_empty(), "tilde path must not produce warnings");
        if let Some(home) = dirs::home_dir() {
            let expected = home.join("foo").join("dict.txt");
            assert_eq!(cfg.diagnostics.dictionary, Some(expected),
                "~/foo/dict.txt must expand to <home>/foo/dict.txt, not a literal ~");
        }
        // Regardless of home detection: the path must NOT start with a literal tilde byte.
        if let Some(dict_path) = &cfg.diagnostics.dictionary {
            let first = dict_path.to_string_lossy();
            assert!(!first.starts_with('~'),
                "expanded dictionary path must not start with a literal tilde, got: {first}");
        }
    }

    /// A bare `~` expands to home_dir (not a literal tilde directory).
    #[test]
    fn dictionary_bare_tilde_expands_to_home() {
        let dir = tempdir();
        let p = dir.join("c.toml");
        std::fs::write(&p, "[diagnostics]\ndictionary = \"~\"\n").unwrap();
        let (cfg, _warns) = load(&[p]);
        if let Some(home) = dirs::home_dir() {
            assert_eq!(cfg.diagnostics.dictionary, Some(home),
                "bare ~ must expand to home_dir");
        }
    }

    // -----------------------------------------------------------------------
    // Task 3: [theme] config layering
    // -----------------------------------------------------------------------

    #[test]
    fn theme_name_parses() {
        let d = tempdir();
        let p = write(&d, "c.toml", "[theme]\nname = \"tokyo-night\"\n");
        let (cfg, warns) = load(&[p]);
        assert_eq!(cfg.theme.name.as_deref(), Some("tokyo-night"));
        assert!(cfg.theme.file.is_none());
        assert!(warns.is_empty());
    }

    #[test]
    fn theme_file_resolves_relative_to_declaring_config() {
        let d = tempdir();
        let p = write(&d, "c.toml", "[theme]\nfile = \"palettes/gruvbox.yaml\"\n");
        let (cfg, _w) = load(&[p]);
        // resolved against the config file's directory, not CWD
        assert_eq!(cfg.theme.file, Some(d.join("palettes/gruvbox.yaml")));
        assert!(cfg.theme.name.is_none());
    }

    #[test]
    fn theme_name_then_file_across_layers_is_discriminated() {
        let d = tempdir();
        let lo = write(&d, "lo.toml", "[theme]\nname = \"tokyo-night\"\n");
        let hi = write(&d, "hi.toml", "[theme]\nfile = \"g.yaml\"\n");
        let (cfg, _w) = load(&[lo, hi]); // hi overrides
        assert!(cfg.theme.name.is_none(), "a later `file` clears an earlier `name`");
        assert_eq!(cfg.theme.file, Some(d.join("g.yaml")));
    }

    #[test]
    fn theme_name_and_file_same_layer_file_wins_with_warning() {
        let d = tempdir();
        let p = write(&d, "c.toml", "[theme]\nname = \"tokyo-night\"\nfile = \"g.yaml\"\n");
        let (cfg, warns) = load(&[p]);
        assert!(cfg.theme.name.is_none());
        assert!(cfg.theme.file.is_some());
        assert!(warns.iter().any(|w| w.contains("name") && w.contains("file")));
    }

    #[test]
    fn theme_styles_accumulate_across_layers() {
        let d = tempdir();
        let lo = write(&d, "lo.toml", "[theme.styles]\nheading1 = { fg = \"#bb9af7\", bold = true }\n");
        let hi = write(&d, "hi.toml", "[theme.styles]\nselection = { bg = \"#283457\" }\n");
        let (cfg, _w) = load(&[lo, hi]);
        assert!(cfg.theme.styles.contains_key("heading1"));
        assert!(cfg.theme.styles.contains_key("selection"));
    }

    // -----------------------------------------------------------------------
    // [export] config layering
    // -----------------------------------------------------------------------

    #[test]
    fn export_config_defaults_when_section_absent() {
        let (cfg, warns) = load(&[]);
        assert!(warns.is_empty());
        assert_eq!(cfg.export.pdf_engine, "xelatex");
        assert!(cfg.export.typography);
    }

    #[test]
    fn export_config_partial_section_inherits_per_field() {
        let d = tempdir();
        let p = write(&d, "c.toml", "[export]\ntypography = false\n");
        let (cfg, warns) = load(&[p]);
        assert!(warns.is_empty());
        assert_eq!(cfg.export.pdf_engine, "xelatex", "pdf_engine stays default");
        assert!(!cfg.export.typography, "typography overridden to false");
    }

    #[test]
    fn export_config_two_layers_per_field_inherit() {
        let d = tempdir();
        let lo = write(&d, "lo.toml", "[export]\ntypography = false\n");
        let hi = write(&d, "hi.toml", "[export]\npdf_engine = \"tectonic\"\n");
        let (cfg, warns) = load(&[lo, hi]);
        assert!(warns.is_empty());
        assert_eq!(cfg.export.pdf_engine, "tectonic", "hi set pdf_engine → wins");
        assert!(!cfg.export.typography, "hi omitted typography → lo's false preserved");
    }

    #[test]
    fn pre_theming_config_still_loads() {
        let d = tempdir();
        let p = write(&d, "c.toml", "[view]\ntypewriter = true\n"); // no [theme] at all
        let (cfg, warns) = load(&[p]);
        assert!(cfg.view.typewriter);
        assert!(cfg.theme.name.is_none() && cfg.theme.file.is_none());
        assert!(warns.is_empty());
    }

    // -----------------------------------------------------------------------
    // [menu] config layering
    // -----------------------------------------------------------------------

    #[test]
    fn menu_bar_absent_defaults_to_auto() {
        let (cfg, warns) = load(&[]);
        assert!(warns.is_empty());
        assert_eq!(cfg.menu.bar, MenuBarMode::Auto, "absent [menu] → Auto");
    }

    #[test]
    fn menu_bar_each_valid_string_folds_to_its_variant() {
        for (s, want) in [("hidden", MenuBarMode::Hidden), ("auto", MenuBarMode::Auto), ("pinned", MenuBarMode::Pinned)] {
            let d = tempdir();
            let p = write(&d, "c.toml", &format!("[menu]\nbar = \"{s}\"\n"));
            let (cfg, warns) = load(&[p]);
            assert!(warns.is_empty(), "no warnings for valid variant \"{s}\"");
            assert_eq!(cfg.menu.bar, want, "\"{}\" → {:?}", s, want);
        }
    }

    #[test]
    fn menu_bar_bogus_string_stays_auto_with_warning() {
        let d = tempdir();
        let p = write(&d, "c.toml", "[menu]\nbar = \"bogus\"\n");
        let (cfg, warns) = load(&[p]);
        assert_eq!(cfg.menu.bar, MenuBarMode::Auto, "bogus value → stays Auto");
        assert!(warns.iter().any(|w| w.contains("menu.bar")),
            "must warn containing 'menu.bar'; got: {warns:?}");
    }

    // -----------------------------------------------------------------------
    // Task 1 (D1+A5): preset-scoped keymap patches
    // -----------------------------------------------------------------------

    #[test]
    fn scoped_keymap_tables_parse_into_named_fields() {
        let d = tempdir();
        let p = write(&d, "s.toml",
            "[keymap]\npreset='cua'\nbind={ \"ctrl-g\"='goto_line' }\n[keymap.cua]\nbind={ \"ctrl-w\"='close_buffer' }\n[keymap.wordstar]\nunbind=[\"ctrl-q ctrl-q\"]\n");
        let (cfg, warns) = load(&[p]);
        assert!(warns.is_empty());
        let patch = &cfg.keymap.patches[0];
        assert!(patch.bind.contains_key("ctrl-g"), "global bind unchanged");
        assert_eq!(patch.cua.as_ref().unwrap().bind.get("ctrl-w").unwrap(), "close_buffer");
        assert_eq!(patch.wordstar.as_ref().unwrap().unbind[0], "ctrl-q ctrl-q");
    }

    #[test]
    fn global_only_configs_leave_scoped_fields_none() {
        let d = tempdir();
        let p = write(&d, "g.toml", "[keymap]\nbind={ \"ctrl-g\"='goto_line' }\n");
        let (cfg, _) = load(&[p]);
        assert!(cfg.keymap.patches[0].cua.is_none() && cfg.keymap.patches[0].wordstar.is_none());
    }

    // D1+A5 Task 4 — baseline vs. production config separation pins ----------

    #[test]
    fn baseline_excludes_overrides_layer() {
        // Spec pin: baseline = load(&[hand]) does NOT include the overrides file;
        // production = load(&[hand, overrides]) does include it.
        let d = tempdir();
        let hand_path = write(&d, "hand.toml", "[keymap]\npreset = 'wordstar'\n");
        let overrides_path = write(&d, "settings-overrides.toml",
            "# managed by wcartel\n[view]\ntypewriter = true\n");

        let (baseline, _) = load(std::slice::from_ref(&hand_path));   // WITHOUT overrides
        let (production, _) = load(&[hand_path, overrides_path]); // WITH overrides

        // Both share the hand-layer keymap.preset value.
        assert_eq!(baseline.keymap.preset, "wordstar", "hand layer keymap.preset in baseline");
        assert_eq!(production.keymap.preset, "wordstar", "hand layer keymap.preset in production");
        // The overrides-only key must differ: baseline lacks it.
        assert!(!baseline.view.typewriter,
            "baseline must NOT include the overrides layer typewriter=true");
        assert!(production.view.typewriter,
            "production must include the overrides layer typewriter=true");
    }

    #[test]
    fn view_transient_keys_parse_and_status_off_coerces() {
        // scrollbar accepts off/auto/on verbatim.
        let d1 = tempdir();
        let p = write(&d1, "c.toml", "[view]\nscrollbar = \"on\"\nstatus_line = \"auto\"\n");
        let (cfg, warns) = load(&[p]);
        assert_eq!(cfg.view.scrollbar, TransientMode::On);
        assert_eq!(cfg.view.status_line, TransientMode::Auto);
        assert!(warns.is_empty());
        // status_line = "off" is rejected → coerced to Auto, with a warning (no-silent-UI).
        let d2 = tempdir();
        let p2 = write(&d2, "c.toml", "[view]\nstatus_line = \"off\"\n");
        let (cfg2, warns2) = load(&[p2]);
        assert_eq!(cfg2.view.status_line, TransientMode::Auto,
            "status_line off must coerce to auto to preserve no-silent-UI");
        assert!(warns2.iter().any(|w| w.contains("status_line")),
            "coercion must warn, got {warns2:?}");
        // bogus value → default + warning.
        let d3 = tempdir();
        let p3 = write(&d3, "c.toml", "[view]\nscrollbar = \"bogus\"\n");
        let (cfg3, warns3) = load(&[p3]);
        assert_eq!(cfg3.view.scrollbar, TransientMode::Auto, "bogus → default auto");
        assert!(warns3.iter().any(|w| w.contains("scrollbar")));
    }

    #[test]
    fn save_reload_roundtrip_restores_settings() {
        // Unit-level pin of the full pipeline without run():
        // 1. Build a runtime snapshot with three divergences from the default baseline.
        // 2. compute_overrides + save_overrides to a tempdir file.
        // 3. config::load(&[overrides_file]) and assert the merged config reflects them.
        use crate::settings::{
            SettingsSnapshot, ThemeIdentity, OverridesFile,
            compute_overrides, save_overrides, snapshot_of,
        };

        let d = tempdir();
        let overrides_path = d.join("settings-overrides.toml");

        // Baseline: defaults (empty layer list — no hand config, no overrides).
        let (baseline_cfg, _) = load(&[]);
        // Use the default resolved theme name (tests run without a real env, so
        // resolve_theme falls back to the default theme — name = "terminal-plain").
        let env = crate::theme_resolve::EnvSnapshot::from_env();
        let baseline_resolved = crate::theme_resolve::resolve_theme(
            &baseline_cfg.theme, &env, wordcartel_core::theme::ChromeDisposition::Full);
        let baseline = snapshot_of(&baseline_cfg, &baseline_resolved.theme.name);

        // Runtime snapshot: eight divergences — keymap → wordstar, typewriter on,
        // bar → pinned, mouse capture off, theme → tokyo-night, wrap_column → 100,
        // chrome → Zen, canvas → Transparent (spec Testing: the round-trip covers
        // [mouse] capture and [theme] name + chrome + canvas too — Fable m-wb-1;
        // wrap_column 100 is distinct from default 72).
        use wordcartel_core::theme::CanvasMode;
        let runtime = SettingsSnapshot {
            keymap_preset:   "wordstar".to_string(),
            theme_identity:  ThemeIdentity::Builtin("tokyo-night".to_string()),
            view_typewriter: true,
            view_focus:      false,
            view_measure:    false,
            view_wrap_guide: false,
            view_word_count: false,
            view_wrap_column: 100,
            view_scrollbar:  crate::config::TransientMode::Auto,
            view_status_line: crate::config::TransientMode::On,
            view_splash:     true,
            view_caret_shape: crate::config::CaretShape::Default,
            view_caret_blink: true,
            menu_bar:        crate::config::MenuBarMode::Pinned,
            mouse_capture:   false,
            chrome_disposition: wordcartel_core::theme::ChromeDisposition::Zen,
            canvas: CanvasMode::Transparent,
            clipboard_provider: crate::config::ClipboardProvider::Auto,
            view_messages_min_kind: crate::status::StatusKind::Info,
            // C5 Task 24: files_show_clutter/files_type_filter round-trip too.
            files_show_clutter: true,
            files_type_filter: crate::config::FileTypeFilter::All,
        };

        let of = compute_overrides(&runtime, &baseline, &OverridesFile::default(), &OverridesFile::default());
        save_overrides(&crate::fsx::RealFs, &overrides_path, &of).unwrap();

        // Reload through the REAL config::load and assert the values round-tripped.
        let (cfg, _) = load(&[overrides_path]);
        assert_eq!(cfg.keymap.preset, "wordstar",
            "keymap.preset must round-trip to 'wordstar'");
        assert!(cfg.view.typewriter, "view.typewriter must round-trip to true");
        assert_eq!(cfg.menu.bar, crate::config::MenuBarMode::Pinned,
            "menu.bar must round-trip to Pinned");
        assert!(!cfg.mouse.mouse_capture, "mouse capture must round-trip to false");
        assert_eq!(cfg.theme.name.as_deref(), Some("tokyo-night"),
            "theme name must round-trip");
        assert_eq!(cfg.view.wrap_column, 100,
            "view.wrap_column must round-trip to 100");
        assert_eq!(cfg.theme.chrome.as_deref(), Some("zen"),
            "[theme] chrome must round-trip to 'zen'");
        assert_eq!(cfg.theme.canvas.as_deref(), Some("transparent"),
            "[theme] canvas must round-trip");
        assert!(cfg.files.show_clutter, "files.show_clutter must round-trip to true");
        assert_eq!(cfg.files.type_filter, crate::config::FileTypeFilter::All,
            "files.type_filter must round-trip to All");
    }

    // -----------------------------------------------------------------------
    // [files] filter defaults + invalid value (C5 Task 24 fix I2)
    //
    // The round-trip test above only ever exercises DIVERGENT values (true/All), so it
    // never touches the default-on-absent path at all — flipping `FilesConfig::default()`
    // to `{true, All}` breaks nothing workspace-wide without these. Mirrors the
    // `clipboard_provider_default_is_auto` / `clipboard_provider_unknown_warns_and_defaults_auto`
    // pair `[files]` was copied from but left unguarded.
    // -----------------------------------------------------------------------

    fn load_files(name: &str, body: &str) -> (Config, Vec<String>) {
        let p = std::env::temp_dir().join(format!("wcartel-cfg-{}-{name}.toml", std::process::id()));
        std::fs::write(&p, body).unwrap();
        let out = load(std::slice::from_ref(&p));
        let _ = std::fs::remove_file(&p);
        out
    }

    #[test]
    fn files_filters_default_on_absent() {
        let (cfg, _) = load(&[]); // no config file → defaults
        assert!(!cfg.files.show_clutter,
            "files.show_clutter must default to false (hidden files off)");
        assert_eq!(cfg.files.type_filter, FileTypeFilter::Documents,
            "files.type_filter must default to Documents");
    }

    #[test]
    fn files_type_filter_unknown_warns_and_defaults_documents() {
        let (cfg, warns) = load_files("unknown", "[files]\ntype_filter = \"spreadsheets\"\n");
        assert_eq!(cfg.files.type_filter, FileTypeFilter::Documents);
        assert!(warns.iter().any(|w| w.contains("files.type_filter")),
            "the invalid-value arm must warn by name (H31 diagnostic); warns was: {warns:?}");
    }

    // -----------------------------------------------------------------------
    // [clipboard] provider config (C3 Task 4)
    // -----------------------------------------------------------------------

    fn load_clip(name: &str, body: &str) -> (Config, Vec<String>) {
        let p = std::env::temp_dir().join(format!("wcartel-cfg-{}-{name}.toml", std::process::id()));
        std::fs::write(&p, body).unwrap();
        let out = load(std::slice::from_ref(&p));
        let _ = std::fs::remove_file(&p);
        out
    }

    #[test]
    fn clipboard_provider_parses_all_values() {
        for (s, want) in [("auto", ClipboardProvider::Auto), ("native", ClipboardProvider::Native),
                          ("osc52", ClipboardProvider::Osc52), ("off", ClipboardProvider::Off)] {
            let (cfg, _warns) = load_clip(s, &format!("[clipboard]\nprovider = \"{s}\"\n"));
            assert_eq!(cfg.clipboard.provider, want, "value {s}");
        }
    }

    #[test]
    fn clipboard_provider_unknown_warns_and_defaults_auto() {
        let (cfg, warns) = load_clip("unknown", "[clipboard]\nprovider = \"telepathy\"\n");
        assert_eq!(cfg.clipboard.provider, ClipboardProvider::Auto);
        assert!(warns.iter().any(|w| w.contains("clipboard.provider")),
            "the invalid-value arm must warn by name (H31 diagnostic); warns was: {warns:?}");
    }

    #[test]
    fn clipboard_provider_default_is_auto() {
        let (cfg, _) = load(&[]); // no config file → defaults
        assert_eq!(cfg.clipboard.provider, ClipboardProvider::Auto);
    }

    #[test]
    fn clipboard_provider_str_roundtrips() {
        assert_eq!(clipboard_provider_str(ClipboardProvider::Auto), "auto");
        assert_eq!(clipboard_provider_str(ClipboardProvider::Native), "native");
        assert_eq!(clipboard_provider_str(ClipboardProvider::Osc52), "osc52");
        assert_eq!(clipboard_provider_str(ClipboardProvider::Off), "off");
    }

    // -----------------------------------------------------------------------
    // [diagnostics] linters + [diagnostics.harper] grammar override (SPINE Task 8)
    // -----------------------------------------------------------------------

    fn load_diag(name: &str, body: &str) -> (Config, Vec<String>) {
        let p = std::env::temp_dir().join(format!("wcartel-cfg-{}-{name}.toml", std::process::id()));
        std::fs::write(&p, body).unwrap();
        let out = load(std::slice::from_ref(&p));
        let _ = std::fs::remove_file(&p);
        out
    }

    #[test]
    fn harper_engine_table_overrides_grammar() {
        let (cfg, _warns) = load_diag("harper-grammar",
            "[diagnostics]\ngrammar = true\n[diagnostics.harper]\ngrammar = false\n");
        assert!(!cfg.diagnostics.grammar, "[diagnostics.harper].grammar overrides top-level");
    }

    #[test]
    fn linters_list_round_trips() {
        let (cfg, _warns) = load_diag("linters", "[diagnostics]\nlinters = [\"harper\"]\n");
        assert_eq!(cfg.diagnostics.linters, Some(vec!["harper".to_string()]));
    }

    #[test]
    fn caret_shape_str_roundtrips() {
        for s in [CaretShape::Default, CaretShape::Block, CaretShape::Beam, CaretShape::Underline] {
            assert_eq!(caret_shape_from_str(caret_shape_str(s)), Some(s));
        }
        assert_eq!(caret_shape_from_str("bogus"), None);
    }

    #[test]
    fn viewconfig_defaults_caret_default_blink_on() {
        let v = ViewConfig::default();
        assert_eq!(v.caret_shape, CaretShape::Default);
        assert!(v.caret_blink, "blink default on (inert under Default until a shape is chosen)");
    }

    // -----------------------------------------------------------------------
    // Task 6: config-class reads acquire a cap
    // -----------------------------------------------------------------------

    #[test]
    fn config_over_cap_degrades_like_an_unreadable_file() {
        // Config-class reads acquire a cap. An over-cap config must warn and fall back to
        // defaults — the SAME degradation an unreadable file already gets — never panic and
        // never silently apply a truncated parse.
        let d = std::env::temp_dir().join(format!("wc-cfg-cap-{}", std::process::id()));
        std::fs::create_dir_all(&d).expect("dir");
        let p = d.join("config.toml");
        std::fs::write(&p, vec![b'#'; (crate::limits::MAX_CONFIG_BYTES + 1) as usize])
            .expect("seed oversized");
        let (cfg, warns) = load_with_fs(&crate::fsx::RealFs, std::slice::from_ref(&p));
        assert_eq!(cfg.state.max_entries, Config::default().state.max_entries,
            "over-cap config falls back to defaults");
        // Names the OVER-CAP branch specifically. `|| w.contains("cannot read")` would let a
        // broken read path read as a cap success — the cap could be absent and an unrelated
        // IO failure would satisfy the assertion.
        assert!(warns.iter().any(|w| w.contains("too large")),
            "the warning must name the CAP, not merely any read failure: {warns:?}");
        let _ = std::fs::remove_dir_all(&d);
    }
}
