//! Generic modal (spec §5.3, as amended by C5 — see below). Reserved for
//! destructive/ambiguous decisions: quit-with-unsaved, external modification, swap
//! recovery. Pure data; the resolver (app.rs) interprets the chosen PromptAction.
//!
//! **The question is one line; the disclosure is a box.** `message` is the single
//! status-row line carrying the question and its choice keys — that half of §5.3's
//! original "single-line modal" law is unchanged, and it is why the prompt keeps its one
//! truthful `RenderSite::StatusRow` entry in the H21 `OVERLAYS` table. C5 §11.3 needed a
//! prompt to *disclose* structured, multi-line information (which recovery files are being
//! spared, and how old they are), which a one-row string cannot hold: it was truncated at
//! the terminal width and the disclosure never reached the screen. `detail` is that seam —
//! typed lines painted in a bordered box above the status row. Prompts with an empty
//! `detail` render exactly as they always have.

/// How many kept-recoverable orphans `Prompt::clean_recovery` names before eliding the rest.
/// Bounded so a state dir full of orphans cannot push the modal's own choices off-screen.
const KEPT_SHOWN: usize = 5;

/// Render `ts_ms` (wall-clock milliseconds) as its age relative to `now_ms`, e.g. `3 days ago`.
///
/// Relative rather than a calendar date on purpose: the crate has no date/timezone dependency,
/// and an absolute stamp rendered in UTC would misreport the writer's own clock — the one
/// reading that would actively mislead. Age is also the question being asked ("is this old
/// enough that I no longer care?"). Coarsens upward through minutes/hours/days and never
/// panics: a `ts_ms` in the future (a clock that went backwards, a file copied from another
/// machine) saturates to `just now` rather than underflowing.
///
/// `format_age(10_000, 10_000)` is `"just now"`; `format_age(3 * 86_400_000, 0)` is
/// `"3 days ago"`. Stated as prose rather than as a doc example: this fn is private, so an
/// example block over it can never be compiled and would be an unchecked claim.
/// `format_age_coarsens_upward_and_never_underflows` is where those equalities are enforced.
fn format_age(now_ms: u64, ts_ms: u64) -> String {
    let secs = now_ms.saturating_sub(ts_ms) / 1_000;
    let plural = |n: u64, unit: &str| format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" });
    match secs {
        0..=59 => "just now".to_string(),
        60..=3_599 => plural(secs / 60, "minute"),
        3_600..=86_399 => plural(secs / 3_600, "hour"),
        _ => plural(secs / 86_400, "day"),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PromptAction {
    Cancel,
    QuitAnyway,
    SaveAndQuit,
    Reload,
    Overwrite,
    OverwriteExport,
    OverwriteSaveAs,
    OverwriteWriteBlock,
    Recover,
    DiscardSwap,
    OpenOriginal,
    Transform(crate::transform::TransformKind),
    /// Effort 6 multi-buffer quit: save every dirty buffer then quit.
    QuitSaveAll,
    /// Effort 6 multi-buffer quit: visit each dirty buffer with a per-buffer prompt.
    QuitReviewEach,
    /// Effort 6 review-each: save the buffer under review, then continue the drain.
    ReviewSave,
    /// Effort 6 review-each: discard the buffer under review, then continue the drain.
    ReviewDiscard,
    /// C4 close-buffer: save the target, then close it. The id is captured at
    /// raise time — background results can switch the active buffer under the
    /// prompt, so resolve must never read active().
    CloseSave { id: crate::editor::BufferId },
    /// C4 close-buffer: close the target without saving (the swap survives).
    CloseDiscard { id: crate::editor::BufferId },
    /// H5 clean-recovery: delete the `editor.pending_clean` snapshot built when the prompt was
    /// raised (never a re-scan — the snapshot is the TOCTOU-safe unit of deletion).
    CleanRecovery,
}

#[derive(Clone, Debug)]
pub struct Choice {
    pub key: char,
    pub label: &'static str,
    pub action: PromptAction,
}

#[derive(Clone, Debug)]
pub struct Prompt {
    /// The single status-row line: the question plus its bracketed choice keys.
    pub message: String,
    /// Optional structured disclosure, one entry per rendered line, painted as a bordered
    /// box directly ABOVE the status row (`render_overlays::paint_prompt_detail`).
    ///
    /// Empty for every prompt that has nothing to disclose, and an empty `detail` paints
    /// **nothing at all** — a prompt without one is byte-identical on screen to the
    /// pre-`detail` rendering. Structured lines belong here rather than `\n`-smuggled into
    /// `message`, which is truncated to one row and would silently swallow them (C5 review
    /// finding C1). The box's own height is bounded by the live frame, so a long `detail`
    /// can never push the choices off a short terminal.
    pub detail: Vec<String>,
    pub choices: Vec<Choice>,
}

impl Prompt {
    /// Map a typed key to its action (case-insensitive on the choice key).
    pub fn action_for(&self, ch: char) -> Option<PromptAction> {
        let lc = ch.to_ascii_lowercase();
        self.choices
            .iter()
            .find(|c| c.key.to_ascii_lowercase() == lc)
            .map(|c| c.action)
    }

    pub fn quit_confirm() -> Prompt {
        Prompt {
            message: "Unsaved changes: [S]ave & quit · [Q]uit anyway · [C]ancel".into(),
            detail: Vec::new(),
            choices: vec![
                Choice { key: 's', label: "Save & quit", action: PromptAction::SaveAndQuit },
                Choice { key: 'q', label: "Quit anyway", action: PromptAction::QuitAnyway },
                Choice { key: 'c', label: "Cancel",      action: PromptAction::Cancel },
            ],
        }
    }

    /// Effort 6 top-level multi-buffer quit prompt: N buffers have unsaved work.
    pub fn quit_multi(n: usize) -> Prompt {
        Prompt {
            message: format!("{n} buffer(s) unsaved: [A]ll save · [R]eview each · [C]ancel"),
            detail: Vec::new(),
            choices: vec![
                Choice { key: 'a', label: "Save all",    action: PromptAction::QuitSaveAll },
                Choice { key: 'r', label: "Review each",  action: PromptAction::QuitReviewEach },
                Choice { key: 'c', label: "Cancel",       action: PromptAction::Cancel },
            ],
        }
    }

    /// Effort 6 per-buffer review prompt raised while draining in Review-each mode.
    pub fn quit_review_buffer(name: &str) -> Prompt {
        Prompt {
            message: format!("{name}: [S]ave · [D]iscard · [C]ancel"),
            detail: Vec::new(),
            choices: vec![
                Choice { key: 's', label: "Save",    action: PromptAction::ReviewSave },
                Choice { key: 'd', label: "Discard", action: PromptAction::ReviewDiscard },
                Choice { key: 'c', label: "Cancel",  action: PromptAction::Cancel },
            ],
        }
    }

    pub fn external_mod() -> Prompt {
        Prompt {
            // Save-as ([S]) is deferred to Effort 5 — omitted from the choices.
            message: "File changed on disk: [R]eload · [O]verwrite  (Save-as: Effort 5)".into(),
            detail: Vec::new(),
            choices: vec![
                Choice { key: 'r', label: "Reload",    action: PromptAction::Reload },
                Choice { key: 'o', label: "Overwrite", action: PromptAction::Overwrite },
            ],
        }
    }

    pub fn swap_recovery() -> Prompt {
        Prompt {
            message: "Recovery file found: [R]ecover · [D]iscard · [O]pen original".into(),
            detail: Vec::new(),
            choices: vec![
                Choice { key: 'r', label: "Recover",       action: PromptAction::Recover },
                Choice { key: 'd', label: "Discard swap",  action: PromptAction::DiscardSwap },
                Choice { key: 'o', label: "Open original", action: PromptAction::OpenOriginal },
            ],
        }
    }

    pub fn transform_chooser() -> Prompt {
        use crate::transform::TransformKind;
        Prompt {
            message: "transform: [r]eflow  [u]nwrap  [v]entilate".into(),
            detail: Vec::new(),
            choices: vec![
                Choice { key: 'r', label: "Reflow",    action: PromptAction::Transform(TransformKind::Reflow) },
                Choice { key: 'u', label: "Unwrap",    action: PromptAction::Transform(TransformKind::Unwrap) },
                Choice { key: 'v', label: "Ventilate", action: PromptAction::Transform(TransformKind::Ventilate) },
            ],
        }
    }

    pub fn save_overwrite(target: &std::path::Path) -> Prompt {
        Prompt {
            message: format!("{} exists: [O]verwrite · [C]ancel", target.display()),
            detail: Vec::new(),
            choices: vec![
                Choice { key: 'o', label: "Overwrite", action: PromptAction::OverwriteSaveAs },
                Choice { key: 'c', label: "Cancel",    action: PromptAction::Cancel },
            ],
        }
    }

    pub fn write_block_overwrite(target: &std::path::Path) -> Prompt {
        Prompt {
            message: format!("{} exists: [O]verwrite · [C]ancel", target.display()),
            detail: Vec::new(),
            choices: vec![
                Choice { key: 'o', label: "Overwrite", action: PromptAction::OverwriteWriteBlock },
                Choice { key: 'c', label: "Cancel",    action: PromptAction::Cancel },
            ],
        }
    }

    pub fn export_overwrite(target: &std::path::Path) -> Prompt {
        Prompt {
            message: format!("{} exists: [O]verwrite · [C]ancel", target.display()),
            detail: Vec::new(),
            choices: vec![
                Choice { key: 'o', label: "Overwrite", action: PromptAction::OverwriteExport },
                Choice { key: 'c', label: "Cancel",    action: PromptAction::Cancel },
            ],
        }
    }

    /// H5 clean-recovery count-confirm, raised by the `clean_recovery` command over a
    /// snapshotted set of provably-valueless recovery files (`n` = the snapshot's length).
    ///
    /// `kept` names the orphaned swaps the sweep deliberately SPARED because they may still
    /// hold unsaved work (`swap::kept_recoverable`). Nothing in `kept` is ever deleted — it is
    /// disclosure, not a choice. Without it a writer saw a state dir that never fully empties
    /// and had no way to learn which documents were holding it open, or how old they were.
    ///
    /// The disclosure goes in `detail`, NOT in `message`: `message` is painted into the single
    /// status ROW, so the earlier `\n`-smuggled form was truncated at the terminal width and no
    /// realpath or age ever reached the writer (C5 review finding C1). Only the first
    /// `KEPT_SHOWN` are named — a list that grew without bound would drive the box past the
    /// frame — and the remainder is accounted for by an explicit elision line.
    ///
    /// `now_ms` is the caller's wall clock, threaded from the injected `Clock` like every other
    /// timed shell path — this constructor stays pure data so a `TestClock` journey renders a
    /// deterministic age.
    pub fn clean_recovery(n: usize, kept: &[crate::swap::KeptRecoverable], now_ms: u64) -> Prompt {
        let message = format!("Delete {n} recovery file(s)? [Y]es · [C]ancel");
        let mut detail: Vec<String> = Vec::new();
        if !kept.is_empty() {
            detail.push(format!("Keeping {} that may hold unsaved work:", kept.len()));
            for k in kept.iter().take(KEPT_SHOWN) {
                detail.push(format!("  {} (written {})", k.realpath, format_age(now_ms, k.ts_ms)));
            }
            if kept.len() > KEPT_SHOWN {
                detail.push(format!("  \u{2026}and {} more", kept.len() - KEPT_SHOWN));
            }
        }
        Prompt {
            message,
            detail,
            choices: vec![
                Choice { key: 'y', label: "Delete", action: PromptAction::CleanRecovery },
                Choice { key: 'c', label: "Cancel", action: PromptAction::Cancel },
            ],
        }
    }

    /// C4 close-confirm, raised when closing a dirty buffer (spec D1).
    pub fn close_confirm(name: &str, id: crate::editor::BufferId) -> Prompt {
        Prompt {
            message: format!("close {name}: [S]ave & close · [D]iscard · [C]ancel"),
            detail: Vec::new(),
            choices: vec![
                Choice { key: 's', label: "Save & close", action: PromptAction::CloseSave { id } },
                Choice { key: 'd', label: "Discard",      action: PromptAction::CloseDiscard { id } },
                Choice { key: 'c', label: "Cancel",       action: PromptAction::Cancel },
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_overwrite_action_is_distinct_from_save_overwrite() {
        let p = Prompt::export_overwrite(std::path::Path::new("/a/notes.html"));
        assert_eq!(p.action_for('o'), Some(PromptAction::OverwriteExport));
        assert_ne!(PromptAction::OverwriteExport, PromptAction::Overwrite);
    }

    #[test]
    fn quit_confirm_routes_keys_case_insensitively() {
        let p = Prompt::quit_confirm();
        assert_eq!(p.action_for('s'), Some(PromptAction::SaveAndQuit));
        assert_eq!(p.action_for('Q'), Some(PromptAction::QuitAnyway));
        assert_eq!(p.action_for('c'), Some(PromptAction::Cancel));
        assert_eq!(p.action_for('z'), None, "unmapped key returns None");
    }
    #[test]
    fn external_mod_offers_reload_overwrite_and_disabled_saveas() {
        let p = Prompt::external_mod();
        assert_eq!(p.action_for('r'), Some(PromptAction::Reload));
        assert_eq!(p.action_for('o'), Some(PromptAction::Overwrite));
        // Save-as is deferred to Effort 5: not an actionable choice in 4b.
        assert_eq!(p.action_for('s'), None);
        assert!(p.message.to_lowercase().contains("changed on disk"));
    }
    #[test]
    fn swap_recovery_offers_recover_discard_open() {
        let p = Prompt::swap_recovery();
        assert_eq!(p.action_for('r'), Some(PromptAction::Recover));
        assert_eq!(p.action_for('d'), Some(PromptAction::DiscardSwap));
        assert_eq!(p.action_for('o'), Some(PromptAction::OpenOriginal));
    }

    #[test]
    fn close_confirm_routes_keys_case_insensitively() {
        let id = crate::editor::BufferId(7);
        let p = Prompt::close_confirm("*a.md", id);
        assert_eq!(p.action_for('S'), Some(PromptAction::CloseSave { id }));
        assert_eq!(p.action_for('d'), Some(PromptAction::CloseDiscard { id }));
        assert_eq!(p.action_for('C'), Some(PromptAction::Cancel));
        assert_eq!(p.action_for('x'), None);
    }

    #[test]
    fn format_age_coarsens_upward_and_never_underflows() {
        assert_eq!(format_age(0, 0), "just now");
        assert_eq!(format_age(59_000, 0), "just now", "sub-minute is not worth a number");
        assert_eq!(format_age(60_000, 0), "1 minute ago", "singular, not '1 minutes'");
        assert_eq!(format_age(150_000, 0), "2 minutes ago");
        assert_eq!(format_age(3_600_000, 0), "1 hour ago");
        assert_eq!(format_age(7_200_000, 0), "2 hours ago");
        assert_eq!(format_age(86_400_000, 0), "1 day ago");
        assert_eq!(format_age(3 * 86_400_000, 0), "3 days ago");
        // A swap stamped in the FUTURE — a clock that went backwards, or a state dir copied
        // from another machine. Must saturate, not underflow into a nonsense age.
        assert_eq!(format_age(0, u64::MAX), "just now");
    }

    /// The DATA half of the kept-orphan disclosure: what `clean_recovery` composes, and the
    /// structural law that the question stays on one line while the disclosure goes to
    /// `detail`. It is deliberately **not** the proof that any of this is visible — that
    /// claim is only checkable against a drawn screen, and asserting it here is exactly the
    /// mistake finding C1 caught. The screen proofs are
    /// `render::tests::the_prompt_detail_box_actually_paints_the_realpath_and_the_age`,
    /// `..._paints_the_elision_line_past_the_cap`, and
    /// `prompts::tests::the_clean_recovery_modal_names_kept_recoverable_files`.
    #[test]
    fn clean_recovery_names_the_newest_kept_orphans_and_elides_the_rest() {
        // The modal is the ONLY place a writer learns a diverged swap was spared, so it must
        // name them — but bounded, or the disclosure box outgrows a short terminal. The list
        // arrives newest-first from `swap::kept_recoverable`; the elision line must account
        // for every one not shown.
        let kept: Vec<crate::swap::KeptRecoverable> = (0..KEPT_SHOWN + 3)
            .map(|i| crate::swap::KeptRecoverable {
                realpath: format!("/docs/chapter-{i}.md"), ts_ms: 0,
            }).collect();
        // The injected clock's reading is a parameter, so the age is deterministic and
        // assertable — three days after every fixture's `ts_ms: 0`.
        let p = Prompt::clean_recovery(4, &kept, 3 * 86_400_000);
        // The question and its choices stay a SINGLE line: that half of §5.3 is unchanged, and
        // it is what keeps the prompt's one `RenderSite::StatusRow` entry truthful.
        assert_eq!(p.message, "Delete 4 recovery file(s)? [Y]es · [C]ancel");
        let detail = p.detail.join("\n");
        assert!(detail.contains("(written 3 days ago)"),
            "each named orphan carries its age, stamped against the injected clock: {detail:?}");
        assert!(detail.contains(&format!("Keeping {} that may hold unsaved work", kept.len())),
            "the FULL kept count is stated even though only some are named: {detail:?}");
        for i in 0..KEPT_SHOWN {
            assert!(detail.contains(&format!("/docs/chapter-{i}.md")), "orphan {i} is named: {detail:?}");
        }
        assert!(!detail.contains(&format!("/docs/chapter-{}.md", KEPT_SHOWN)),
            "past the cap, orphans are elided rather than named: {detail:?}");
        assert!(detail.contains("\u{2026}and 3 more"),
            "and the elision accounts for exactly the unnamed remainder: {detail:?}");
        assert_eq!(p.detail.len(), KEPT_SHOWN + 2,
            "one header line, {KEPT_SHOWN} named orphans, one elision line — each its OWN entry, \
             never newlines inside one: {:?}", p.detail);
        // Disclosure never changes the decision surface.
        assert_eq!(p.action_for('y'), Some(PromptAction::CleanRecovery));
        assert_eq!(p.action_for('c'), Some(PromptAction::Cancel));

        let none = Prompt::clean_recovery(4, &[], 3 * 86_400_000);
        assert!(none.detail.is_empty(),
            "nothing spared → no disclosure at all, so no box is painted: {:?}", none.detail);
        assert!(!none.message.contains("Keeping"), "{:?}", none.message);
    }
}
