//! pathx — pure tilde expansion and the platform-dirs carrier.
//!
//! The H33 seam: `dirs::*` is read ONCE at a production boundary (`PlatformDirs::from_env`,
//! or an inline `dirs::home_dir()` at a caller that IS the boundary) and passed down as
//! explicit data. Pure code and tests below the boundary never read the process
//! environment.

use std::path::{Path, PathBuf};

/// Expand a tilde against an EXPLICIT home directory — the pure core of every `~` site.
///
/// - `"~"` → `home`, or the literal `"~"` when `home` is `None`.
/// - `"~/rest"` → `home/rest`, or the literal input when `home` is `None`.
/// - anything else → `PathBuf::from(text)`, verbatim (no mid-string expansion).
///
/// `home` is `dirs::home_dir()` at every production boundary and an injected temp dir in
/// tests — no caller of this function reads the process environment.
///
/// # Examples
///
/// ```
/// use std::path::{Path, PathBuf};
/// use wordcartel::pathx::expand_tilde;
///
/// let home = Path::new("/home/w");
/// assert_eq!(expand_tilde("~/notes.md", Some(home)), PathBuf::from("/home/w/notes.md"));
/// assert_eq!(expand_tilde("~", Some(home)), PathBuf::from("/home/w"));
/// assert_eq!(expand_tilde("~/notes.md", None), PathBuf::from("~/notes.md"));
/// assert_eq!(expand_tilde("plain.md", Some(home)), PathBuf::from("plain.md"));
/// ```
pub fn expand_tilde(text: &str, home: Option<&Path>) -> PathBuf {
    if text == "~" {
        return home.map(PathBuf::from).unwrap_or_else(|| PathBuf::from("~"));
    }
    if let Some(rest) = text.strip_prefix("~/") {
        return home.map(|h| h.join(rest)).unwrap_or_else(|| PathBuf::from(text));
    }
    PathBuf::from(text)
}

/// Platform directories resolved ONCE at a production boundary and passed down — the
/// injection carrier that keeps `dirs::*` reads out of pure code and out of tests. A dumb
/// carrier on purpose: tests construct it literally with explicit paths; accessor ceremony
/// would fight the point.
///
/// # Examples
///
/// ```
/// use wordcartel::pathx::PlatformDirs;
///
/// // A test injects explicit dirs; production calls `PlatformDirs::from_env()`.
/// let dirs = PlatformDirs { home: Some("/home/w".into()), config_dir: None };
/// assert_eq!(dirs.home.as_deref(), Some(std::path::Path::new("/home/w")));
/// ```
pub struct PlatformDirs {
    pub home: Option<PathBuf>,
    pub config_dir: Option<PathBuf>,
}

impl PlatformDirs {
    /// Resolve from the real environment — production boundaries only; tests construct
    /// the struct literally with explicit paths. (Deliberately no unit test: this fn IS
    /// the env read, and asserting on it would recreate the oracle coupling H33 removes.)
    pub fn from_env() -> Self {
        PlatformDirs { home: dirs::home_dir(), config_dir: dirs::config_dir() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilde_slash_joins_the_injected_home() {
        let home = Path::new("/inj/home");
        assert_eq!(expand_tilde("~/a/b.md", Some(home)), PathBuf::from("/inj/home/a/b.md"));
    }

    #[test]
    fn bare_tilde_is_the_injected_home_itself() {
        let home = Path::new("/inj/home");
        assert_eq!(expand_tilde("~", Some(home)), PathBuf::from("/inj/home"));
    }

    #[test]
    fn no_home_falls_back_to_the_literal_input() {
        // The never-before-tested branches: every legacy site fell back to the literal
        // text when the platform had no resolvable home. Asserted here, unconditionally.
        assert_eq!(expand_tilde("~", None), PathBuf::from("~"));
        assert_eq!(expand_tilde("~/a.md", None), PathBuf::from("~/a.md"));
    }

    #[test]
    fn non_tilde_text_passes_through_verbatim() {
        let home = Path::new("/inj/home");
        assert_eq!(expand_tilde("plain/rel.md", Some(home)), PathBuf::from("plain/rel.md"));
        assert_eq!(expand_tilde("/abs/p.md", Some(home)), PathBuf::from("/abs/p.md"));
        assert_eq!(expand_tilde("", Some(home)), PathBuf::from(""));
        // No mid-string expansion — only a LEADING tilde means home.
        assert_eq!(expand_tilde("a/~/b", Some(home)), PathBuf::from("a/~/b"));
    }
}
