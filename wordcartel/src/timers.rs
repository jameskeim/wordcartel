//! Timed-subsystem hub. Static fn-pointer table; each subsystem's `deadline` embeds its
//! own in-flight/pending gate so a de-gated past-due Some can never reach recv_timeout(0)
//! (the swap-thrash / A3-spin class). Extracted from app.rs run()/reduce (Effort H1 r2).
//! Plugin-forward: the static slice upgrades to a `Vec<TimedSubsystem>` when Effort P needs
//! dynamic (plugin) timer registration; builtins stay plain fn pointers.
use crate::editor::Editor;
use crate::jobs::Executor;
use crate::registry::Ctx;
use crate::app::Msg;
use wordcartel_core::history::Clock;

// ---------------------------------------------------------------------------
// Save-timeout seam (extracted from run() so it is testable — C4 Task 2,
// relocated here in Effort H1 round 2).
// ---------------------------------------------------------------------------

/// Milliseconds before a pending save-then-act is considered overdue. Moved
/// from run()-local to module scope so `save_timeout_tick` can reference it
/// and tests can drive it without magic literals (C4 r2).
pub(crate) const SAVE_QUIT_TIMEOUT_MS: u64 = 5_000;

/// Save-timeout disposition (extracted from run()'s tick so it is testable — C4).
/// Returns without effect while no pending save is overdue.
pub(crate) fn save_timeout_tick(editor: &mut Editor, now: u64) {
    if let Some(p) = &editor.pending_after_save {
        let waited = now.saturating_sub(p.at_ms);
        if waited > SAVE_QUIT_TIMEOUT_MS {
            // Compiler-exhaustive on purpose (Codex plan r2): a future
            // PostSaveAction variant must NOT compile silently past this helper.
            let action = p.action.clone();
            editor.pending_after_save = None;
            match action {
                crate::editor::PostSaveAction::Quit => {
                    // Re-raise the quit-confirm modal so the user can choose again.
                    editor.open_prompt(crate::prompt::Prompt::quit_confirm());
                    editor.set_status_full(crate::status::StatusKind::Warning, "Save still running — choose again",
                        crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
                }
                crate::editor::PostSaveAction::ContinueQuitDrain => {
                    // Codex C3: a stranded drain (no in-flight save, no re-drive) would
                    // hang the quit. Abort the whole quit rather than silently clearing.
                    editor.quit_drain = None;
                    editor.quit_drain_advance = false;
                    editor.set_status_full(crate::status::StatusKind::Warning, "save timed out — quit cancelled",
                        crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
                }
                crate::editor::PostSaveAction::CloseBuffer { .. } => {
                    // C4: a close is not a session-ending action the user is
                    // waiting on — cancel without re-prompting (spec D3).
                    editor.set_status_full(crate::status::StatusKind::Warning, "save timed out — close cancelled",
                        crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The timed-subsystem table: one `deadline` fn per wake source, each carrying
// its own anti-spin gate. `next_wake` folds their min; idle → every fn is None
// → the run loop blocks on the 3600 s fallback (idle is free — §8.1-E).
// ---------------------------------------------------------------------------

/// One timed subsystem: a name (for guardrail tests and future plugin identity)
/// and a `deadline` fn that returns the next wall-clock ms this subsystem needs
/// the loop to wake at — or `None` when its work is not armed / is gated out.
pub(crate) struct TimedSubsystem {
    // Read only by the guardrail tests today (they select a subsystem by name) and reserved
    // as the stable plugin identity for Effort P; unread in a non-test release build.
    #[allow(dead_code)]
    pub(crate) name: &'static str,
    pub(crate) deadline: fn(&Editor, u64) -> Option<u64>,
}

/// Swap wake-up: armed ONLY when a swap is actually pending (unsaved content not
/// yet on disk) and none is in flight. Arming it off `last_edit_at` alone left a
/// permanently past-due deadline, so an idle buffer kept waking the loop to
/// rewrite its swap file — continuous disk I/O, not a CPU spin (see swap::pending
/// and the 2026-07-08 idle-thrash fix). The `!swap_in_flight` gate is what keeps
/// this None while a write is outstanding.
fn swap_deadline(e: &Editor, now: u64) -> Option<u64> {
    if crate::swap::pending(e.active().document.dirty(), e.active().document.version,
        e.active().swapped_version) && !e.active().swap_in_flight {
        crate::swap::next_deadline_ms(now, e.active().last_edit_at, e.active().last_swap_at)
    } else { None }
}

/// Save-then-quit/close overdue deadline: fires the 5 s guard so a stuck save
/// cannot wedge the quit forever.
fn sq_deadline(e: &Editor, _now: u64) -> Option<u64> {
    e.pending_after_save.as_ref().map(|p| p.at_ms.saturating_add(SAVE_QUIT_TIMEOUT_MS))
}

/// Scrollbar auto-fade: wake when the bar should fade (avoids relying on the idle
/// 1-hour Tick).
fn sb_deadline(e: &Editor, now: u64) -> Option<u64> {
    if e.mouse.scrollbar_until_ms > now { Some(e.mouse.scrollbar_until_ms) } else { None }
}

/// Menu-bar dwell/grace: at most one of reveal/hide is Some by construction (the
/// Moved arm clears the other side); recompute_menu_bar clears a fired due, so a
/// past deadline cannot persist and spin the loop.
fn menu_deadline(e: &Editor, _now: u64) -> Option<u64> {
    e.mouse.menu_reveal_due.or(e.mouse.menu_hide_due)
}

/// Scrollbar dwell deadline — armed by the right-edge Moved arm.
fn sb_dwell_deadline(e: &Editor, _now: u64) -> Option<u64> {
    e.mouse.scrollbar_reveal_due.or(e.mouse.scrollbar_hide_due)
}

/// Status-line dwell deadline — armed by the right-edge Moved arm.
fn status_dwell_deadline(e: &Editor, _now: u64) -> Option<u64> {
    e.mouse.status_reveal_due.or(e.mouse.status_hide_due)
}

/// Diagnostics recheck deadline. Fix A3: include it ONLY when no check is in
/// flight. When a check is in flight, `recheck_due_at` may be a past timestamp
/// (armed before the check started), which would drive recv_timeout(0) → 100%
/// CPU spin until the worker completes. When the result lands it clears
/// `in_flight_version`; the next iteration re-includes the (re-armed) deadline
/// and dispatches. The `in_flight_version.is_none()` gate is load-bearing.
/// E7 T2: also gated on `should_run_diagnostics` (draft-quiet) — an armed-but-stale
/// deadline left over from a buffer that has since left Review must not wake the loop
/// (the same spin class as the in-flight gate, one call site down).
fn diag_deadline(e: &Editor, _now: u64) -> Option<u64> {
    if crate::diagnostics_run::should_run_diagnostics(e) {
        e.active().diagnostics.due_deadline() // already excludes in-flight slots, per-slot
    } else { None }
}

/// Block-tree reconcile deadline — same A3 shape as diagnostics: excluded while a
/// reparse is in flight (`due_at` may be past-due, armed before the reparse), so
/// the `in_flight_version.is_none()` gate prevents a recv_timeout(0) spin.
fn reconcile_deadline(e: &Editor, _now: u64) -> Option<u64> {
    if e.active().reconcile.in_flight_version.is_none() {
        e.active().reconcile.due_at } else { None }
}

/// on_change debounce (P3 §6): the content-settled deadline, GATED on a subscriber so it is zero-cost
/// when no plugin uses on_change (proportional-to-work). Edge-armed by an edit (like reconcile),
/// self-clearing on fire — stays inside the idle-free law.
fn on_change_deadline(e: &Editor, _now: u64) -> Option<u64> {
    if e.has_on_change_subscriber { e.on_change_due } else { None }
}

/// Plugin-timer deadline (P3 §3): the soonest NON-pending armed timer's next-due. `None` when no timer
/// is armed (idle-free preserved). A `pending` timer is excluded (its callback is in flight — the
/// one-pending-per-timer rule; the same in-flight-gate shape as the builtin swap/diag/reconcile rows).
/// NOTE: a due timer's next-due may be in the past, so this can be < `now`; `run` uses `saturating_sub`
/// → one immediate wake, then the pump fires + reschedules to a future due (spec §4).
fn plugin_timer_deadline(e: &Editor, _now: u64) -> Option<u64> {
    e.pending_plugin_timers.iter().filter(|t| !t.pending).map(|t| t.next_due_ms).min()
}

/// The timed-subsystem table. Order = the run loop's historical fold order
/// (documented fire order): swap, save-quit, scrollbar-fade, menu-dwell,
/// scrollbar-dwell, status-dwell, diagnostics, reconcile, on_change, plugin_timer.
pub(crate) static SUBSYSTEMS: &[TimedSubsystem] = &[
    TimedSubsystem { name: "swap",         deadline: swap_deadline },
    TimedSubsystem { name: "save_quit",    deadline: sq_deadline },
    TimedSubsystem { name: "scrollbar",    deadline: sb_deadline },
    TimedSubsystem { name: "menu_dwell",   deadline: menu_deadline },
    TimedSubsystem { name: "sb_dwell",     deadline: sb_dwell_deadline },
    TimedSubsystem { name: "status_dwell", deadline: status_dwell_deadline },
    TimedSubsystem { name: "diagnostics",  deadline: diag_deadline },
    TimedSubsystem { name: "reconcile",    deadline: reconcile_deadline },
    TimedSubsystem { name: "on_change",    deadline: on_change_deadline },
    TimedSubsystem { name: "plugin_timer", deadline: plugin_timer_deadline },
];

/// The soonest wall-clock ms any timed subsystem needs the loop to wake — or
/// `None` when nothing is armed (idle is free; the loop blocks on the 3600 s
/// fallback). Each subsystem's own gate keeps a de-gated past-due Some out of
/// this fold, so idle ⇒ every term None ⇒ None.
pub(crate) fn next_wake(editor: &Editor, now: u64) -> Option<u64> {
    SUBSYSTEMS.iter().filter_map(|s| (s.deadline)(editor, now)).min()
}

/// Loop-top pre-recv hook: fire the save-then-act timeout guard before blocking
/// on the channel (the same fixed position `save_timeout_tick` held in run()).
pub(crate) fn pre_recv(editor: &mut Editor, now: u64) { save_timeout_tick(editor, now); }

/// The Tick-arm body: dispatch any timed work that is now due (swap write,
/// diagnostics recheck, block-tree reconcile). Verbatim transplant of reduce's
/// `Msg::Tick` arm — the per-dispatch `_due` predicates re-check the gate at fire
/// time, so a wake for one subsystem never fires another prematurely.
pub(crate) fn on_tick(editor: &mut Editor, ex: &dyn Executor, clock: &dyn Clock,
    msg_tx: &std::sync::mpsc::Sender<Msg>) {
    let now = clock.now_ms();
    if crate::swap::pending(
        editor.active().document.dirty(), editor.active().document.version, editor.active().swapped_version,
    )
        && !editor.active().swap_in_flight
        && crate::swap::due(now, editor.active().last_edit_at, editor.active().last_swap_at)
    {
        editor.active_mut().swap_in_flight = true;
        let mut ctx = Ctx { editor, clock, executor: ex, msg_tx: msg_tx.clone() };
        crate::swap::dispatch_swap_write(&mut ctx);
    }
    // Dispatch diagnostics if due.
    if crate::diagnostics_run::should_run_diagnostics(editor)
        && editor.active().diagnostics.any_due(now)
    {
        crate::diagnostics_run::dispatch_diagnostics(editor, now);
    }
    // Dispatch a block-tree reconcile if due.
    if crate::reconcile::reconcile_due(&editor.active().reconcile, now) {
        crate::reconcile::dispatch_reconcile(editor, ex);
    }
    // Fire the debounced on_change event if due (P3 §3g) — cold Tick path only, never the hot
    // edit path; `advance` only ever sets the `Option`, this is the sole place it fires.
    if editor.has_on_change_subscriber {
        if let Some(due) = editor.on_change_due {
            if now >= due {
                editor.on_change_due = None;
                let path = editor.active().document.path.clone();
                crate::plugin::fire_event(editor, crate::plugin::PluginEventKind::Change, path.as_deref());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use crate::editor::Editor;
    use wordcartel_core::diagnostics::DiagSource;

    // -----------------------------------------------------------------------
    // Save-timeout seam (relocated from app.rs — C4 Task 2)
    // -----------------------------------------------------------------------

    #[test]
    fn close_save_timeout_cancels_with_status() {
        // Drives the EXTRACTED helper directly (the timeout block lives in run(),
        // unreachable via reduce). Arrange a CloseBuffer pending at t=0; call
        // save_timeout_tick at SAVE_QUIT_TIMEOUT_MS+1 → pending cleared, status
        // "save timed out — close cancelled", buffer open, NO prompt.
        // Also pins the extraction is faithful: a Quit-variant pending re-raises
        // quit_confirm through the same helper.
        use crate::editor::{Editor, PostSaveAction, PendingAfterSave};
        let p = std::env::temp_dir().join(format!("wc-c4t2-timeout-{}.md", std::process::id()));
        std::fs::write(&p, "old\n").unwrap();
        let mut e = Editor::new_from_text("new\n", Some(p.clone()), (80, 24));
        e.active_mut().document.version = 1;
        e.active_mut().document.saved_version = None; // dirty
        let id = e.active().id;

        // Arm pending for a CloseBuffer action at t=0.
        e.pending_after_save = Some(PendingAfterSave {
            buffer_id: id, version: 1,
            action: PostSaveAction::CloseBuffer { id },
            at_ms: 0,
        });

        // Call the extracted helper at a time past the timeout.
        crate::timers::save_timeout_tick(&mut e, crate::timers::SAVE_QUIT_TIMEOUT_MS + 1);

        assert!(e.pending_after_save.is_none(), "pending cleared on CloseBuffer timeout");
        assert_eq!(e.status_text(), "save timed out — close cancelled");
        // A17 T5 (F4 Warning table): a Sticky Warning.
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Warning);
        assert_eq!(e.status().unwrap().lifetime(), crate::status::StatusLifetime::Sticky);
        assert!(e.by_id(id).is_some(), "buffer NOT closed — timeout only cancels");
        assert!(e.prompt.is_none(), "no re-prompt for a close timeout (spec D3)");

        // Fidelity pin: Quit-variant pending re-raises quit_confirm through the same helper.
        e.pending_after_save = Some(PendingAfterSave {
            buffer_id: id, version: 1,
            action: PostSaveAction::Quit,
            at_ms: 0,
        });
        crate::timers::save_timeout_tick(&mut e, crate::timers::SAVE_QUIT_TIMEOUT_MS + 1);
        assert!(e.pending_after_save.is_none(), "quit pending cleared");
        assert!(e.prompt.is_some(), "Quit timeout re-raises quit_confirm prompt");
        assert_eq!(e.status_text(), "Save still running — choose again");
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Warning);
        assert_eq!(e.status().unwrap().lifetime(), crate::status::StatusLifetime::Sticky);

        let _ = std::fs::remove_file(&p);
    }

    /// A17 T5 (F4 Warning table): a `ContinueQuitDrain` timeout aborts the drain and is a
    /// Sticky Warning ("save timed out — quit cancelled").
    #[test]
    fn quit_drain_save_timeout_cancels_with_sticky_warning() {
        use crate::editor::{Editor, PostSaveAction, PendingAfterSave, QuitDrain, QuitMode};
        let p = std::env::temp_dir().join(format!("wc-c4t2-drain-timeout-{}.md", std::process::id()));
        std::fs::write(&p, "old\n").unwrap();
        let mut e = Editor::new_from_text("new\n", Some(p.clone()), (80, 24));
        let id = e.active().id;
        e.quit_drain = Some(QuitDrain { queue: std::collections::VecDeque::from([id]), mode: QuitMode::SaveAll });
        e.pending_after_save = Some(PendingAfterSave {
            buffer_id: id, version: 1,
            action: PostSaveAction::ContinueQuitDrain,
            at_ms: 0,
        });
        crate::timers::save_timeout_tick(&mut e, crate::timers::SAVE_QUIT_TIMEOUT_MS + 1);
        assert!(e.pending_after_save.is_none(), "pending cleared on drain timeout");
        assert!(e.quit_drain.is_none(), "the whole quit is aborted, not just this pending");
        assert_eq!(e.status_text(), "save timed out — quit cancelled");
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Warning);
        assert_eq!(e.status().unwrap().lifetime(), crate::status::StatusLifetime::Sticky);
        let _ = std::fs::remove_file(&p);
    }

    // -----------------------------------------------------------------------
    // Timed-subsystem guardrails (T8)
    // -----------------------------------------------------------------------

    /// Draft-quiet (E7 T2): an armed diagnostics deadline must not wake the loop outside
    /// Review — the spin-class guardrail. Without the `should_run_diagnostics` gate in
    /// `diag_deadline`, a non-Review armed buffer would return the past-due `Some(400)`
    /// every loop iteration, driving `recv_timeout(0)` at 100% CPU (spec §2.2 site 5 / §8.1).
    #[test]
    fn armed_diag_deadline_is_none_outside_review() {
        use crate::editor::RenderMode;
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (40, 10));
        e.diag_cfg.enabled = true;
        e.active_mut().view.mode = RenderMode::LivePreview;
        e.active_mut().diagnostics.slot_mut(DiagSource::Harper).arm(0, 400); // recheck_due_at = Some(400), in_flight None
        assert_eq!(crate::timers::diag_deadline(&e, 10_000), None, "no wake for a non-Review armed store (no spin)");
        e.active_mut().view.mode = RenderMode::Review;
        assert_eq!(crate::timers::diag_deadline(&e, 10_000), Some(400), "Review: the armed deadline is live");
    }

    /// Idle-is-free: a clean, settled, no-overlay editor arms no wake (§8.1-E). This is the
    /// timers-native form of app.rs's settled_editor_arms_no_deadline pin.
    #[test]
    fn next_wake_none_when_settled() {
        let e = Editor::new_from_text("hello\n", None, (80, 24));
        assert!(!e.active().document.dirty());
        assert_eq!(crate::timers::next_wake(&e, 10_000), None);
    }

    /// Each named subsystem's in-flight/pending gate yields None when gated — generalizes
    /// diag_deadline_excluded_when_in_flight across the whole table (§8.1-E). CRITICAL: each
    /// subsystem is ARMED so its deadline would be Some WITHOUT its gate — the test must FAIL
    /// if a gate is deleted, not pass vacuously. All THREE in-flight/pending guards
    /// (swap `!swap_in_flight`, diag `in_flight_version.is_none()`, reconcile
    /// `in_flight_version.is_none()`) are proven load-bearing in BOTH directions here.
    #[test]
    fn gated_subsystems_yield_none() {
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        let diag = || crate::timers::SUBSYSTEMS.iter().find(|s| s.name == "diagnostics").unwrap().deadline;
        let reconcile = || crate::timers::SUBSYSTEMS.iter().find(|s| s.name == "reconcile").unwrap().deadline;
        let swap = || crate::timers::SUBSYSTEMS.iter().find(|s| s.name == "swap").unwrap().deadline;

        // E7 T2: diag_deadline is now also Review-gated; set Review so the un-gated
        // (in_flight None) case below still yields Some(0) — the in-flight assertion
        // above/below it still isolates that gate specifically.
        e.active_mut().view.mode = crate::editor::RenderMode::Review;

        // --- diagnostics: past-due recheck ARMED ---
        e.active_mut().diagnostics.slot_mut(DiagSource::Harper).recheck_due_at = Some(0);
        e.active_mut().diagnostics.slot_mut(DiagSource::Harper).in_flight_version = Some(1);
        assert_eq!((diag())(&e, 10_000), None, "diagnostics must be None while in-flight");
        // Un-gate (same due time): the deadline reappears — proving the gate suppressed it,
        // NOT that the work was simply unarmed. This assert FAILS if the diag guard is removed.
        e.active_mut().diagnostics.slot_mut(DiagSource::Harper).in_flight_version = None;
        assert_eq!((diag())(&e, 10_000), Some(0), "without the in-flight gate diag would be due");
        e.active_mut().diagnostics.slot_mut(DiagSource::Harper).in_flight_version = Some(1); // re-gate for the fold check below

        // --- reconcile: past-due ARMED ---
        e.active_mut().reconcile.due_at = Some(0);
        e.active_mut().reconcile.in_flight_version = Some(1);
        assert_eq!((reconcile())(&e, 10_000), None, "reconcile must be None while in-flight");
        e.active_mut().reconcile.in_flight_version = None;
        assert_eq!((reconcile())(&e, 10_000), Some(0), "without the in-flight gate reconcile would be due");
        e.active_mut().reconcile.in_flight_version = Some(1); // re-gate

        // --- swap: make the buffer DIRTY (version 1 != saved_version Some(0)) with swapped_version
        // None so swap::pending == true (swap.rs), and last_edit_at Some so next_deadline_ms would
        // return Some WITHOUT the gate — then a write in flight is the ONLY reason swap yields None.
        // new_from_text seeds saved_version Some(0)/version 0 (a clean buffer), so without this
        // arming swap::pending is already false and the gate is a no-op.
        e.active_mut().document.version = 1;          // dirty: 1 != saved_version Some(0)
        e.active_mut().last_edit_at = Some(0);         // arm next_deadline_ms
        // swapped_version stays None → pending true.
        assert!(crate::swap::pending(e.active().document.dirty(), e.active().document.version,
            e.active().swapped_version), "precondition: swap work is pending (else the gate is vacuous)");
        assert!(crate::swap::next_deadline_ms(10_000, e.active().last_edit_at, e.active().last_swap_at).is_some(),
            "precondition: WITHOUT the !swap_in_flight gate the swap deadline would be Some");
        // Un-gated swap yields Some; the in-flight gate is what suppresses it.
        assert!((swap())(&e, 10_000).is_some(), "without the !swap_in_flight gate swap would be due");
        e.active_mut().swap_in_flight = true;          // the gate under test

        for s in crate::timers::SUBSYSTEMS {
            if matches!(s.name, "diagnostics" | "reconcile") {
                assert_eq!((s.deadline)(&e, 10_000), None, "{} must be None while in-flight", s.name);
            }
        }
        assert_eq!((swap())(&e, 10_000), None,
            "swap must be None while a write is in flight (§8.1-E — the !swap_in_flight gate)");
    }

    // -----------------------------------------------------------------------
    // Effort P3: on_change / plugin_timer idle-free guardrails (Task 2). These
    // land BEFORE any timer-firing exists (Task 3), so they prove zero-cost-at-rest
    // against an EMPTY timer set / no subscriber — the load-bearing invariant.
    // -----------------------------------------------------------------------

    /// A fresh editor with the new P3 fields at baseline (no plugin timer armed,
    /// no on_change subscriber) must still yield next_wake == None — the two new
    /// rows must not disturb the pre-existing idle-free result.
    #[test]
    fn next_wake_none_with_commands_only_plugin() {
        let e = Editor::new_from_text("hello\n", None, (80, 24));
        assert!(e.pending_plugin_timers.is_empty());
        assert!(!e.has_on_change_subscriber);
        assert_eq!(e.on_change_due, None);
        assert_eq!(crate::timers::next_wake(&e, 10_000), None);
    }

    /// on_change_deadline is gated on has_on_change_subscriber — a due without a
    /// subscriber must not wake the loop (zero-cost when no plugin uses on_change).
    #[test]
    fn on_change_deadline_none_without_subscriber() {
        let on_change = || crate::timers::SUBSYSTEMS.iter().find(|s| s.name == "on_change").unwrap().deadline;
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        e.on_change_due = Some(400);
        assert!(!e.has_on_change_subscriber);
        assert_eq!((on_change())(&e, 10_000), None, "no subscriber ⇒ None despite an armed due");
        e.has_on_change_subscriber = true;
        assert_eq!((on_change())(&e, 10_000), Some(400), "flipping the subscriber flag arms the deadline");
    }

    /// plugin_timer_deadline reads the min next_due_ms across NON-pending timers only;
    /// a pending timer (callback in flight) is excluded — the same in-flight-gate shape
    /// as the builtin swap/diag/reconcile rows.
    #[test]
    fn plugin_timer_deadline_reads_min_nonpending() {
        use crate::plugin::PluginTimer;
        let plugin_timer = || crate::timers::SUBSYSTEMS.iter().find(|s| s.name == "plugin_timer").unwrap().deadline;
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        e.pending_plugin_timers.push(PluginTimer {
            handle: 1, origin: "p".into(), key: "wc-timer-1".into(),
            next_due_ms: 500, interval_ms: 1_000, repeat: false, pending: false,
        });
        e.pending_plugin_timers.push(PluginTimer {
            handle: 2, origin: "p".into(), key: "wc-timer-2".into(),
            next_due_ms: 300, interval_ms: 1_000, repeat: false, pending: false,
        });
        assert_eq!((plugin_timer())(&e, 10_000), Some(300), "min of two non-pending timers");

        // Mark the 300-due timer pending (in flight) — it must drop out, leaving 500.
        e.pending_plugin_timers[1].pending = true;
        assert_eq!((plugin_timer())(&e, 10_000), Some(500), "pending timer excluded from the fold");

        // Mark both pending — no armed (non-pending) timer remains ⇒ None (idle-free).
        e.pending_plugin_timers[0].pending = true;
        assert_eq!((plugin_timer())(&e, 10_000), None, "all timers pending ⇒ None (nothing armed)");
    }
}
