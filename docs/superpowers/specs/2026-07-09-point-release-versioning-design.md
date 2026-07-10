# Point-release versioning + release ritual — design

**Date:** 2026-07-09 · **Status:** approved (brainstorm), implementing directly (small, well-bounded —
full spec→Codex→plan pipeline waived per the effort's size). **Closes:** engineering-health **H6**;
touches **H4** (packaging). **Origin:** user request "set up a point release for the app."

## Problem

The app has no human-meaningful version. Both workspace crates are `version = "0.0.0"`; there are no git
tags and no CHANGELOG; the Arch `PKGBUILD` stamps builds as `0.0.0.r<commit-count>.g<hash>` (a raw
`git describe` snapshot); and the CLI has **no `--version` flag at all** — the app cannot report its own
version. Distribution is a locally-built Arch **binary** package (`wcartel`), single maintainer, single
target (x86_64 Arch); it is a binary package deliberately, because the crate has an out-of-repo path
dependency (`../../par-command/repar`) that cannot build inside makepkg's sandbox.

## Decisions (locked in brainstorm)

1. **Goal:** human-meaningful version stamps **plus** a real, repeatable release ritual (tags + CHANGELOG +
   checklist).
2. **Scheme:** SemVer, pre-1.0 `0.MINOR.PATCH`. MINOR = a release with new features / notable UX;
   PATCH = bugfix-only follow-up. `1.0.0` is **reserved for the Effort-P plugin capstone** (per CLAUDE.md).
   First release: **`0.1.0`**, cut now against the current tree.
3. **Source of truth:** the Cargo **workspace** `version`. A git tag `vX.Y.Z` mirrors it; the PKGBUILD and
   the app's `--version` **derive** from it. The tag is a thin release marker, never a source of truth that
   breaks outside git.
4. **Changelog:** `CHANGELOG.md` in the "Keep a Changelog" format, hand-curated.

## Design

### 1. Version as workspace-inherited Cargo field

Root `Cargo.toml` gains:

```toml
[workspace.package]
version = "0.1.0"
```

Both members switch `version = "0.0.0"` → `version.workspace = true` (`wordcartel/Cargo.toml`,
`wordcartel-core/Cargo.toml`). One number for core + shell — they always ship together. Only `version` is
shared; `edition`/`license` stay per-member (minimal change).

### 2. `--version` CLI flag (new)

`config::Cli` gains `pub version: bool`. The hand-rolled `parse_cli` learns a `-V | --version` arm that sets
it (parser stays pure/testable — it does not print or exit). `main.rs`, immediately after `parse_cli`, when
`cli.version` is set, prints `wcartel {CARGO_PKG_VERSION}` to **stdout** and `std::process::exit(0)` — before
`app::run` and thus before the terminal guard / alternate screen, so stdout is safe. The version string is
`env!("CARGO_PKG_VERSION")` (compile-time; no `.git` needed; zero drift from the Cargo field).

`print_stdout` is denied workspace-wide (the app owns the terminal), so `main.rs`'s existing
`#[allow(clippy::print_stderr)]` is widened to `#[allow(clippy::print_stdout, clippy::print_stderr)]` with the
rationale that the version line is pre-guard stdout — the conventional `--version` channel.

**Command-surface contract: N/A** — `--version` is a process-lifecycle CLI flag (like `--config`/`--no-config`),
not an in-app command, user-settable option, palette/menu entry, or keybinding. It does not touch the command
surface.

### 3. PKGBUILD derivation (tag-anchored)

`pkgver()` changes from the commit-count form to `git describe --tags`, normalized to a valid Arch `pkgver`:

```sh
pkgver() {
  local root; root="$(_root)"
  cd "$root"
  git describe --tags 2>/dev/null | sed 's/^v//;s/-\([0-9]*\)-g/.r\1.g/' \
    || printf '0.0.0.r%s.g%s' "$(git rev-list --count HEAD)" "$(git rev-parse --short HEAD)"
}
```

- Exactly on `v0.1.0` → `git describe --tags` = `v0.1.0` → `0.1.0`.
- Five commits past the tag → `v0.1.0-5-gABCDEF` → `0.1.0.r5.gABCDEF` (monotonic; `rN` increases).
- The `|| printf …` keeps a safe fallback if no tag is reachable.

The static `pkgver=` default line is set to `0.1.0` and `pkgrel=1` (reset to 1 on every new `pkgver`).

### 4. CHANGELOG.md (new, Keep a Changelog)

Top-of-file `[Unreleased]` section (accumulated during development), then `[0.1.0] — 2026-07-09`. The 0.1.0
entry is a **concise summary of the shipped feature set**, not an enumeration of the ~1,000 pre-release commits.

## Release checklist (repeatable, for every future vX.Y.Z)

1. Move `[Unreleased]` → `[X.Y.Z] — <date>` in `CHANGELOG.md`; open a fresh empty `[Unreleased]`.
2. Bump `[workspace.package] version` in the root `Cargo.toml`.
3. Commit `release: vX.Y.Z`; annotated tag: `git tag -a vX.Y.Z -m "wcartel vX.Y.Z"`.
4. Build the artifact: `cargo build --profile release-dist -p wordcartel --bin wcartel`, then
   `cd packaging/arch && makepkg -f`.
5. Push commit + tag (`git push origin <branch> --follow-tags`), when ready.

## Cutting v0.1.0 now (this effort)

Edits (§1–§4) on branch `release-v0.1.0-versioning` → gates (`cargo build -p wordcartel`,
`cargo test --no-run -p wordcartel`, `cargo clippy --workspace --all-targets`, and a manual
`wcartel --version` check) → merge `--no-ff` to `main` → annotated tag `v0.1.0` on the merge commit →
full `release-dist` build + `makepkg -f` → push on request.

## Non-goals

- No crates.io publish (out-of-repo path dep; local Arch package only).
- No multi-distro / CI release automation (solo maintainer, single target).
- No conventional-commits / auto-generated changelog (commit trailers would make it noisy).
- No `panic = "abort"`, no profile changes (the shell's panic isolation requires unwinding).
