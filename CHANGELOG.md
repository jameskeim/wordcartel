# Changelog

All notable changes to wordcartel (binary: `wcartel`) are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Pre-1.0, MINOR marks releases
with new features / notable UX and PATCH marks bugfix-only follow-ups; `1.0.0` is reserved for the
Effort-P plugin capstone.

## [Unreleased]

## [0.1.0] — 2026-07-09

Initial versioned release — the markdown-first terminal word processor, feature-complete for the
pre-plugin (pre-Effort-P) milestone.

### Added
- **Editing core** — instant per-keystroke typing over a functional-core buffer (`wordcartel-core`,
  `#![forbid(unsafe_code)]`), grapheme-correct caret motion, word/paragraph/page/document navigation,
  selection, undo/redo, and register cut/copy/paste.
- **Markdown rendering modes** — Live Preview (conceal markers on inactive lines), Source Highlighted,
  and Source Plain; incremental block-tree parse with a debounced full-reparse reconcile.
- **Section folding** — fold/unfold by heading, fold-all/unfold-all, fold-aware navigation and scroll,
  with a caret-never-inside-a-fold invariant.
- **Theming & chrome** — a theme system (tokyo-night, phosphor, terminal-ansi, and more), a six-face
  chrome elevation ladder, density presets (Full/Zen), opaque/transparent canvas, and accessibility
  (no-color / cue) modes.
- **Search** — incremental find, regex, and query-replace (single-undo replace-all) with match
  highlighting.
- **Diagnostics** — spelling and grammar checking (Harper) with underline overlays and quick-fix apply.
- **Transform & export** — reflow/transform commands and export to HTML / DOCX / PDF via pandoc
  (`--pdf-engine=xelatex`).
- **Clipboard** — native Wayland/X11 (dlopened), OSC 52, and `wl-copy`/`xclip` provider fallbacks.
- **Command surface** — a name-keyed command registry, an exhaustive command palette, a menu bar
  (a subset of the palette), keybinding presets (CUA / WordStar), and mouse support.
- **Durability** — crash-recovery swap files, session restore (cursor/scroll/marks/folds), and
  multi-buffer support with a persistent scratch buffer.
- **Resource discipline** — idle is free (the input loop blocks with nothing pending); background work
  is edge-triggered by real state changes, not wall-clock.

### Changed
- Version is now the Cargo workspace `version` (SemVer, starting at `0.1.0`); both crates inherit it.
- The Arch `PKGBUILD` derives `pkgver` from `git describe --tags` (tag-anchored) instead of a raw
  commit-count snapshot.

### Added (CLI)
- `wcartel --version` / `-V` prints the version and exits.

[Unreleased]: https://github.com/jameskeim/wordcartel/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/jameskeim/wordcartel/releases/tag/v0.1.0
