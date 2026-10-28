//! Optional terminal bell and OSC notifications. Off by default.

use crate::title::AwaySummary;
use hoshi_core::{BuildingKind, GameState, MissionPhase};

const RAID_INCOMING_SECS: u64 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prefs {
    pub enabled: bool,
    pub build: bool,
    pub raid: bool,
    pub fleet: bool,
    pub quiet_start: Option<u8>,
    pub quiet_end: Option<u8>,
    pub bell: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            enabled: false,
            build: true,
            raid: true,
            fleet: true,
            quiet_start: None,
            quiet_end: None,
            bell: true,
        }
    }
}

impl Prefs {
    fn allows(self, kind: Kind) -> bool {
        match kind {
            Kind::Build => self.build,
            Kind::Raid => self.raid,
            Kind::Fleet => self.fleet,
            Kind::Summary => true,
        }
    }

    fn quiet_now(self, now_unix: u64) -> bool {
        match (self.quiet_start, self.quiet_end) {
            (Some(start), Some(end)) => in_quiet_hours(hour_utc(now_unix), start, end),
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Build,
    Raid,
    Fleet,
    Summary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub(crate) kind: Kind,
    pub(crate) title: String,
    pub(crate) body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Watch {
    buildings: u32,
    raid_at: Option<u64>,
    raid_warned: Option<u64>,
    returning: usize,
}

impl Watch {
    pub fn from_state(state: &GameState) -> Self {
        Self {
            buildings: building_sum(state),
            raid_at: state.upcoming_raid_at(),
            raid_warned: None,
            returning: returning_count(state),
        }
    }

    pub fn observe(&mut self, state: &GameState, now_unix: u64) -> Vec<Notice> {
        let mut out = Vec::new();
        let buildings = building_sum(state);
        if buildings > self.buildings {
            let n = buildings - self.buildings;
            out.push(Notice {
                kind: Kind::Build,
                title: "hoshi".into(),
                body: if n == 1 {
                    "build finished".into()
                } else {
                    format!("{n} builds finished")
                },
            });
        }
        self.buildings = buildings;

        let returning = returning_count(state);
        if returning < self.returning {
            let n = self.returning - returning;
            out.push(Notice {
                kind: Kind::Fleet,
                title: "hoshi".into(),
                body: if n == 1 {
                    "fleet returned".into()
                } else {
                    format!("{n} fleets returned")
                },
            });
        }
        self.returning = returning;

        if state.upcoming_raid_at() != self.raid_at {
            self.raid_warned = None;
        }
        self.raid_at = state.upcoming_raid_at();
        if let Some(at) = state.upcoming_raid_at()
            && now_unix < at
            && at - now_unix <= RAID_INCOMING_SECS
            && self.raid_warned != Some(at)
        {
            self.raid_warned = Some(at);
            out.push(Notice {
                kind: Kind::Raid,
                title: "hoshi".into(),
                body: format!("raid incoming in {}s", at - now_unix),
            });
        }
        out
    }
}

pub fn summarize_away(summary: &AwaySummary, prefs: Prefs) -> Option<Notice> {
    let mut parts = Vec::new();
    if prefs.build && summary.builds > 0 {
        parts.push(format!("{} builds", summary.builds));
    }
    let raids = summary.ledger.raids();
    if prefs.raid && raids > 0 {
        parts.push(format!("{raids} raids"));
    }
    let fleets = summary.ledger.fleets_returned;
    if prefs.fleet && fleets > 0 {
        parts.push(format!("{fleets} fleets"));
    }
    if parts.is_empty() {
        return None;
    }
    Some(Notice {
        kind: Kind::Summary,
        title: "hoshi".into(),
        body: format!("while away: {}", parts.join(", ")),
    })
}

pub fn encode(prefs: Prefs, now_unix: u64, notices: &[Notice]) -> Vec<u8> {
    if !prefs.enabled || prefs.quiet_now(now_unix) || notices.is_empty() {
        return Vec::new();
    }
    let mut kept: Vec<&Notice> = notices.iter().filter(|n| prefs.allows(n.kind)).collect();
    if kept.is_empty() {
        return Vec::new();
    }
    if kept.len() > 1 {
        let body = kept
            .iter()
            .map(|n| n.body.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        let summary = Notice {
            kind: Kind::Summary,
            title: "hoshi".into(),
            body,
        };
        return encode_one(prefs.bell, &summary);
    }
    encode_one(prefs.bell, kept.remove(0))
}

pub fn encode_one(bell: bool, notice: &Notice) -> Vec<u8> {
    let title = sanitize(&notice.title);
    let body = sanitize(&notice.body);
    let mut out = Vec::new();
    if bell {
        out.push(0x07);
    }
    out.extend_from_slice(b"\x1b]9;");
    out.extend_from_slice(body.as_bytes());
    out.push(0x07);
    out.extend_from_slice(b"\x1b]777;notify;");
    out.extend_from_slice(title.as_bytes());
    out.push(b';');
    out.extend_from_slice(body.as_bytes());
    out.extend_from_slice(b"\x1b\\");
    out
}

pub fn in_quiet_hours(hour: u8, start: u8, end: u8) -> bool {
    if start == end {
        return false;
    }
    if start < end {
        hour >= start && hour < end
    } else {
        hour >= start || hour < end
    }
}

fn hour_utc(unix: u64) -> u8 {
    ((unix % 86_400) / 3_600) as u8
}

fn building_sum(state: &GameState) -> u32 {
    let mut n = BuildingKind::ALL
        .iter()
        .map(|&k| state.buildings.get(k))
        .sum::<u32>();
    for colony in &state.colonies {
        n += BuildingKind::ALL
            .iter()
            .map(|&k| colony.buildings.get(k))
            .sum::<u32>();
    }
    n + state.completed_wonders.len() as u32
}

fn returning_count(state: &GameState) -> usize {
    state
        .missions
        .iter()
        .filter(|m| m.phase == MissionPhase::Return)
        .count()
}

fn sanitize(text: &str) -> String {
    text.chars()
        .filter(|c| *c >= ' ' && *c != '\u{7f}' && *c != '\u{1b}' && *c != ';')
        .take(120)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoshi_core::{Coord, Fleet, GameState, Mission, ShipKind};

    #[test]
    fn defaults_emit_nothing() {
        let mut state = GameState::new(1_000);
        let mut watch = Watch::from_state(&state);
        state.buildings.increment(BuildingKind::MetalMine);
        let notices = watch.observe(&state, 1_000);
        assert!(!notices.is_empty());
        assert!(encode(Prefs::default(), 1_000, &notices).is_empty());
    }

    #[test]
    fn payload_uses_bell_and_osc_and_strips_controls() {
        let notice = Notice {
            kind: Kind::Build,
            title: "ho\x1bshi".into(),
            body: "done;\x07ok".into(),
        };
        let bytes = encode_one(true, &notice);
        let text = String::from_utf8_lossy(&bytes);
        assert!(bytes.starts_with(&[0x07]), "{text}");
        assert!(text.contains("]9;doneok"), "{text}");
        assert!(text.contains("]777;notify;hoshi;doneok"), "{text}");
        assert!(!text.contains("\u{1b}shi"), "{text}");
    }

    #[test]
    fn quiet_hours_wrap_midnight() {
        assert!(in_quiet_hours(23, 22, 8));
        assert!(in_quiet_hours(3, 22, 8));
        assert!(!in_quiet_hours(10, 22, 8));
        assert!(in_quiet_hours(14, 12, 18));
        assert!(!in_quiet_hours(12, 12, 12));
        let prefs = Prefs {
            enabled: true,
            quiet_start: Some(22),
            quiet_end: Some(8),
            ..Prefs::default()
        };
        let notice = Notice {
            kind: Kind::Raid,
            title: "hoshi".into(),
            body: "raid incoming in 10s".into(),
        };
        assert!(encode(prefs, 23 * 3_600, std::slice::from_ref(&notice)).is_empty());
        assert!(!encode(prefs, 10 * 3_600, &[notice]).is_empty());
    }

    #[test]
    fn per_type_toggles_and_offline_summary() {
        let prefs = Prefs {
            enabled: true,
            raid: false,
            ..Prefs::default()
        };
        let raid = Notice {
            kind: Kind::Raid,
            title: "hoshi".into(),
            body: "raid incoming in 10s".into(),
        };
        assert!(encode(prefs, 0, &[raid]).is_empty());
        let mut ledger = hoshi_core::CatchUpLedger::default();
        ledger.fleets_returned = 1;
        ledger.raids_lost = 1;
        let summary = summarize_away(
            &AwaySummary {
                elapsed_secs: 3_600,
                builds: 2,
                ledger,
                ..AwaySummary::default()
            },
            prefs,
        )
        .unwrap();
        assert!(summary.body.contains("2 builds"), "{}", summary.body);
        assert!(!summary.body.contains("raid"), "{}", summary.body);
        assert!(summary.body.contains("1 fleets"), "{}", summary.body);
        let bytes = encode(prefs, 0, &[summary]);
        assert!(!bytes.is_empty());
    }

    #[test]
    fn watch_reports_build_raid_and_fleet_once() {
        let mut state = GameState::new(1_000);
        let mut watch = Watch::from_state(&state);
        state.buildings.increment(BuildingKind::MetalMine);
        let built = watch.observe(&state, 1_000);
        assert_eq!(built.len(), 1);
        assert_eq!(built[0].kind, Kind::Build);
        assert!(watch.observe(&state, 1_000).is_empty());

        state.next_raid_at = Some(1_050);
        let incoming = watch.observe(&state, 1_000);
        assert_eq!(incoming.len(), 1);
        assert_eq!(incoming[0].kind, Kind::Raid);
        assert!(watch.observe(&state, 1_010).is_empty());

        let mut ships = Fleet::default();
        ships.add(ShipKind::LightFighter, 1);
        state.missions.push(Mission {
            id: 1,
            ships,
            origin: Coord::HOME,
            dest: Coord {
                galaxy: 1,
                system: 1,
                position: 8,
            },
            speed_percent: 100,
            phase: MissionPhase::Return,
            launched_at: 1_000,
            outbound_secs: 10,
            hold_secs: 0,
            phase_ends_at: 1_100,
            fuel_paid: 0.0,
            colonize: false,
            expedition_seed: None,
            expedition_applied: false,
            cargo: hoshi_core::Resources::ZERO,
            transport: false,
            attack: false,
            invade: false,
            spy: false,
            site_id: None,
            recycle: false,
            deploy: false,
        });
        watch = Watch::from_state(&state);
        state.missions.clear();
        let fleet = watch.observe(&state, 1_100);
        assert_eq!(fleet.len(), 1);
        assert_eq!(fleet[0].kind, Kind::Fleet);
    }
}
