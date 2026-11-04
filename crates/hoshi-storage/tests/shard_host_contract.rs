//! Executable reference for a shard host: the call order in
//! `docs/chisei.md` ("Host contract") played against one in-process host.
//!
//! The host owns the clock, joins tenants under the cap, submits intents and
//! raid launches, settles, reads per-tenant views, and persists tenant saves
//! through `hoshi-storage` beside the bookkeeping and commons bytes. A host
//! rebuilt from those bytes must settle and read like one that never stopped.

use hoshi_core::{
    Actor, BalanceConfig, BuildingKind, Clock, Fleet, GameState, Intent, ShardBookkeeping,
    ShardCommand, ShardCommons, ShardConfig, ShardError, ShardMap, ShardRaid, ShardSession,
    ShipKind, load_tenants,
};
use hoshi_storage::{load_state, open_save, save_state};
use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};

const START: u64 = 1_000;
const NEWBIE_SECS: u64 = 3_600;
const KEY: [u8; 32] = [0x5a; 32];
const MAP: ShardMap = ShardMap {
    seed: 7,
    galaxies: 1,
    systems: 4,
    positions: 3,
};

/// Host clock the test moves; the session holds a handle to the same cell.
#[derive(Clone)]
struct HostClock(Rc<Cell<u64>>);

impl HostClock {
    fn set(&self, now: u64) {
        self.0.set(now);
    }
}

impl Clock for HostClock {
    fn now_unix(&self) -> u64 {
        self.0.get()
    }
}

/// One host process: its clock, its session, and the tenants it holds.
struct Host {
    clock: HostClock,
    shard: ShardSession<HostClock>,
    empires: BTreeMap<String, GameState>,
}

/// The rules every tenant save is loaded with; the save does not store them.
fn rules() -> BalanceConfig {
    let mut cfg = BalanceConfig::default();
    cfg.raid.enabled = false;
    cfg.moons.form_chance_per_debris = 0.0;
    cfg
}

fn config() -> ShardConfig {
    ShardConfig {
        newbie_secs: NEWBIE_SECS,
        tenant_cap: 2,
        ..ShardConfig::default()
    }
}

fn host_at(now: u64) -> Host {
    let clock = HostClock(Rc::new(Cell::new(now)));
    Host {
        shard: ShardSession::new(clock.clone(), KEY, config()),
        clock,
        empires: BTreeMap::new(),
    }
}

/// Create the shard and join `a` (raider) and `b` (defender) at `START`.
fn two_tenant_host() -> Host {
    let mut host = host_at(START);
    for identity in ["a", "b"] {
        host.shard
            .join(identity, MAP, &mut host.empires, |_| {
                GameState::with_config(START, rules())
            })
            .unwrap();
    }
    let a = host.empires.get_mut("a").unwrap();
    a.fleet = Fleet::default();
    a.fleet.add(ShipKind::Battleship, 20);
    a.resources.deuterium = 50_000.0;
    let b = host.empires.get_mut("b").unwrap();
    b.fleet = Fleet::default();
    b.fleet.add(ShipKind::LightFighter, 2);
    b.resources.iron = 40_000.0;
    host
}

/// A metal mine on `identity`'s home.
fn build(host: &Host, identity: &str, id: &str, client_now_unix: Option<u64>) -> ShardCommand {
    ShardCommand {
        identity: identity.into(),
        idempotency_id: id.into(),
        intent: Intent::Build {
            planet: host.empires[identity].home,
            building: BuildingKind::MetalMine,
            actor: Actor::Player,
        },
        client_now_unix,
    }
}

fn raid(host: &Host, id: &str) -> ShardRaid {
    ShardRaid {
        identity: "a".into(),
        idempotency_id: id.into(),
        ships: host.empires["a"].fleet,
        dest: host.empires["b"].home,
        speed_percent: 100,
    }
}

/// Submit for `identity` the way a host does: settle and retry on `Unsettled`.
fn submit(host: &mut Host, cmd: ShardCommand) -> Result<(), ShardError> {
    let identity = cmd.identity.clone();
    let state = host.empires.get_mut(&identity).unwrap();
    match host.shard.submit(state, cmd.clone()) {
        Err(ShardError::Unsettled { .. }) => {
            host.shard.settle(&mut host.empires);
            let state = host.empires.get_mut(&identity).unwrap();
            host.shard.submit(state, cmd)
        }
        other => other,
    }
}

/// Launch a's raid after the newbie window and return its arrival from b's view.
fn launch_after_newbie(host: &mut Host) -> u64 {
    host.clock.set(START + NEWBIE_SECS);
    let cmd = raid(host, "raid-1");
    host.shard.launch_raid(&mut host.empires, cmd).unwrap();
    let view = host.shard.view("b", &mut host.empires).unwrap();
    let [inbound] = view.inbound.as_slice() else {
        panic!("one inbound raid: {:?}", view.inbound);
    };
    inbound.arrives_at
}

#[test]
fn join_seats_two_tenants_on_one_seed_and_refuses_a_third_past_the_cap() {
    let mut host = two_tenant_host();
    assert_ne!(host.empires["a"].home, host.empires["b"].home);
    let before = host.empires.clone();
    assert_eq!(
        host.shard.join("c", MAP, &mut host.empires, |_| {
            unreachable!("a refused join builds nothing")
        }),
        Err(ShardError::ShardFull { cap: 2 })
    );
    assert_eq!(host.empires, before, "a refused join writes nothing");
    let a_home = host.empires["a"].home;
    assert_eq!(
        host.shard.join("a", MAP, &mut host.empires, |_| {
            unreachable!("a reconnect resumes the same tenant")
        }),
        Ok(a_home)
    );
}

#[test]
fn intent_at_a_fake_client_clock_applies_at_the_host_clock_and_only_to_its_tenant() {
    let mut host = two_tenant_host();
    host.clock.set(START + 60);
    let b_before = host.empires["b"].clone();
    let cmd = build(&host, "a", "mine-1", Some(START + 86_400));
    submit(&mut host, cmd).unwrap();
    let a = &host.empires["a"];
    assert_eq!(
        a.last_tick_unix,
        START + 60,
        "the host clock, not the client"
    );
    assert_eq!(a.build_queue.len(), 1);
    assert_eq!(host.empires["b"], b_before, "a's intent never touches b");
}

#[test]
fn duplicate_idempotency_id_applies_once() {
    let mut host = two_tenant_host();
    let cmd = build(&host, "a", "mine-1", None);
    submit(&mut host, cmd.clone()).unwrap();
    let once = host.empires["a"].clone();
    submit(&mut host, cmd).unwrap();
    assert_eq!(host.empires["a"], once);
}

#[test]
fn raid_on_a_new_defender_is_refused_then_flies_resolves_and_moves_plunder() {
    let mut host = two_tenant_host();
    let before = host.empires.clone();
    let cmd = raid(&host, "raid-early");
    assert_eq!(
        host.shard.launch_raid(&mut host.empires, cmd),
        Err(ShardError::NewbieProtected {
            identity: "b".into()
        })
    );
    assert_eq!(host.empires, before, "a refused raid launches nothing");

    let arrives_at = launch_after_newbie(&mut host);
    assert!(
        host.empires["b"].combat_logs.is_empty(),
        "nothing fights at launch"
    );
    host.clock.set(arrives_at);
    host.shard.settle(&mut host.empires);

    let view = host.shard.view("b", &mut host.empires).unwrap();
    let [aftermath] = view.raids.as_slice() else {
        panic!("one raid on b: {:?}", view.raids);
    };
    assert_eq!(aftermath.at, arrives_at);
    assert_eq!(aftermath.lost.get(ShipKind::LightFighter), 2);
    assert!(aftermath.plunder.iron > 0.0);
    let [mission] = host.empires["a"].missions.as_slice() else {
        panic!("a's survivors fly home");
    };
    assert_eq!(mission.cargo, aftermath.plunder, "plunder rides a's hold");
}

/// Per-test temp dir: pid plus a counter and the clock, removed on drop.
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
            "hoshi-shard-contract-{}-{nanos}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn save(&self, identity: &str) -> PathBuf {
        self.0.join(format!("{identity}.sqlite"))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// What a host keeps on disk: tenant saves plus the two opaque byte blobs.
struct Persisted {
    identities: Vec<String>,
    bookkeeping: Vec<u8>,
    commons: Vec<u8>,
}

fn persist(host: &mut Host, dir: &TempDir) -> Persisted {
    for (identity, state) in &host.empires {
        let mut conn = open_save(&dir.save(identity)).unwrap();
        save_state(&mut conn, state).unwrap();
    }
    Persisted {
        identities: host.empires.keys().cloned().collect(),
        bookkeeping: host.shard.export_bookkeeping().unwrap().as_bytes().to_vec(),
        commons: host.shard.commons().to_bytes().unwrap(),
    }
}

fn restore(persisted: Persisted, dir: &TempDir, now: u64) -> Host {
    let mut host = host_at(now);
    host.shard
        .restore_bookkeeping(&ShardBookkeeping::from_bytes(persisted.bookkeeping))
        .unwrap();
    *host.shard.commons_mut() = ShardCommons::from_bytes(&persisted.commons).unwrap();
    let (empires, failed) = load_tenants(persisted.identities, |identity| {
        let conn = open_save(&dir.save(identity))?;
        load_state(&conn, rules())
    });
    assert!(failed.is_empty(), "every tenant loads: {failed:?}");
    host.empires = empires;
    host
}

#[test]
fn restart_from_persisted_state_settles_and_reads_like_a_host_that_never_stopped() {
    // Same deterministic history on both hosts, with the raid still in flight.
    let mut steady = two_tenant_host();
    let mut stopped = two_tenant_host();
    let mut arrivals = Vec::new();
    for host in [&mut steady, &mut stopped] {
        let cmd = build(host, "b", "mine-1", None);
        submit(host, cmd).unwrap();
        arrivals.push(launch_after_newbie(host));
    }
    let [arrives_at, other] = arrivals[..] else {
        unreachable!()
    };
    assert_eq!(arrives_at, other);

    let dir = TempDir::new();
    let persisted = persist(&mut stopped, &dir);
    let restart_at = stopped.clock.now_unix();
    drop(stopped);
    let mut restarted = restore(persisted, &dir, restart_at);

    for (name, host) in [("steady", &mut steady), ("restarted", &mut restarted)] {
        host.clock.set(arrives_at);
        let fresh = build(host, "b", "mine-2", None);
        let retried = build(host, "b", "mine-1", None);
        let b = host.empires.get_mut("b").unwrap();
        assert_eq!(
            host.shard.submit(b, fresh),
            Err(ShardError::Unsettled {
                identity: "b".into()
            }),
            "{name}: the inbound raid survives the restart"
        );
        host.shard.settle(&mut host.empires);
        submit(host, retried).unwrap();
    }
    for identity in ["a", "b"] {
        assert_eq!(
            restarted.shard.view(identity, &mut restarted.empires),
            steady.shard.view(identity, &mut steady.empires),
            "{identity}'s view after restart"
        );
    }
    assert_eq!(
        restarted.empires["b"].build_queue, steady.empires["b"].build_queue,
        "the retried id stays applied once"
    );
    assert_eq!(restarted.shard.commons(), steady.shard.commons());
}
