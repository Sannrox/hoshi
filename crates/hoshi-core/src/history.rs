//! Append-only structured history: search, lineage, snapshots and compaction.
//!
//! Retention: the last [`HISTORY_EVENT_CAP`] events and one empire snapshot per
//! event. Older rows drop on compact. Time-travel reconstructs only within that
//! window. The short TUI `event_log` (32 lines) is a separate feed.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::io::{self, Write};

/// Maximum retained structured events (and matching snapshots).
pub const HISTORY_EVENT_CAP: usize = 256;

/// Kind of recorded action or simulation event.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum HistoryKind {
    Build,
    Research,
    Ship,
    Fleet,
    Combat,
    Trade,
    Activity,
    Prestige,
    #[default]
    Other,
}

impl HistoryKind {
    pub const ALL: [HistoryKind; 9] = [
        HistoryKind::Build,
        HistoryKind::Research,
        HistoryKind::Ship,
        HistoryKind::Fleet,
        HistoryKind::Combat,
        HistoryKind::Trade,
        HistoryKind::Activity,
        HistoryKind::Prestige,
        HistoryKind::Other,
    ];

    pub fn as_key(self) -> &'static str {
        match self {
            HistoryKind::Build => "build",
            HistoryKind::Research => "research",
            HistoryKind::Ship => "ship",
            HistoryKind::Fleet => "fleet",
            HistoryKind::Combat => "combat",
            HistoryKind::Trade => "trade",
            HistoryKind::Activity => "activity",
            HistoryKind::Prestige => "prestige",
            HistoryKind::Other => "other",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "build" => HistoryKind::Build,
            "research" => HistoryKind::Research,
            "ship" => HistoryKind::Ship,
            "fleet" => HistoryKind::Fleet,
            "combat" => HistoryKind::Combat,
            "trade" => HistoryKind::Trade,
            "activity" => HistoryKind::Activity,
            "prestige" => HistoryKind::Prestige,
            "other" => HistoryKind::Other,
            _ => return None,
        })
    }
}

impl fmt::Display for HistoryKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_key())
    }
}

/// One append-only history row.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryEvent {
    pub id: u64,
    pub at: u64,
    pub kind: HistoryKind,
    pub actor: String,
    pub action: String,
    pub objects: Vec<String>,
    pub inputs: String,
    pub outcome: String,
    pub reason: String,
    pub cause_key: Option<String>,
    pub parent_ids: Vec<u64>,
    pub energy: f64,
    pub heat: f64,
    pub mass: f64,
}

impl HistoryEvent {
    /// One JSON object for replay export (JSON lines).
    pub fn to_json(&self) -> String {
        let objects = self
            .objects
            .iter()
            .map(|o| json_string(o))
            .collect::<Vec<_>>()
            .join(",");
        let parents = self
            .parent_ids
            .iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let cause = self
            .cause_key
            .as_deref()
            .map(json_string)
            .unwrap_or_else(|| "null".into());
        format!(
            "{{\"id\":{},\"at\":{},\"kind\":{},\"actor\":{},\"action\":{},\"objects\":[{objects}],\"inputs\":{},\"outcome\":{},\"reason\":{},\"cause_key\":{cause},\"parent_ids\":[{parents}],\"energy\":{},\"heat\":{},\"mass\":{}}}",
            self.id,
            self.at,
            json_string(self.kind.as_key()),
            json_string(&self.actor),
            json_string(&self.action),
            json_string(&self.inputs),
            json_string(&self.outcome),
            json_string(&self.reason),
            json_number(self.energy),
            json_number(self.heat),
            json_number(self.mass),
        )
    }
}

/// Recorded empire glance used for read-only time travel.
#[derive(Debug, Clone, PartialEq)]
pub struct EmpireSnapshot {
    pub at: u64,
    pub event_id: u64,
    pub iron: f64,
    pub silicon: f64,
    pub deuterium: f64,
    pub energy: f64,
    pub heat: f64,
    pub home_kind: String,
    pub building_levels: u32,
}

/// Fields supplied by callers; energy and heat are filled from live state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HistoryDraft {
    pub at: u64,
    pub kind: HistoryKind,
    pub actor: String,
    pub action: String,
    pub objects: Vec<String>,
    pub inputs: String,
    pub outcome: String,
    pub reason: String,
    pub parent_key: Option<String>,
    pub cause_key: Option<String>,
    pub mass: f64,
}

/// Search filters for TUI and CLI.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoryQuery {
    pub kind: Option<HistoryKind>,
    pub actor: Option<String>,
    pub object: Option<String>,
    pub from: Option<u64>,
    pub to: Option<u64>,
}

/// Bounded append-only log plus matching snapshots.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryLog {
    pub next_id: u64,
    pub events: Vec<HistoryEvent>,
    pub snapshots: Vec<EmpireSnapshot>,
    causes: BTreeMap<String, VecDeque<u64>>,
}

impl Default for HistoryLog {
    fn default() -> Self {
        Self {
            next_id: 1,
            events: Vec::new(),
            snapshots: Vec::new(),
            causes: BTreeMap::new(),
        }
    }
}

impl HistoryLog {
    pub fn record(&mut self, draft: HistoryDraft, mut snapshot: EmpireSnapshot) -> u64 {
        let parent_ids = draft
            .parent_key
            .as_deref()
            .and_then(|key| self.take_parent(key))
            .into_iter()
            .collect::<Vec<_>>();
        let id = self.next_id.max(1);
        self.next_id = id.saturating_add(1);
        if let Some(key) = draft.cause_key.as_deref() {
            self.causes
                .entry(key.to_string())
                .or_default()
                .push_back(id);
        }
        snapshot.at = draft.at;
        snapshot.event_id = id;
        self.events.push(HistoryEvent {
            id,
            at: draft.at,
            kind: draft.kind,
            actor: draft.actor,
            action: draft.action,
            objects: draft.objects,
            inputs: draft.inputs,
            outcome: draft.outcome,
            reason: draft.reason,
            cause_key: draft.cause_key,
            parent_ids,
            energy: snapshot.energy,
            heat: snapshot.heat,
            mass: draft.mass,
        });
        self.snapshots.push(snapshot);
        self.compact();
        id
    }

    pub fn search(&self, query: &HistoryQuery) -> Vec<&HistoryEvent> {
        self.events
            .iter()
            .filter(|event| query.matches(event))
            .collect()
    }

    /// Self plus ancestors via cause links, oldest first.
    pub fn lineage(&self, id: u64) -> Vec<&HistoryEvent> {
        let mut ids = BTreeSet::new();
        let mut stack = vec![id];
        while let Some(cur) = stack.pop() {
            if !ids.insert(cur) {
                continue;
            }
            if let Some(event) = self.events.iter().find(|e| e.id == cur) {
                stack.extend(event.parent_ids.iter().copied());
            }
        }
        self.events
            .iter()
            .filter(|event| ids.contains(&event.id))
            .collect()
    }

    /// Latest snapshot at or before `at`, if still retained.
    pub fn as_of(&self, at: u64) -> Option<&EmpireSnapshot> {
        self.snapshots.iter().rev().find(|snap| snap.at <= at)
    }

    pub fn export_jsonl(&self, mut writer: impl Write) -> io::Result<()> {
        for event in &self.events {
            writeln!(writer, "{}", event.to_json())?;
        }
        Ok(())
    }

    pub fn compact(&mut self) {
        if self.events.len() <= HISTORY_EVENT_CAP {
            return;
        }
        let drop = self.events.len() - HISTORY_EVENT_CAP;
        let dropped: BTreeSet<u64> = self.events.drain(..drop).map(|e| e.id).collect();
        self.snapshots
            .retain(|snap| !dropped.contains(&snap.event_id));
        for pending in self.causes.values_mut() {
            pending.retain(|id| !dropped.contains(id));
        }
        self.causes.retain(|_, pending| !pending.is_empty());
    }

    pub fn forget_cause(&mut self, key: &str) {
        if let Some(id) = self.take_parent(key) {
            self.clear_cause_key(id);
        }
    }

    pub fn forget_cause_at(&mut self, key: &str, index: usize) {
        let id = {
            let Some(pending) = self.causes.get_mut(key) else {
                return;
            };
            if index >= pending.len() {
                return;
            }
            pending.remove(index)
        };
        if let Some(id) = id {
            self.clear_cause_key(id);
        }
        if self
            .causes
            .get(key)
            .is_some_and(|pending| pending.is_empty())
        {
            self.causes.remove(key);
        }
    }

    pub fn rebuild_causes(&mut self) {
        let used: BTreeSet<u64> = self
            .events
            .iter()
            .flat_map(|event| event.parent_ids.iter().copied())
            .collect();
        self.causes.clear();
        for event in &self.events {
            if let Some(key) = event.cause_key.as_deref()
                && !used.contains(&event.id)
            {
                self.causes
                    .entry(key.to_string())
                    .or_default()
                    .push_back(event.id);
            }
        }
        if let Some(last) = self.events.last() {
            self.next_id = last.id.saturating_add(1);
        }
    }

    fn clear_cause_key(&mut self, id: u64) {
        if let Some(event) = self.events.iter_mut().find(|event| event.id == id) {
            event.cause_key = None;
        }
    }

    fn take_parent(&mut self, key: &str) -> Option<u64> {
        let pending = self.causes.get_mut(key)?;
        let id = pending.pop_front()?;
        if pending.is_empty() {
            self.causes.remove(key);
        }
        Some(id)
    }
}

impl HistoryQuery {
    fn matches(&self, event: &HistoryEvent) -> bool {
        if self.kind.is_some_and(|kind| event.kind != kind) {
            return false;
        }
        if let Some(actor) = self.actor.as_deref()
            && !actor.is_empty()
            && event.actor != actor
        {
            return false;
        }
        if let Some(object) = self.object.as_deref()
            && !object.is_empty()
            && !event.objects.iter().any(|item| item == object)
        {
            return false;
        }
        if self.from.is_some_and(|from| event.at < from) {
            return false;
        }
        if self.to.is_some_and(|to| event.at > to) {
            return false;
        }
        true
    }
}

pub fn join_csv(parts: &[String]) -> String {
    parts.join(",")
}

pub fn split_csv(raw: &str) -> Vec<String> {
    if raw.is_empty() {
        Vec::new()
    } else {
        raw.split(',').map(str::to_string).collect()
    }
}

pub fn join_ids(ids: &[u64]) -> String {
    ids.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

pub fn split_ids(raw: &str) -> Result<Vec<u64>, ()> {
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    raw.split(',')
        .map(|part| part.parse::<u64>().map_err(|_| ()))
        .collect()
}

fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn json_number(value: f64) -> String {
    if value.is_finite() {
        format!("{value}")
    } else {
        "0".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(at: u64, action: &str) -> HistoryDraft {
        HistoryDraft {
            at,
            kind: HistoryKind::Build,
            actor: "player".into(),
            action: action.into(),
            objects: vec!["home".into(), "metal_mine".into()],
            ..HistoryDraft::default()
        }
    }

    fn snap(at: u64, iron: f64, levels: u32) -> EmpireSnapshot {
        EmpireSnapshot {
            at,
            event_id: 0,
            iron,
            silicon: 1.0,
            deuterium: 0.0,
            energy: 10.0,
            heat: 2.0,
            home_kind: "rocky".into(),
            building_levels: levels,
        }
    }

    #[test]
    fn record_is_append_only_and_searchable() {
        let mut log = HistoryLog::default();
        log.record(draft(10, "queue"), snap(10, 100.0, 1));
        let mut research = draft(20, "start");
        research.kind = HistoryKind::Research;
        research.objects = vec!["energy".into()];
        log.record(research, snap(20, 90.0, 1));
        let hits = log.search(&HistoryQuery {
            kind: Some(HistoryKind::Build),
            object: Some("metal_mine".into()),
            from: Some(10),
            to: Some(10),
            ..HistoryQuery::default()
        });
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].action, "queue");
        assert_eq!(log.search(&HistoryQuery::default()).len(), 2);
    }

    #[test]
    fn lineage_walks_cause_links() {
        let mut log = HistoryLog::default();
        let mut queue = draft(1, "queue");
        queue.cause_key = Some("build:home:metal_mine".into());
        log.record(queue, snap(1, 50.0, 0));
        let mut complete = draft(40, "complete");
        complete.actor = "simulation".into();
        complete.parent_key = Some("build:home:metal_mine".into());
        let id = log.record(complete, snap(40, 50.0, 1));
        let chain = log.lineage(id);
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].action, "queue");
        assert_eq!(chain[1].action, "complete");
        assert_eq!(chain[1].parent_ids, vec![1]);
    }

    #[test]
    fn overlapping_jobs_keep_fifo_lineage() {
        let mut log = HistoryLog::default();
        let mut first = draft(1, "queue");
        first.cause_key = Some("build:home:metal_mine".into());
        log.record(first, snap(1, 50.0, 0));
        let mut second = draft(2, "queue");
        second.cause_key = Some("build:home:metal_mine".into());
        log.record(second, snap(2, 40.0, 0));
        let mut complete = draft(40, "complete");
        complete.actor = "simulation".into();
        complete.parent_key = Some("build:home:metal_mine".into());
        let id = log.record(complete, snap(40, 40.0, 1));
        assert_eq!(
            log.events.iter().find(|e| e.id == id).unwrap().parent_ids,
            vec![1]
        );
        let mut later = draft(80, "complete");
        later.actor = "simulation".into();
        later.parent_key = Some("build:home:metal_mine".into());
        let id = log.record(later, snap(80, 40.0, 2));
        assert_eq!(
            log.events.iter().find(|e| e.id == id).unwrap().parent_ids,
            vec![2]
        );
    }

    #[test]
    fn canceled_cause_does_not_return_after_rebuild() {
        let mut log = HistoryLog::default();
        let mut first = draft(1, "queue");
        first.cause_key = Some("build:home:metal_mine".into());
        log.record(first, snap(1, 50.0, 0));
        let mut second = draft(2, "queue");
        second.cause_key = Some("build:home:metal_mine".into());
        log.record(second, snap(2, 40.0, 0));
        log.forget_cause_at("build:home:metal_mine", 0);
        log.rebuild_causes();
        let mut complete = draft(40, "complete");
        complete.actor = "simulation".into();
        complete.parent_key = Some("build:home:metal_mine".into());
        let id = log.record(complete, snap(40, 40.0, 1));
        assert_eq!(
            log.events.iter().find(|e| e.id == id).unwrap().parent_ids,
            vec![2]
        );
    }

    #[test]
    fn as_of_matches_recorded_snapshot() {
        let mut log = HistoryLog::default();
        log.record(draft(5, "queue"), snap(5, 80.0, 2));
        log.record(draft(50, "complete"), snap(50, 80.0, 3));
        let past = log.as_of(5).expect("snapshot at 5");
        assert_eq!(past.iron, 80.0);
        assert_eq!(past.building_levels, 2);
        let later = log.as_of(50).expect("snapshot at 50");
        assert_eq!(later.building_levels, 3);
        assert!(log.as_of(0).is_none());
    }

    #[test]
    fn compact_drops_oldest_events_and_snapshots() {
        let mut log = HistoryLog::default();
        for i in 0..(HISTORY_EVENT_CAP as u64 + 20) {
            log.record(draft(i, "tick"), snap(i, i as f64, 0));
        }
        assert_eq!(log.events.len(), HISTORY_EVENT_CAP);
        assert_eq!(log.snapshots.len(), HISTORY_EVENT_CAP);
        assert_eq!(log.events[0].id, 21);
        assert!(log.as_of(0).is_none());
        assert_eq!(log.as_of(20).unwrap().event_id, 21);
    }

    #[test]
    fn export_jsonl_writes_one_object_per_event() {
        let mut log = HistoryLog::default();
        log.record(draft(3, "queue"), snap(3, 1.0, 0));
        let mut buf = Vec::new();
        log.export_jsonl(&mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(text.starts_with('{'));
        assert!(text.contains("\"action\":\"queue\""));
        assert!(text.ends_with('\n'));
    }
}
