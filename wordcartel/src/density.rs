//! Chrome density presets (E1). A preset is DATA — a `ChromeBundle` of
//! `element → value` applied by one general `apply_bundle` routine, never
//! `if zen {…}` branching. Two built-ins (`ZEN`, `FULL`); the shape is additive
//! so config-defined bundles (L2) and named profiles that also set theme/keymap
//! (L3) are future efforts, not rewrites.

use crate::config::{MenuBarMode, TransientMode};
use crate::editor::Editor;
use wordcartel_core::theme::ChromeDisposition;

/// The resolved density target for each preset-owned chrome element. Fields are
/// additive: L3 may add `theme`/`keymap` without changing `apply_bundle`.
#[derive(Debug, Clone, Copy)]
pub struct ChromeBundle {
    /// The chrome disposition this bundle selects.
    pub chrome_disposition: ChromeDisposition,
    /// Menu bar visibility mode for this density.
    pub menu_bar: MenuBarMode,
    /// Status line reveal policy for this density.
    pub status_line: TransientMode,
    /// Scrollbar reveal policy for this density.
    pub scrollbar: TransientMode,
    /// Whether the centered-measure column guide is enabled.
    pub measure: bool,
    /// Whether the word-count overlay is enabled.
    pub word_count: bool,
}

/// Zen density: muted chrome, everything transient, centered measure on, word count off.
pub const ZEN: ChromeBundle = ChromeBundle {
    chrome_disposition: ChromeDisposition::Zen,
    menu_bar: MenuBarMode::Auto,
    status_line: TransientMode::Auto,
    scrollbar: TransientMode::Auto,
    measure: true,
    word_count: false,
};

/// Full density: elevated chrome, everything pinned/on, no centered measure, word count on.
pub const FULL: ChromeBundle = ChromeBundle {
    chrome_disposition: ChromeDisposition::Full,
    menu_bar: MenuBarMode::Pinned,
    status_line: TransientMode::On,
    scrollbar: TransientMode::On,
    measure: false,
    word_count: true,
};

/// The built-in bundle for a disposition — the single lookup the density command uses.
pub fn bundle_for(disp: ChromeDisposition) -> &'static ChromeBundle {
    match disp { ChromeDisposition::Zen => &ZEN, ChromeDisposition::Full => &FULL }
}

/// Enumerable selectable-bundle names (today `["zen", "full"]`); L2/L3 extend this.
pub fn bundle_names() -> [&'static str; 2] { ["zen", "full"] }

/// Apply `bundle` to `editor`, setting every preset-owned element via its existing
/// runtime field. Origin-agnostic (built-in const vs future config/registry entry).
/// Does NOT itself request a theme re-derive — the caller (`toggle_chrome`) owns
/// that so the flip + re-derive stay a single honest transition.
pub fn apply_bundle(editor: &mut Editor, bundle: &ChromeBundle) {
    editor.chrome_disposition = bundle.chrome_disposition;
    editor.set_menu_bar_mode(bundle.menu_bar);
    editor.set_status_line_mode(bundle.status_line);
    editor.set_scrollbar_mode(bundle.scrollbar);
    editor.view_opts.measure    = bundle.measure;
    editor.view_opts.word_count = bundle.word_count;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Editor;
    use crate::config::{MenuBarMode, TransientMode};
    use wordcartel_core::theme::ChromeDisposition;

    // The bool fields are `const` so clippy::assertions_on_constants fires; allowing it
    // here because the test is a spec-table assertion — readable prose beats a const block.
    #[allow(clippy::assertions_on_constants)]
    #[test]
    fn zen_and_full_bundles_match_the_table() {
        assert_eq!(ZEN.chrome_disposition, ChromeDisposition::Zen);
        assert_eq!(ZEN.menu_bar, MenuBarMode::Auto);
        assert_eq!(ZEN.status_line, TransientMode::Auto);
        assert_eq!(ZEN.scrollbar, TransientMode::Auto);
        assert!(ZEN.measure);
        assert!(!ZEN.word_count);
        assert_eq!(FULL.chrome_disposition, ChromeDisposition::Full);
        assert_eq!(FULL.menu_bar, MenuBarMode::Pinned);
        assert_eq!(FULL.status_line, TransientMode::On);
        assert_eq!(FULL.scrollbar, TransientMode::On);
        assert!(!FULL.measure);
        assert!(FULL.word_count);
    }

    #[test]
    fn apply_bundle_sets_every_owned_field_and_clears_menu_dwell() {
        let mut e = Editor::new_from_text("x\n", None, (40, 8));
        e.menu_bar_mode = MenuBarMode::Hidden;
        e.mouse.menu_bar_revealed = true; // stale auto-state
        apply_bundle(&mut e, &FULL);
        assert_eq!(e.menu_bar_mode, MenuBarMode::Pinned);
        assert_eq!(e.status_line_mode, TransientMode::On);
        assert_eq!(e.scrollbar_mode, TransientMode::On);
        assert!(!e.view_opts.measure);
        assert!(e.view_opts.word_count);
        assert_eq!(e.chrome_disposition, ChromeDisposition::Full);
        assert!(!e.mouse.menu_bar_revealed, "bundle apply must clear stale menu dwell state");
    }
}
