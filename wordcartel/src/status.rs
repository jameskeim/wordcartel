//! A17 — the one routed user-message model (shell-only; core has no status concept).
//! `Status` is the single value every user-facing message becomes; `resolve_slot` is the pure
//! Q1 severity-ranked slot rule; `StatusHistory` (Task 2) is the browsable ring.

use std::collections::VecDeque;

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
    #[inline] pub(crate) fn bump_repeat(&mut self) { self.repeat = self.repeat.saturating_add(1); }
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

/// Bounded in-memory ring of recent user messages (spec §5). M5 resource-cap ethos: fixed capacity,
/// oldest evicted, no growth at rest. Written only on an emit — never on a timer.
#[derive(Debug, Default)]
pub struct StatusHistory { entries: VecDeque<Status> }

impl StatusHistory {
    pub fn new() -> StatusHistory { StatusHistory { entries: VecDeque::new() } }
    pub fn entries(&self) -> &VecDeque<Status> { &self.entries }

    /// Append `msg`, coalescing an immediately-repeated identical message (spec §5.2). Evicts the
    /// oldest when at `MESSAGES_HISTORY_CAP`.
    pub fn push(&mut self, msg: Status) {
        if let Some(last) = self.entries.back_mut() {
            // Topic is part of the dedup key: two progress starts for DIFFERENT operations
            // (e.g. `Save(buf, v5)` and `Save(buf, v6)`) share the "Saving…" text but must stay
            // distinct history entries so each is collapsed by its own `finish_topic` — coalescing
            // them would strand the second lineage's start (A17 T6 concurrency-soundness).
            if last.kind == msg.kind && last.text == msg.text && last.source == msg.source
                && last.topic == msg.topic
                && msg.seq.saturating_sub(last.seq) <= crate::limits::MESSAGES_DEDUP_WINDOW
            {
                last.bump_repeat();
                return;
            }
        }
        self.entries.push_back(msg);
        while self.entries.len() > crate::limits::MESSAGES_HISTORY_CAP {
            self.entries.pop_front();
        }
    }

    /// Progress-completion collapse (spec §4.2): replace the most-recent entry whose `topic` exactly
    /// equals `topic` with `terminal` (in place — no append), else fall back to `push`. Exact-match on
    /// the full topic value (Save carries `(BufferId, version)`) makes a Filter finish unable to
    /// collapse a Save entry, and a same-buffer different-version Save unable to collapse another.
    pub fn collapse_topic(&mut self, topic: StatusTopic, terminal: Status) {
        if let Some(slot) = self.entries.iter_mut().rev().find(|s| s.topic == Some(topic)) {
            *slot = terminal;
        } else {
            self.push(terminal);
        }
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

    #[test]
    fn push_evicts_oldest_at_cap() {
        let mut h = StatusHistory::new();
        for i in 0..(crate::limits::MESSAGES_HISTORY_CAP + 10) as u64 {
            h.push(Status::new(StatusKind::Info, format!("m{i}"), StatusLifetime::Transient,
                               StatusSource::Host, None, i));
        }
        assert_eq!(h.entries().len(), crate::limits::MESSAGES_HISTORY_CAP);
        assert_eq!(h.entries().front().unwrap().text(), "m10"); // oldest 0..9 evicted
    }

    #[test]
    fn adjacent_identical_coalesces_repeat() {
        let mut h = StatusHistory::new();
        h.push(Status::new(StatusKind::Info, "loop", StatusLifetime::Transient, StatusSource::Host, None, 1));
        h.push(Status::new(StatusKind::Info, "loop", StatusLifetime::Transient, StatusSource::Host, None, 2));
        assert_eq!(h.entries().len(), 1);
        assert_eq!(h.entries().back().unwrap().repeat(), 2);
    }

    #[test]
    fn collapse_topic_replaces_matching_lineage_in_place() {
        use crate::editor::BufferId;
        let mut h = StatusHistory::new();
        let t = StatusTopic::Save(BufferId(1), 5);
        h.push(Status::new(StatusKind::Info, "Saving…", StatusLifetime::Progress, StatusSource::Host, Some(t), 1));
        h.push(Status::new(StatusKind::Info, "other",   StatusLifetime::Transient, StatusSource::Host, None, 2));
        let done = Status::new(StatusKind::Info, "Saved", StatusLifetime::Transient, StatusSource::Host, Some(t), 3);
        h.collapse_topic(t, done);
        // The "Saving…" entry was replaced in place by "Saved"; no new trailing entry appended.
        let texts: Vec<&str> = h.entries().iter().map(|s| s.text()).collect();
        assert_eq!(texts, vec!["Saved", "other"]);
    }

    #[test]
    fn collapse_topic_no_match_appends() {
        use crate::editor::BufferId;
        let mut h = StatusHistory::new();
        let done = Status::new(StatusKind::Info, "Saved", StatusLifetime::Transient, StatusSource::Host,
                               Some(StatusTopic::Save(BufferId(1), 5)), 1);
        h.collapse_topic(StatusTopic::Save(BufferId(1), 5), done);
        assert_eq!(h.entries().len(), 1);
    }
}
