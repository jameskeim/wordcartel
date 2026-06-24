//! Generic single-line modal (spec §5.3). Reserved for destructive/ambiguous
//! decisions: quit-with-unsaved, external modification, swap recovery. Pure
//! data; the resolver (app.rs) interprets the chosen PromptAction.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PromptAction {
    Cancel,
    QuitAnyway,
    SaveAndQuit,
    Reload,
    Overwrite,
    Recover,
    DiscardSwap,
    OpenOriginal,
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
}

#[cfg(test)]
mod tests {
    use super::*;
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
