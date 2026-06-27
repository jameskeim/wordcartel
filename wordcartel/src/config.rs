//! Layered TOML config + CLI parsing. Built-in defaults < XDG < project-local < --config.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use serde::Deserialize;

#[derive(Debug, Default, Clone)]
pub struct Cli {
    pub path: Option<PathBuf>,
    pub config_path: Option<PathBuf>,
    pub no_config: bool,
}

/// Hand-rolled (no clap dep): `[--config <path>] [--no-config] [file]`.
pub fn parse_cli<I: IntoIterator<Item = String>>(args: I) -> Cli {
    let mut cli = Cli::default();
    let mut it = args.into_iter();
    let _ = it.next(); // argv[0]
    while let Some(a) = it.next() {
        match a.as_str() {
            "--no-config" => cli.no_config = true,
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
}

#[derive(Debug, Default, Clone)]
pub struct ThemeConfig {
    pub name: Option<String>,
    pub file: Option<PathBuf>,           // ~-expanded, resolved relative to declaring config
    pub depth: Option<String>,           // "truecolor"|"256"|"16"|"none"
    pub heading_level_glyph: Option<bool>,
    pub styles: BTreeMap<String, RawFace>,
}

#[derive(Debug, Clone)]
pub struct DiagnosticsConfig {
    pub enabled: bool,
    pub grammar: bool,
    pub debounce_ms: u64,
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
}
impl Default for ViewConfig {
    fn default() -> Self {
        ViewConfig { typewriter: false, typewriter_anchor: 0.5, focus: false,
            focus_granularity: FocusGranularity::Paragraph, measure: false,
            wrap_column: 80, wrap_guide: false, word_count: false }
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
}
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawKeymap {
    preset: Option<String>,
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
struct RawView {
    typewriter: Option<bool>,
    typewriter_anchor: Option<f32>,
    focus: Option<bool>,
    focus_granularity: Option<String>,
    measure: Option<bool>,
    wrap_column: Option<u16>,
    wrap_guide: Option<bool>,
    word_count: Option<bool>,
}

/// Ordered existing config files, lowest→highest precedence. Empty when --no-config.
pub fn config_layer_paths(
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
        if p.is_file() {
            v.push(p);
        }
    }
    // project-local: nearest .wordcartel.toml walking up from anchor_dir
    let mut dir = Some(anchor_dir);
    while let Some(d) = dir {
        let p = d.join(".wordcartel.toml");
        if p.is_file() {
            v.push(p);
            break;
        }
        dir = d.parent();
    }
    if let Some(c) = &cli.config_path {
        if c.is_file() {
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
pub fn load(paths: &[PathBuf]) -> (Config, Vec<String>) {
    let mut cfg = Config::default();
    let mut warns = Vec::new();
    for p in paths {
        let text = match std::fs::read_to_string(p) {
            Ok(t) => t,
            Err(e) => {
                warns.push(format!("config: cannot read {}: {e}", p.display()));
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
        if let Some(a) = raw.view.typewriter_anchor {
            if (0.0..=1.0).contains(&a) { cfg.view.typewriter_anchor = a; }
            else { cfg.view.typewriter_anchor = a.clamp(0.0, 1.0);
                   warns.push(format!("view.typewriter_anchor {a} out of 0.0..=1.0; clamped")); }
        }
        if let Some(c) = raw.view.wrap_column {
            if c >= 20 { cfg.view.wrap_column = c; }
            else { cfg.view.wrap_column = 20;
                   warns.push(format!("view.wrap_column {c} below min 20; clamped to 20")); }
        }
        if let Some(g) = raw.view.focus_granularity {
            match g.as_str() {
                "paragraph" => cfg.view.focus_granularity = FocusGranularity::Paragraph,
                "sentence"  => cfg.view.focus_granularity = FocusGranularity::Sentence,
                other => warns.push(format!("view.focus_granularity \"{other}\" invalid; using paragraph")),
            }
        }
        // diagnostics: per-field override + debounce_ms floor validation.
        if let Some(v) = raw.diagnostics.enabled { cfg.diagnostics.enabled = v; }
        if let Some(v) = raw.diagnostics.grammar { cfg.diagnostics.grammar = v; }
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
        // unknown linter names are validated against the core catalog later (Task 4 assembly) — warn there.

        // ---- [theme] (discriminated source; file resolved vs the declaring config) ----
        let rt = raw.theme;
        let layer_dir = p.parent().unwrap_or_else(|| std::path::Path::new("."));
        let raw_file = rt.file.clone();
        // Resolve a layer's `file` (~ expand + relative-to-this-config) if present.
        let resolved_file = raw_file.as_ref().map(|s| {
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
        if let Some(h) = rt.heading_level_glyph { cfg.theme.heading_level_glyph = Some(h); }
        for (k, v) in rt.styles { cfg.theme.styles.insert(k, v); } // accumulate across layers
    }
    (cfg, warns)
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
        assert_eq!(
            cfg.state.resume,
            false,
            "hi OMITTED resume → lo's false is preserved (NOT reset to default true)"
        );
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

    #[test]
    fn pre_theming_config_still_loads() {
        let d = tempdir();
        let p = write(&d, "c.toml", "[view]\ntypewriter = true\n"); // no [theme] at all
        let (cfg, warns) = load(&[p]);
        assert!(cfg.view.typewriter);
        assert!(cfg.theme.name.is_none() && cfg.theme.file.is_none());
        assert!(warns.is_empty());
    }

}
