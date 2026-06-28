//! Generic single-line modal (spec §5.3). Reserved for destructive/ambiguous
//! decisions: quit-with-unsaved, external modification, swap recovery. Pure
//! data; the resolver (app.rs) interprets the chosen PromptAction.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PromptAction {
    Cancel,
    QuitAnyway,
    SaveAndQuit,
    /// Dirty-guard: save first, then perform the pending PostSaveAction.
    /// Distinct from SaveAndQuit (which is quit-confirm only).
    SaveAndProceed,
    /// Dirty-guard: discard unsaved changes, then perform the pending PostSaveAction.
    /// Distinct from QuitAnyway (which is quit-confirm only).
    DiscardAndProceed,
    Reload,
    Overwrite,
    OverwriteExport,
    OverwriteSaveAs,
    Recover,
    DiscardSwap,
    OpenOriginal,
    Transform(crate::transform::TransformKind),
}

#[derive(Clone, Debug)]
pub struct Choice {
    pub key: char,
    pub label: &'static str,
    pub action: PromptAction,
}

#[derive(Clone, Debug)]
pub struct Prompt {
    pub message: String,
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
            choices: vec![
                Choice { key: 's', label: "Save & quit", action: PromptAction::SaveAndQuit },
                Choice { key: 'q', label: "Quit anyway", action: PromptAction::QuitAnyway },
                Choice { key: 'c', label: "Cancel",      action: PromptAction::Cancel },
            ],
        }
    }

    pub fn external_mod() -> Prompt {
        Prompt {
            // Save-as ([S]) is deferred to Effort 5 — omitted from the choices.
            message: "File changed on disk: [R]eload · [O]verwrite  (Save-as: Effort 5)".into(),
            choices: vec![
                Choice { key: 'r', label: "Reload",    action: PromptAction::Reload },
                Choice { key: 'o', label: "Overwrite", action: PromptAction::Overwrite },
            ],
        }
    }

    pub fn swap_recovery() -> Prompt {
        Prompt {
            message: "Recovery file found: [R]ecover · [D]iscard · [O]pen original".into(),
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
            choices: vec![
                Choice { key: 'o', label: "Overwrite", action: PromptAction::OverwriteSaveAs },
                Choice { key: 'c', label: "Cancel",    action: PromptAction::Cancel },
            ],
        }
    }

    /// Dirty-guard modal: raised by New (and Open in Task 5) when the buffer has unsaved changes.
    /// Choices: [S]ave (save first, then proceed) · [D]iscard (drop changes, proceed) · [C]ancel.
    pub fn dirty_guard() -> Prompt {
        Prompt {
            message: "Unsaved changes: [S]ave · [D]iscard · [C]ancel".into(),
            choices: vec![
                Choice { key: 's', label: "Save",    action: PromptAction::SaveAndProceed },
                Choice { key: 'd', label: "Discard", action: PromptAction::DiscardAndProceed },
                Choice { key: 'c', label: "Cancel",  action: PromptAction::Cancel },
            ],
        }
    }

    pub fn export_overwrite(target: &std::path::Path) -> Prompt {
        Prompt {
            message: format!("{} exists: [O]verwrite · [C]ancel", target.display()),
            choices: vec![
                Choice { key: 'o', label: "Overwrite", action: PromptAction::OverwriteExport },
                Choice { key: 'c', label: "Cancel",    action: PromptAction::Cancel },
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
}
