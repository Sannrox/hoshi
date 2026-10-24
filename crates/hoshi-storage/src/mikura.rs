//! Optional Mikura backend pinned to git tag v0.2.0.

use super::{Storage, decode_state_sqlite, encode_state_sqlite};
use crate::save::SaveError;
use hoshi_core::BalanceConfig;
use hoshi_core::Coord;
use hoshi_core::GameState;
use hoshi_core::{Fleet, ShipKind};
use mikura::{ObjectRecord, PropertyAcl, Store};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

const SNAPSHOT_KIND: &str = "Snapshot";
const SNAPSHOT_KEY: &str = "state";
const PLAYER_KIND: &str = "Player";
const PLAYER_KEY: &str = "player";
const PLANET_KIND: &str = "Planet";
const FLEET_KIND: &str = "Fleet";
const OUTPOST_KIND: &str = "Outpost";

pub struct MikuraStorage {
    pub(crate) store: Store,
    log: PathBuf,
}

impl MikuraStorage {
    pub fn open(path: &Path) -> Result<Self, SaveError> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let store = if path.exists() {
            Store::open(path).map_err(SaveError::Backend)?
        } else {
            Store::create(path).map_err(SaveError::Backend)?
        };
        Ok(Self {
            store,
            log: path.to_path_buf(),
        })
    }

    fn snapshot(&self) -> Result<Option<ObjectRecord>, SaveError> {
        match self
            .store
            .load(SNAPSHOT_KIND, SNAPSHOT_KEY, &PropertyAcl::allow_all())
        {
            Ok(record) => Ok(Some(record)),
            Err(msg) if msg.starts_with("unknown identity") => Ok(None),
            Err(msg) => Err(SaveError::Backend(msg)),
        }
    }
}

impl Storage for MikuraStorage {
    fn load(&self, config: BalanceConfig) -> Result<GameState, SaveError> {
        let record = self
            .snapshot()?
            .ok_or(SaveError::Corrupt("missing mikura snapshot"))?;
        let sidecar_id = sidecar_gen(&record)?;
        let bytes = std::fs::read(sidecar_path(&self.log, sidecar_id))
            .map_err(|_| SaveError::Corrupt("missing mikura snapshot blob"))?;
        decode_state_sqlite(&bytes, config)
    }

    fn save(&mut self, state: &GameState) -> Result<(), SaveError> {
        let previous = self.snapshot()?;
        if previous.is_some() {
            let committed = self.load(state.config.clone())?;
            if !state.cheats.preserves(&committed.cheats) {
                return Err(SaveError::Corrupt("stale cheat provenance"));
            }
        }
        let sidecar_id = previous
            .as_ref()
            .and_then(|record| sidecar_gen(record).ok())
            .unwrap_or(0)
            .saturating_add(1);
        let bytes = encode_state_sqlite(state)?;
        let sidecar = sidecar_path(&self.log, sidecar_id);
        write_atomic(&sidecar, &bytes)?;
        let mut records = object_records(state, bytes.len(), sidecar_id);
        let live: HashSet<(String, String)> = records
            .iter()
            .map(|r| (r.kind.clone(), r.key.clone()))
            .collect();
        if let Some(previous) = previous.as_ref()
            && let Some(index) = previous.props.get("index")
        {
            for id in parse_index(index) {
                if !live.contains(&id)
                    && let Ok(mut old) = self.store.load(&id.0, &id.1, &PropertyAcl::allow_all())
                {
                    old.hidden = true;
                    old.r#gen = 0;
                    records.push(old);
                }
            }
        }
        self.store
            .append_batch(records)
            .map_err(SaveError::Backend)?;
        // Keep the new sidecar even if a later reopen is needed: append_batch
        // may report an error after the object log already committed.
        remove_old_sidecars(&self.log, sidecar_id);
        Ok(())
    }

    fn is_empty(&self) -> Result<bool, SaveError> {
        Ok(self.snapshot()?.is_none())
    }
}

fn object_records(state: &GameState, sqlite_bytes: usize, sidecar_gen: u64) -> Vec<ObjectRecord> {
    let mut records = Vec::new();
    let mut index = Vec::new();
    push(
        &mut records,
        &mut index,
        PLAYER_KIND,
        PLAYER_KEY,
        [
            ("last_tick", state.last_tick_unix.to_string()),
            ("rng_seed", state.rng_seed.to_string()),
        ],
    );

    push_planet(
        &mut records,
        &mut index,
        state.home,
        state.home_kind.as_key(),
    );
    for colony in &state.colonies {
        push_planet(&mut records, &mut index, colony.coord, colony.kind.as_key());
    }

    push_fleet(
        &mut records,
        &mut index,
        &format!("parked:{}", state.home),
        &state.fleet,
        Some(state.home),
        None,
    );
    for colony in &state.colonies {
        if !colony.fleet.is_empty() {
            push_fleet(
                &mut records,
                &mut index,
                &format!("parked:{}", colony.coord),
                &colony.fleet,
                Some(colony.coord),
                None,
            );
        }
    }
    for mission in &state.missions {
        push_fleet(
            &mut records,
            &mut index,
            &format!("mission:{}", mission.id),
            &mission.ships,
            None,
            Some(mission.dest),
        );
    }

    for outpost in &state.outposts {
        let key = outpost.coord.to_string();
        push(
            &mut records,
            &mut index,
            OUTPOST_KIND,
            &key,
            [
                ("garrison", outpost.garrison.to_string()),
                ("kind", outpost.kind.as_key().to_string()),
            ],
        );
    }

    let mut snapshot_props = HashMap::new();
    snapshot_props.insert("bytes".into(), sqlite_bytes.to_string());
    snapshot_props.insert("sidecar".into(), sidecar_gen.to_string());
    snapshot_props.insert("index".into(), write_index(&index));
    records.insert(
        0,
        ObjectRecord {
            r#gen: 0,
            kind: SNAPSHOT_KIND.into(),
            key: SNAPSHOT_KEY.into(),
            hidden: false,
            action_id: None,
            props: snapshot_props,
        },
    );
    records
}

fn push_planet(
    records: &mut Vec<ObjectRecord>,
    index: &mut Vec<(String, String)>,
    coord: Coord,
    kind: &str,
) {
    let key = coord.to_string();
    push(
        records,
        index,
        PLANET_KIND,
        &key,
        [
            ("owner", PLAYER_KEY.to_string()),
            ("kind", kind.to_string()),
        ],
    );
}

fn push_fleet(
    records: &mut Vec<ObjectRecord>,
    index: &mut Vec<(String, String)>,
    key: &str,
    fleet: &Fleet,
    stationed: Option<Coord>,
    en_route: Option<Coord>,
) {
    let mut props = vec![("ships", fleet_csv(*fleet))];
    if let Some(at) = stationed {
        props.push(("stationed-at", at.to_string()));
    }
    if let Some(to) = en_route {
        props.push(("en-route-to", to.to_string()));
    }
    push(records, index, FLEET_KIND, key, props);
}

fn push(
    records: &mut Vec<ObjectRecord>,
    index: &mut Vec<(String, String)>,
    kind: &str,
    key: &str,
    props: impl IntoIterator<Item = (&'static str, String)>,
) {
    let mut map = HashMap::new();
    for (name, value) in props {
        map.insert(name.to_string(), value);
    }
    index.push((kind.to_string(), key.to_string()));
    records.push(ObjectRecord {
        r#gen: 0,
        kind: kind.into(),
        key: key.into(),
        hidden: false,
        action_id: None,
        props: map,
    });
}

fn fleet_csv(fleet: Fleet) -> String {
    let mut parts = Vec::new();
    for kind in ShipKind::ALL {
        let n = fleet.get(kind);
        if n > 0 {
            parts.push(format!("{}:{n}", kind.as_key()));
        }
    }
    parts.join(",")
}

fn write_index(ids: &[(String, String)]) -> String {
    ids.iter()
        .map(|(kind, key)| format!("{kind}\t{key}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_index(index: &str) -> Vec<(String, String)> {
    index
        .lines()
        .filter_map(|line| {
            let (kind, key) = line.split_once('\t')?;
            Some((kind.to_string(), key.to_string()))
        })
        .collect()
}

fn sidecar_gen(record: &ObjectRecord) -> Result<u64, SaveError> {
    record
        .props
        .get("sidecar")
        .ok_or(SaveError::Corrupt("missing mikura sidecar id"))?
        .parse()
        .map_err(|_| SaveError::Corrupt("mikura sidecar id"))
}

fn sidecar_path(log: &Path, sidecar_id: u64) -> PathBuf {
    let mut path = log.as_os_str().to_os_string();
    path.push(format!(".state.{sidecar_id}.sqlite"));
    PathBuf::from(path)
}

fn sidecar_prefix(log: &Path) -> Option<String> {
    Some(format!("{}.state.", log.file_name()?.to_str()?))
}

fn remove_old_sidecars(log: &Path, keep: u64) {
    let Some(prefix) = sidecar_prefix(log) else {
        return;
    };
    let keep_name = format!("{prefix}{keep}.sqlite");
    let parent = log.parent().filter(|p| !p.as_os_str().is_empty());
    let parent = parent.unwrap_or_else(|| Path::new("."));
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with(&prefix) && name != keep_name {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), SaveError> {
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::Storage;
    use super::*;
    use hoshi_core::Coord;
    use hoshi_core::ShipKind;
    use hoshi_core::{Clock, FakeClock};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_log() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "hoshi-mikura-objects-{}-{nanos}.mikura",
            std::process::id()
        ))
    }

    fn cleanup(path: &Path) {
        let _ = std::fs::remove_file(path);
        remove_old_sidecars(path, u64::MAX);
        let mut joins = path.as_os_str().to_os_string();
        joins.push(".joins");
        let mut delta = path.as_os_str().to_os_string();
        delta.push(".joins.delta");
        let _ = std::fs::remove_file(joins);
        let _ = std::fs::remove_file(delta);
    }

    #[test]
    fn mikura_writes_player_planet_fleet_outpost_links() {
        let path = unique_log();
        let mut store = MikuraStorage::open(&path).unwrap();
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        let now = 50u64;
        let mut state = GameState::with_config(now, cfg.clone());
        state.fleet.add(ShipKind::SmallCargo, 2);
        state.colonies.push(hoshi_core::Colony {
            coord: Coord {
                galaxy: 1,
                system: 1,
                position: 8,
            },
            kind: hoshi_core::PlanetType::Rocky,
            size: 8,
            rare: None,
            resources: hoshi_core::Resources::ZERO,
            buildings: hoshi_core::Buildings::default(),
            build_queue: Vec::new(),
            fleet: Fleet::default(),
            defences: hoshi_core::DefenceGrid::default(),
            goods: std::collections::BTreeMap::new(),
        });
        store.save(&state).unwrap();
        let acl = PropertyAcl::allow_all();
        let player = store.store.load(PLAYER_KIND, PLAYER_KEY, &acl).unwrap();
        assert_eq!(player.props.get("last_tick").unwrap(), "50");
        let home = store.store.load(PLANET_KIND, "1:1:1", &acl).unwrap();
        assert_eq!(home.props.get("owner").unwrap(), "player");
        let colony = store.store.load(PLANET_KIND, "1:1:8", &acl).unwrap();
        assert_eq!(colony.props.get("owner").unwrap(), "player");
        let fleet = store.store.load(FLEET_KIND, "parked:1:1:1", &acl).unwrap();
        assert_eq!(fleet.props.get("stationed-at").unwrap(), "1:1:1");
        let outpost_key = state.outposts[0].coord.to_string();
        let outpost = store.store.load(OUTPOST_KIND, &outpost_key, &acl).unwrap();
        assert!(outpost.props.contains_key("garrison"));
        drop(store);
        cleanup(&path);
    }

    #[test]
    fn mikura_in_flight_fleet_uses_en_route_to() {
        let path = unique_log();
        let mut store = MikuraStorage::open(&path).unwrap();
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        let now = 10u64;
        let mut state = GameState::with_config(now, cfg.clone());
        state.tech_levels.insert("colonization".into(), 1);
        state.resources.deuterium = 10_000.0;
        state.fleet.add(ShipKind::ColonyShip, 1);
        state
            .launch_colony(
                Coord {
                    galaxy: 1,
                    system: 1,
                    position: 14,
                },
                100,
                now,
            )
            .unwrap();
        store.save(&state).unwrap();
        let id = state.missions[0].id;
        let fleet = store
            .store
            .load(
                FLEET_KIND,
                &format!("mission:{id}"),
                &PropertyAcl::allow_all(),
            )
            .unwrap();
        assert_eq!(fleet.props.get("en-route-to").unwrap(), "1:1:14");
        drop(store);
        cleanup(&path);
    }

    #[test]
    fn mikura_catch_up_matches_clock() {
        let path = unique_log();
        let mut store = MikuraStorage::open(&path).unwrap();
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        let clock = FakeClock::new(1_000);
        let mut state = GameState::with_config(clock.now_unix(), cfg.clone());
        store.save(&state).unwrap();
        clock.advance(3_600);
        state.catch_up(clock.now_unix());
        store.save(&state).unwrap();
        let loaded = store.load(cfg).unwrap();
        assert_eq!(loaded.last_tick_unix, clock.now_unix());
        drop(store);
        cleanup(&path);
    }

    #[test]
    fn load_follows_the_committed_sidecar_not_a_newer_file() {
        let path = unique_log();
        let mut store = MikuraStorage::open(&path).unwrap();
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        let mut committed = GameState::with_config(10, cfg.clone());
        committed.resources.iron = 111.0;
        store.save(&committed).unwrap();
        let mut stray = committed.clone();
        stray.resources.iron = 999.0;
        let bytes = encode_state_sqlite(&stray).unwrap();
        write_atomic(&sidecar_path(&path, 99), &bytes).unwrap();
        let loaded = store.load(cfg).unwrap();
        assert!((loaded.resources.iron - 111.0).abs() < 1e-9);
        drop(store);
        cleanup(&path);
    }
}
