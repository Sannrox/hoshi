//! Opt-in web push for raid warnings and finished builds.
//!
//! Operator VAPID keys live in runtime config. They never enter the save.

use crate::notify::{self, Watch};
use hoshi_core::GameState;

/// Player opt-in plus operator VAPID material. Empty keys keep the channel off.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Settings {
    pub opt_in: bool,
    pub vapid_private: String,
    pub vapid_subject: String,
    pub endpoint: String,
}

impl Settings {
    pub fn from_config(cfg: &crate::config::UserConfig) -> Self {
        Self {
            opt_in: env_flag("HOSHI_SERVE_PUSH").unwrap_or(cfg.serve_push),
            vapid_private: env_nonempty("HOSHI_SERVE_VAPID_PRIVATE")
                .unwrap_or_else(|| cfg.serve_vapid_private.clone()),
            vapid_subject: env_nonempty("HOSHI_SERVE_VAPID_SUBJECT")
                .unwrap_or_else(|| cfg.serve_vapid_subject.clone()),
            endpoint: env_nonempty("HOSHI_SERVE_PUSH_ENDPOINT")
                .unwrap_or_else(|| cfg.serve_push_endpoint.clone()),
        }
    }

    pub fn armed(&self) -> bool {
        self.opt_in && !self.vapid_private.is_empty() && !self.vapid_subject.is_empty()
    }
}

fn env_flag(key: &str) -> Option<bool> {
    let value = std::env::var(key).ok()?;
    match value.as_str() {
        "1" | "true" | "TRUE" | "yes" => Some(true),
        "0" | "false" | "FALSE" | "no" => Some(false),
        _ => None,
    }
}

fn env_nonempty(key: &str) -> Option<String> {
    let value = std::env::var(key).ok()?;
    (!value.is_empty()).then_some(value)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Build,
    Raid,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Raid => "raid",
        }
    }
}

/// Public notice body. Hidden simulation stays off this object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Payload {
    pub kind: Kind,
    pub title: String,
    pub body: String,
}

impl Payload {
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "kind": self.kind.as_str(),
            "title": self.title,
            "body": self.body,
        })
    }
}

pub trait Transport {
    fn send(&mut self, payload: &serde_json::Value);
}

/// Production stand-in until a live HTTP POST exists. Decision logic still runs.
pub struct Noop;

impl Transport for Noop {
    fn send(&mut self, _payload: &serde_json::Value) {}
}

pub struct Gate<T: Transport> {
    settings: Settings,
    watch: Watch,
    transport: T,
}

impl<T: Transport> Gate<T> {
    pub fn new(settings: Settings, state: &GameState, transport: T) -> Self {
        Self {
            settings,
            watch: Watch::from_state(state),
            transport,
        }
    }

    pub fn armed(&self) -> bool {
        self.settings.armed()
    }

    pub fn poll(&mut self, state: &GameState, now: u64) {
        let notices = self.watch.observe(state, now);
        if !self.settings.armed() {
            return;
        }
        for notice in notices {
            let kind = match notice.kind {
                notify::Kind::Build => Kind::Build,
                notify::Kind::Raid => Kind::Raid,
                notify::Kind::Fleet | notify::Kind::Summary => continue,
            };
            let payload = Payload {
                kind,
                title: notice.title.clone(),
                body: notice.body.clone(),
            };
            self.transport.send(&payload.json());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoshi_core::{BuildingKind, Fleet, GameState, Mission, MissionPhase, ShipKind};

    #[derive(Default)]
    struct Recording {
        sent: Vec<serde_json::Value>,
    }

    impl Transport for Recording {
        fn send(&mut self, payload: &serde_json::Value) {
            self.sent.push(payload.clone());
        }
    }

    fn armed() -> Settings {
        Settings {
            opt_in: true,
            vapid_private: "test-vapid-private".into(),
            vapid_subject: "mailto:hoshi@localhost".into(),
            endpoint: "http://127.0.0.1/push".into(),
        }
    }

    #[test]
    fn finished_build_and_raid_warning_push_when_opt_in() {
        let mut state = GameState::new(1_000);
        let mut gate = Gate::new(armed(), &state, Recording::default());
        state.buildings.increment(BuildingKind::MetalMine);
        gate.poll(&state, 1_000);
        assert_eq!(gate.transport.sent.len(), 1);
        assert_eq!(gate.transport.sent[0]["kind"], "build");
        assert_eq!(gate.transport.sent[0]["body"], "build finished");

        state.next_raid_at = Some(1_050);
        gate.poll(&state, 1_000);
        assert_eq!(gate.transport.sent.len(), 2);
        let raid = &gate.transport.sent[1];
        assert_eq!(raid["kind"], "raid");
        let body = raid["body"].as_str().unwrap_or("");
        assert!(body.contains("raid incoming"), "{body}");
        let json = raid.to_string();
        assert!(!json.contains("next_raid_at"), "{json}");
        assert_eq!(raid["title"], "hoshi");
    }

    #[test]
    fn no_push_when_opt_in_off() {
        let mut state = GameState::new(1_000);
        let mut settings = armed();
        settings.opt_in = false;
        let mut gate = Gate::new(settings, &state, Recording::default());
        state.buildings.increment(BuildingKind::MetalMine);
        state.next_raid_at = Some(1_050);
        gate.poll(&state, 1_000);
        assert!(gate.transport.sent.is_empty());
    }

    #[test]
    fn no_push_when_vapid_keys_missing() {
        let mut state = GameState::new(1_000);
        let mut settings = armed();
        settings.vapid_private.clear();
        let mut gate = Gate::new(settings, &state, Recording::default());
        state.buildings.increment(BuildingKind::MetalMine);
        gate.poll(&state, 1_000);
        assert!(gate.transport.sent.is_empty());

        let mut settings = armed();
        settings.vapid_subject.clear();
        let mut gate = Gate::new(settings, &state, Recording::default());
        state.next_raid_at = Some(1_050);
        gate.poll(&state, 1_000);
        assert!(gate.transport.sent.is_empty());
    }

    #[test]
    fn fleet_return_does_not_push() {
        let mut state = GameState::new(1_000);
        let mut ships = Fleet::default();
        ships.add(ShipKind::LightFighter, 1);
        state.missions.push(Mission {
            id: 1,
            ships,
            origin: hoshi_core::Coord::HOME,
            dest: hoshi_core::Coord {
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
        });
        let mut gate = Gate::new(armed(), &state, Recording::default());
        state.missions.clear();
        gate.poll(&state, 1_100);
        assert!(gate.transport.sent.is_empty());
    }
}
