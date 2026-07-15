//! A17 — the one routed user-message model (shell-only; core has no status concept).
//! `Status` is the single value every user-facing message becomes; `resolve_slot` is the pure
//! Q1 severity-ranked slot rule; `StatusHistory` (Task 2) is the browsable ring.

/// User-message severity, mirroring LSP `window/showMessage` `MessageType` (Error=1 … Log=4).
/// Variant ORDER is most-severe first, so the derived `Ord` gives `Error < Warning < Info < Log` —
/// the MORE severe a kind, the SMALLER it compares. The Q1 rule is "candidate takes the slot iff
/// `candidate.kind <= occupant.kind`". This inversion is load-bearing; do NOT describe it as
/// "Error > Warning > …".
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum StatusKind { Error, Warning, Info, Log }

impl StatusKind {
    /// Parse the `wc.notify` / `[view] messages_min_kind` severity string. `None` on an unknown
    /// spelling (the caller surfaces a typed error — never a silent default).
    // Deliberately not `std::str::FromStr`: callers want `Option` (an unknown spelling is a
    // caller-surfaced typed error, not an `Err` payload) — same precedent as
    // `search_overlay.rs`'s inherent non-trait methods.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<StatusKind> {
        match s {
            "error"   => Some(StatusKind::Error),
            "warning" => Some(StatusKind::Warning),
            "info"    => Some(StatusKind::Info),
            "log"     => Some(StatusKind::Log),
            _ => None,
        }
    }
    /// The persisted / round-trip spelling (mirrors `config::transient_mode_str`).
    pub fn as_str(self) -> &'static str {
        match self {
            StatusKind::Error => "error", StatusKind::Warning => "warning",
            StatusKind::Info => "info",   StatusKind::Log => "log",
        }
    }
}

/// Orthogonal to severity. `Transient` clears on next input (Info/Log). `Sticky` holds until
/// dismissed or superseded (Warning/Error). `Progress` is held but expected to be superseded by its
/// own operation's completion (a `finish_topic` naming the same `StatusTopic`) and collapsed in
/// history; never traps (`Esc` dismisses, its completion always supersedes).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StatusLifetime { Transient, Sticky, Progress }

impl StatusLifetime {
    /// Default lifetime for a kind when the caller does not override.
    pub fn default_for(kind: StatusKind) -> StatusLifetime {
        match kind {
            StatusKind::Info | StatusKind::Log => StatusLifetime::Transient,
            StatusKind::Warning | StatusKind::Error => StatusLifetime::Sticky,
        }
    }
}

/// Correlation handle (spec §3.3). Instance-keyed for the one same-op-concurrent progress (Save);
/// static for the global single-slot progresses (Filter/Transform) and the singleton parse-degraded
/// state indicator. An EXACT-MATCH key: a Filter finish can never collapse a Save entry, and a Save
/// of (buffer B, version 7) can never collapse the Save of (buffer B, version 5).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StatusTopic {
    Save(crate::editor::BufferId, u64),
    Filter,
    Transform,
    ParseDegraded,
}

/// Where a message originated. There is NO stable plugin id in the shipped plugin system
/// (`plugin/host.rs::Bridge` holds only `InvokeState { current: Option<String>, … }`), so plugin
/// attribution is by that invocation LABEL, best-effort. Never a shadowing field.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum StatusSource { Host, Plugin { label: Option<String> } }

/// The one value every user-facing message becomes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Status {
    kind: StatusKind,
    text: String,
    lifetime: StatusLifetime,
    source: StatusSource,
    topic: Option<StatusTopic>,
    seq: u64,
    repeat: u32,
}

impl Status {
    /// Construct a message, capping `text` to `MESSAGES_MAX_TEXT_LEN` on a char boundary and
    /// stamping `repeat = 1`. `seq` is the caller's monotonic emit counter (history ordering + dedup).
    pub fn new(
        kind: StatusKind, text: impl Into<String>, lifetime: StatusLifetime,
        source: StatusSource, topic: Option<StatusTopic>, seq: u64,
    ) -> Status {
        let mut text = text.into();
        // Cap on a char boundary — never split a UTF-8 sequence (multibyte-safe).
        if text.len() > crate::limits::MESSAGES_MAX_TEXT_LEN {
            let mut end = crate::limits::MESSAGES_MAX_TEXT_LEN;
            while !text.is_char_boundary(end) { end -= 1; }
            text.truncate(end);
        }
        Status { kind, text, lifetime, source, topic, seq, repeat: 1 }
    }
    #[inline] pub fn kind(&self) -> StatusKind { self.kind }
    #[inline] pub fn text(&self) -> &str { &self.text }
    #[inline] pub fn lifetime(&self) -> StatusLifetime { self.lifetime }
    #[inline] pub fn source(&self) -> &StatusSource { &self.source }
    #[inline] pub fn topic(&self) -> Option<StatusTopic> { self.topic }
    #[inline] pub fn repeat(&self) -> u32 { self.repeat }
    // Task 2 wires this into StatusHistory's push/dedup path; unused until then.
    #[inline] #[allow(dead_code)] pub(crate) fn bump_repeat(&mut self) { self.repeat = self.repeat.saturating_add(1); }
}

/// The outcome of the Q1 slot rule for one candidate against the current occupant.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SlotOutcome { Take, HistoryOnly }

/// Pure Q1 precedence (spec §4.1). `floor` is `messages_min_kind` (verbosity floor). A candidate
/// strictly less severe than the floor is history-only. Otherwise it takes the slot iff there is no
/// occupant OR it is at least as severe as the occupant (`candidate.kind <= occupant.kind`).
pub fn resolve_slot(occupant: Option<&Status>, candidate: &Status, floor: StatusKind) -> SlotOutcome {
    if candidate.kind > floor {
        return SlotOutcome::HistoryOnly; // below the verbosity floor (more severe = smaller)
    }
    match occupant {
        None => SlotOutcome::Take,
        Some(occ) if candidate.kind <= occ.kind => SlotOutcome::Take,
        Some(_) => SlotOutcome::HistoryOnly,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::BufferId;

    fn s(kind: StatusKind) -> Status {
        Status::new(kind, "x", StatusLifetime::default_for(kind), StatusSource::Host, None, 1)
    }

    #[test]
    fn ord_is_error_smallest() {
        assert!(StatusKind::Error < StatusKind::Warning);
        assert!(StatusKind::Warning < StatusKind::Info);
        assert!(StatusKind::Info < StatusKind::Log);
    }

    #[test]
    fn empty_slot_always_takes() {
        // floor = Log clears every kind (Log is the least severe), so this exercises "empty slot
        // always takes" in isolation from the floor rule. NOTE: the brief/plan text for this test
        // used `StatusKind::Info` as the floor, which contradicts
        // `below_floor_is_history_only_even_on_empty_slot` below (identical call, opposite expected
        // outcome) and the spec (§4.1 step 2: "Log is always below an Info floor, so Log is
        // history-only by default"). Fixed here to `StatusKind::Log` so both tests are satisfiable;
        // flagged in the task report for human sign-off.
        let cand = s(StatusKind::Log);
        assert_eq!(resolve_slot(None, &cand, StatusKind::Log), SlotOutcome::Take);
    }

    #[test]
    fn equal_or_more_severe_takes_the_slot() {
        let occ = s(StatusKind::Warning);
        assert_eq!(resolve_slot(Some(&occ), &s(StatusKind::Warning), StatusKind::Info), SlotOutcome::Take);
        assert_eq!(resolve_slot(Some(&occ), &s(StatusKind::Error),   StatusKind::Info), SlotOutcome::Take);
    }

    #[test]
    fn less_severe_is_history_only() {
        let occ = s(StatusKind::Error);
        assert_eq!(resolve_slot(Some(&occ), &s(StatusKind::Info), StatusKind::Info), SlotOutcome::HistoryOnly);
    }

    #[test]
    fn below_floor_is_history_only_even_on_empty_slot() {
        // floor = Warning → an Info candidate is below the floor → history-only.
        assert_eq!(resolve_slot(None, &s(StatusKind::Info), StatusKind::Warning), SlotOutcome::HistoryOnly);
        // Log is always below an Info floor.
        assert_eq!(resolve_slot(None, &s(StatusKind::Log), StatusKind::Info), SlotOutcome::HistoryOnly);
    }

    #[test]
    fn from_str_round_trips() {
        assert_eq!(StatusKind::from_str("error"),   Some(StatusKind::Error));
        assert_eq!(StatusKind::from_str("warning"), Some(StatusKind::Warning));
        assert_eq!(StatusKind::from_str("info"),    Some(StatusKind::Info));
        assert_eq!(StatusKind::from_str("log"),     Some(StatusKind::Log));
        assert_eq!(StatusKind::from_str("bogus"),   None);
    }

    #[test]
    fn save_topic_instance_keys_differ_by_version() {
        let a = StatusTopic::Save(BufferId(1), 5);
        let b = StatusTopic::Save(BufferId(1), 6);
        assert_ne!(a, b);
        assert_eq!(a, StatusTopic::Save(BufferId(1), 5));
    }
}
