//! Hoshi shard host on `sekai-source-sdk`.
//!
//! Game maths stay in [`hoshi_core::ShardSession`]. This crate claims
//! `external_mutate` writeback (`apply`), persists tenant saves, and posts a
//! signed tenant-view webhook. It is not a game server and has no sockets.

mod host;
mod webhook;

pub use host::{HostClock, HostParams, SOURCE, ShardHost, TYPE_DIGEST};
pub use webhook::{AdmitOutcome, DeliveryPlane, MemoryPlane, WebhookDelivery};

use sekai_source_sdk::{ConfigField, ConfigFieldKind, ConnectorType, CredentialSlot, Operation};

/// Out-of-tree connector type. The plane matches it by name and version.
pub const HOSHI_SHARD_CONNECTOR: ConnectorType = ConnectorType {
    name: "hoshi-shard",
    version: "1",
    fixed_source: Some(SOURCE),
    operations: &[Operation::Check, Operation::Apply],
    config: &[
        ConfigField {
            name: "seed",
            kind: ConfigFieldKind::Integer,
            required: true,
        },
        ConfigField {
            name: "tenant_cap",
            kind: ConfigFieldKind::Integer,
            required: false,
        },
    ],
    credential_slots: &[
        CredentialSlot {
            name: "signing_seed",
            required: true,
        },
        CredentialSlot {
            name: "bridge_mac",
            required: true,
        },
    ],
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::ShardHost;
    use hoshi_core::{ShardMap, ShipKind};
    use sekai_source_sdk::{Connector, ConnectorFailure, Operation, RecordChange};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    const START: u64 = 1_000;
    const NEWBIE_SECS: u64 = 3_600;
    const MAP: ShardMap = ShardMap {
        seed: 7,
        galaxies: 1,
        systems: 4,
        positions: 3,
    };

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "hoshi-connector-{}-{nanos}-{n}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn params(dir: &TempDir) -> HostParams {
        HostParams {
            persist_dir: dir.0.clone(),
            now: START,
            map: MAP,
            tenant_cap: 2,
            newbie_secs: NEWBIE_SECS,
            namespace: "galaxy".into(),
            source_instance: "shard-7".into(),
            producer_identity: "hoshi-connector".into(),
            signing_seed: [0x11; 32],
            bridge_mac: [0x5a; 32],
        }
    }

    fn host(dir: &TempDir) -> ShardHost<MemoryPlane> {
        ShardHost::create(params(dir), MemoryPlane::default()).unwrap()
    }

    struct FailAdmit {
        inner: MemoryPlane,
        error: Option<ConnectorFailure>,
    }

    impl DeliveryPlane for FailAdmit {
        fn admit(&mut self, delivery: &WebhookDelivery) -> Result<AdmitOutcome, ConnectorFailure> {
            if let Some(error) = self.error {
                return Err(error);
            }
            self.inner.admit(delivery)
        }
    }

    fn change(id: &str, key: &str, fields: &[(&str, &str)]) -> RecordChange {
        RecordChange {
            idempotency_key: id.into(),
            key: key.into(),
            expected_version: String::new(),
            change: fields
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
        }
    }

    fn join(id: &str, identity: &str) -> RecordChange {
        change(id, identity, &[("op", "join"), ("identity", identity)])
    }

    fn build(id: &str, identity: &str, planet: &str, client_now: Option<u64>) -> RecordChange {
        let mut cmd = change(
            id,
            identity,
            &[
                ("op", "intent"),
                ("identity", identity),
                ("kind", "build"),
                ("planet", planet),
                ("building", "metal_mine"),
            ],
        );
        if let Some(now) = client_now {
            cmd.change.insert("client_now_unix".into(), now.to_string());
        }
        cmd
    }

    fn two_tenants(dir: &TempDir) -> ShardHost<MemoryPlane> {
        let mut host = host(dir);
        host.apply(&join("join-a", "a")).unwrap();
        host.apply(&join("join-b", "b")).unwrap();
        host.park("a", ShipKind::Battleship, 20);
        host.stock_deuterium("a", 50_000.0);
        host.park("b", ShipKind::LightFighter, 2);
        host.stock_iron("b", 40_000.0);
        host.save_now();
        host
    }

    #[test]
    fn join_seats_two_and_refuses_a_third_past_the_cap() {
        let dir = TempDir::new();
        let mut host = host(&dir);
        host.apply(&join("join-a", "a")).unwrap();
        host.apply(&join("join-b", "b")).unwrap();
        assert_ne!(host.home("a"), host.home("b"));
        assert_eq!(
            host.apply(&join("join-c", "c")),
            Err(ConnectorFailure::Conflict)
        );
        assert!(host.object("c").is_none());
        let home = host.home("a");
        host.apply(&join("join-a-again", "a")).unwrap();
        assert_eq!(host.home("a"), home);
    }

    #[test]
    fn intent_ignores_client_clock_and_does_not_write_the_other_tenant() {
        let dir = TempDir::new();
        let mut host = two_tenants(&dir);
        host.set_now(START + 60);
        let b_iron = host.tenant_iron("b");
        let planet = host.home("a").to_string();
        host.apply(&build("mine-clock", "a", &planet, Some(START + 86_400)))
            .unwrap();
        assert_eq!(host.last_tick("a"), START + 60);
        assert_eq!(host.tenant_iron("b"), b_iron);
        let b = host.object("b").unwrap();
        assert_eq!(b["identity"], "b");
        assert!(!b.contains_key("signing_seed"));
        assert!(!b.contains_key("bridge_mac"));
        assert!(!host.object("a").unwrap().contains_key("rng"));
    }

    #[test]
    fn duplicate_apply_key_writes_once() {
        let dir = TempDir::new();
        let mut host = two_tenants(&dir);
        let planet = host.home("a").to_string();
        host.apply(&build("mine-1", "a", &planet, None)).unwrap();
        let queue = host.build_queue_len("a");
        let admits = host.plane().admits;
        host.apply(&build("mine-1", "a", &planet, None)).unwrap();
        assert_eq!(host.build_queue_len("a"), queue);
        assert_eq!(
            host.plane().admits,
            admits,
            "exact replay does not admit again"
        );
    }

    #[test]
    fn a_reused_key_with_a_different_change_is_refused() {
        let dir = TempDir::new();
        let mut host = two_tenants(&dir);
        let planet_a = host.home("a").to_string();
        host.apply(&build("mine-1", "a", &planet_a, None)).unwrap();
        let planet_b = host.home("b").to_string();
        assert_eq!(
            host.apply(&build("mine-1", "b", &planet_b, None)),
            Err(ConnectorFailure::Conflict)
        );
        assert_eq!(host.build_queue_len("b"), 0);
    }

    #[test]
    fn raid_on_a_new_defender_is_refused_then_plunder_moves_on_the_host() {
        let dir = TempDir::new();
        let mut host = two_tenants(&dir);
        let dest = host.home("b").to_string();
        let early = change(
            "raid-early",
            "a",
            &[
                ("op", "launch_raid"),
                ("identity", "a"),
                ("dest", dest.as_str()),
                ("ships", "battleship=20"),
                ("speed_percent", "100"),
            ],
        );
        assert_eq!(host.apply(&early), Err(ConnectorFailure::Conflict));
        assert!(
            host.object("b")
                .unwrap()
                .get("inbound_arrives_at")
                .is_none()
        );

        host.set_now(START + NEWBIE_SECS);
        let late = change(
            "raid-1",
            "a",
            &[
                ("op", "launch_raid"),
                ("identity", "a"),
                ("dest", dest.as_str()),
                ("ships", "battleship=20"),
                ("speed_percent", "100"),
            ],
        );
        host.apply(&late).unwrap();
        let arrives = host.object("b").unwrap()["inbound_arrives_at"]
            .parse::<u64>()
            .unwrap();
        host.set_now(arrives);
        host.apply(&change("settle-1", "shard", &[("op", "settle")]))
            .unwrap();
        let plunder: f64 = host.object("b").unwrap()["raid_plunder_iron"]
            .parse()
            .unwrap();
        assert!(plunder > 0.0);
        assert!(host.mission_cargo_iron("a").unwrap() > 0.0);
    }

    #[test]
    fn restart_resumes_the_same_session() {
        let dir = TempDir::new();
        let mut host = two_tenants(&dir);
        let planet = host.home("b").to_string();
        host.apply(&build("mine-b", "b", &planet, None)).unwrap();
        host.set_now(START + NEWBIE_SECS);
        let dest = host.home("b").to_string();
        host.apply(&change(
            "raid-1",
            "a",
            &[
                ("op", "launch_raid"),
                ("identity", "a"),
                ("dest", dest.as_str()),
                ("ships", "battleship=20"),
                ("speed_percent", "100"),
            ],
        ))
        .unwrap();
        let arrives = host.object("b").unwrap()["inbound_arrives_at"]
            .parse::<u64>()
            .unwrap();
        let now = host.now();
        drop(host);

        let mut restarted = ShardHost::restore(params(&dir), MemoryPlane::default()).unwrap();
        restarted.set_now(now);
        restarted
            .apply(&change("peek", "shard", &[("op", "settle")]))
            .unwrap();
        assert_eq!(
            restarted.object("b").unwrap()["inbound_arrives_at"],
            arrives.to_string(),
            "the inbound raid survives the restart"
        );
        restarted.set_now(arrives);
        restarted
            .apply(&change("settle-restart", "shard", &[("op", "settle")]))
            .unwrap();
        restarted
            .apply(&build("mine-fresh", "b", &planet, None))
            .unwrap();
        assert_eq!(restarted.last_tick("b"), arrives);
        assert_ne!(restarted.home("a"), restarted.home("b"));
        let plunder: f64 = restarted.object("b").unwrap()["raid_plunder_iron"]
            .parse()
            .unwrap();
        assert!(plunder > 0.0);
    }

    #[test]
    fn replayed_webhook_delivery_does_not_double_apply() {
        let dir = TempDir::new();
        let mut host = host(&dir);
        host.apply(&join("join-a", "a")).unwrap();
        let first = host.object("a").unwrap().clone();
        let admits = host.plane().admits;
        let delivery = host.plane().delivery("join-a").cloned().unwrap();
        assert_eq!(
            host.plane_mut().admit(&delivery),
            Ok(AdmitOutcome::ExactReplay)
        );
        assert_eq!(host.plane().admits, admits);
        assert_eq!(host.object("a").unwrap(), &first);
        host.apply(&join("join-a", "a")).unwrap();
        assert_eq!(host.plane().admits, admits);
        assert_eq!(host.object("a").unwrap(), &first);
    }

    #[test]
    fn a_mismatched_identity_field_is_refused() {
        let dir = TempDir::new();
        let mut host = host(&dir);
        host.apply(&join("join-a", "a")).unwrap();
        host.apply(&join("join-b", "b")).unwrap();
        let planet_b = host.home("b").to_string();
        let mut stolen = build("mine-stolen", "a", &planet_b, None);
        stolen.change.insert("identity".into(), "b".into());
        assert_eq!(host.apply(&stolen), Err(ConnectorFailure::Invalid));
        assert_eq!(host.build_queue_len("b"), 0);
    }

    #[test]
    fn cheat_intent_is_refused() {
        let dir = TempDir::new();
        let mut host = host(&dir);
        host.apply(&join("join-a", "a")).unwrap();
        let err = host.apply(&change(
            "cheat-1",
            "a",
            &[("op", "intent"), ("identity", "a"), ("kind", "cheat")],
        ));
        assert_eq!(err, Err(ConnectorFailure::Conflict));
    }

    #[test]
    fn a_stale_expected_version_is_refused() {
        let dir = TempDir::new();
        let mut host = host(&dir);
        let version = host.apply(&join("join-a", "a")).unwrap();
        host.set_now(START + 60);
        let planet = host.home("a").to_string();
        let mut stale = build("mine-stale", "a", &planet, None);
        stale.expected_version = "not-the-version".into();
        assert_eq!(host.apply(&stale), Err(ConnectorFailure::Conflict));
        let mut fresh = build("mine-fresh-version", "a", &planet, None);
        fresh.expected_version = version;
        host.apply(&fresh).unwrap();
        assert!(host.build_queue_len("a") >= 1);
    }

    #[test]
    fn another_tenants_apply_does_not_stale_this_tenants_version() {
        let dir = TempDir::new();
        let mut host = host(&dir);
        let version_a = host.apply(&join("join-a", "a")).unwrap();
        host.apply(&join("join-b", "b")).unwrap();
        host.set_now(START + 60);
        let planet_b = host.home("b").to_string();
        host.apply(&build("mine-b", "b", &planet_b, None)).unwrap();
        let planet_a = host.home("a").to_string();
        let mut mine_a = build("mine-a", "a", &planet_a, None);
        mine_a.expected_version = version_a;
        host.apply(&mine_a).unwrap();
        assert!(host.build_queue_len("a") >= 1);
    }

    #[test]
    fn a_failed_generation_commit_reverts_the_session() {
        let dir = TempDir::new();
        let mut host = host(&dir);
        std::fs::write(dir.0.join("staging"), b"blocked").unwrap();
        assert!(host.apply(&join("join-a", "a")).is_err());
        assert!(!host.seated("a"));
        std::fs::remove_file(dir.0.join("staging")).unwrap();
        host.apply(&join("join-a", "a")).unwrap();
        assert!(host.seated("a"));
    }

    #[test]
    fn delivery_id_that_cannot_round_trip_the_ack_file_is_refused() {
        let dir = TempDir::new();
        let mut host = host(&dir);
        assert_eq!(
            host.apply(&join("first\nsecond", "a")),
            Err(ConnectorFailure::Invalid)
        );
        assert!(!host.seated("a"));
    }

    #[test]
    fn a_second_host_cannot_open_the_same_persist_dir() {
        let dir = TempDir::new();
        let _host = host(&dir);
        assert_eq!(
            ShardHost::<MemoryPlane>::create(params(&dir), MemoryPlane::default()).map(|_| ()),
            Err(ConnectorFailure::Conflict)
        );
    }

    #[test]
    fn a_failed_delivery_blocks_the_next_mutation() {
        let dir = TempDir::new();
        let mut host = ShardHost::create(
            params(&dir),
            FailAdmit {
                inner: MemoryPlane::default(),
                error: Some(ConnectorFailure::Unreachable),
            },
        )
        .unwrap();
        assert_eq!(
            host.apply(&join("join-a", "a")),
            Err(ConnectorFailure::Unreachable)
        );
        assert!(host.seated("a"));
        assert_eq!(
            host.apply(&join("join-b", "b")),
            Err(ConnectorFailure::Unreachable)
        );
        assert!(!host.seated("b"));
        host.plane_mut().error = None;
        host.apply(&join("join-b", "b")).unwrap();
        assert!(host.seated("a"));
        assert!(host.seated("b"));
    }

    #[test]
    fn a_terminal_plane_refusal_returns_the_committed_version() {
        let dir = TempDir::new();
        let mut host = ShardHost::create(
            params(&dir),
            FailAdmit {
                inner: MemoryPlane::default(),
                error: Some(ConnectorFailure::Conflict),
            },
        )
        .unwrap();
        let version = host.apply(&join("join-a", "a")).unwrap();
        assert!(host.seated("a"));
        assert_eq!(host.apply(&join("join-a", "a")).unwrap(), version);
        host.plane_mut().error = None;
        host.apply(&join("join-b", "b")).unwrap();
        assert!(host.seated("b"));
    }

    #[test]
    fn identity_that_cannot_round_trip_the_identities_file_is_refused() {
        let dir = TempDir::new();
        let mut host = host(&dir);
        for (key, identity) in [
            ("nl", "a\nb"),
            ("lead", " a"),
            ("trail", "a "),
            ("colon", "a:b"),
            ("hash", "a#b"),
            ("slash", "a/b"),
            ("backslash", "a\\b"),
        ] {
            assert_eq!(
                host.apply(&join(key, identity)),
                Err(ConnectorFailure::Invalid),
                "{identity:?}"
            );
        }
        assert!(!host.seated("a"));
        assert!(!host.seated("b"));
    }

    #[test]
    fn view_source_version_follows_the_payload() {
        let dir = TempDir::new();
        let mut host = host(&dir);
        let version = host.apply(&join("join-a", "a")).unwrap();
        let first = host.plane().delivery("join-a").unwrap().records[0].clone();
        assert_eq!(version, first.source_version);
        let planet = host.home("a").to_string();
        host.apply(&build("mine-1", "a", &planet, None)).unwrap();
        let second = host.plane().delivery("mine-1").unwrap().records[0].clone();
        assert_ne!(first.source_version, second.source_version);
        assert_eq!(first.observed_at_ms, second.observed_at_ms);
        host.set_now(START + 10);
        assert_eq!(host.apply(&join("join-a", "a")).unwrap(), version);
    }

    #[test]
    fn restore_reads_the_previous_generation_when_current_is_missing() {
        let dir = TempDir::new();
        let mut host = host(&dir);
        host.apply(&join("join-a", "a")).unwrap();
        let home_a = host.home("a");
        host.apply(&join("join-b", "b")).unwrap();
        drop(host);
        std::fs::remove_dir_all(dir.0.join("current")).unwrap();
        let restored = ShardHost::restore(params(&dir), MemoryPlane::default()).unwrap();
        assert_eq!(restored.home("a"), home_a);
        assert!(!restored.seated("b"));
    }

    #[test]
    fn expired_unacked_delivery_is_resigned_with_the_same_id() {
        let dir = TempDir::new();
        let mut host = host(&dir);
        host.apply(&join("join-a", "a")).unwrap();
        let first = host.plane().delivery("join-a").cloned().unwrap();
        drop(host);
        std::fs::remove_file(dir.0.join("current").join("acked")).unwrap();
        std::fs::write(dir.0.join("current").join("pending"), "join-a").unwrap();
        let mut restarted = ShardHost::restore(params(&dir), MemoryPlane::default()).unwrap();
        restarted.set_now(START + 120);
        restarted.apply(&join("join-a", "a")).unwrap();
        let again = restarted.plane().delivery("join-a").cloned().unwrap();
        assert_eq!(again.delivery_id, first.delivery_id);
        assert_eq!(again.signed_at_ms, first.signed_at_ms);
        assert!(again.not_after_ms > first.not_after_ms);
        assert_eq!(restarted.plane().admits, 1);
    }

    #[test]
    fn connector_type_keeps_credentials_out_of_configuration() {
        for field in HOSHI_SHARD_CONNECTOR.config {
            let name = field.name;
            assert!(
                !["password", "secret", "token", "credential", "key"]
                    .iter()
                    .any(|word| name.contains(word)),
                "{name} belongs in a credential slot"
            );
        }
        assert!(HOSHI_SHARD_CONNECTOR.operations.contains(&Operation::Apply));
    }
}
