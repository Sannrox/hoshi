//! Persistence behind a trait. Game rules never open a database.

use crate::balance::BalanceConfig;
use crate::save::{
    SaveError, load_or_new, load_or_new_with_seed, load_state, open_save, save_state,
};
use crate::state::GameState;
use rusqlite::Connection;
use std::ffi::OsStr;
use std::path::Path;

#[cfg(feature = "mikura")]
mod mikura;

#[cfg(feature = "mikura")]
pub use mikura::MikuraStorage;

#[cfg(feature = "chisei")]
mod chisei;

#[cfg(feature = "chisei")]
pub use chisei::ChiseiStorage;

/// How a save path picks a backend. SQLite is the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    Sqlite,
    Mikura,
    Chisei,
}

impl BackendKind {
    pub fn from_path(path: &Path) -> Self {
        match path.extension().and_then(OsStr::to_str) {
            Some(ext) if ext.eq_ignore_ascii_case("mikura") => Self::Mikura,
            Some(ext) if ext.eq_ignore_ascii_case("chisei") => Self::Chisei,
            _ => Self::Sqlite,
        }
    }
}

/// Load and save a [`GameState`]. Implementations persist atomically.
pub trait Storage {
    fn load(&self, config: BalanceConfig) -> Result<GameState, SaveError>;
    fn save(&mut self, state: &GameState) -> Result<(), SaveError>;

    /// Persist every object change from `state` in one atomic write.
    fn apply_batch(&mut self, state: &GameState) -> Result<(), SaveError> {
        self.save(state)
    }

    fn is_empty(&self) -> Result<bool, SaveError>;

    fn load_or_new(
        &mut self,
        now_unix: u64,
        config: BalanceConfig,
    ) -> Result<GameState, SaveError> {
        if self.is_empty()? {
            let mut state = GameState::with_config(now_unix, config);
            state.tutorial = crate::TutorialBoard::start();
            self.save(&state)?;
            Ok(state)
        } else {
            self.load(config)
        }
    }

    fn load_or_new_with_seed(
        &mut self,
        now_unix: u64,
        config: BalanceConfig,
        seed: u64,
    ) -> Result<GameState, SaveError> {
        if self.is_empty()? {
            let state = GameState::with_seed(now_unix, config, seed);
            self.save(&state)?;
            Ok(state)
        } else {
            self.load(config)
        }
    }
}

/// Existing SQLite save, unchanged on disk.
pub struct SqliteStorage {
    conn: Connection,
}

impl SqliteStorage {
    pub fn open(path: &Path) -> Result<Self, SaveError> {
        Ok(Self {
            conn: open_save(path)?,
        })
    }
}

impl Storage for SqliteStorage {
    fn load(&self, config: BalanceConfig) -> Result<GameState, SaveError> {
        load_state(&self.conn, config)
    }

    fn save(&mut self, state: &GameState) -> Result<(), SaveError> {
        save_state(&mut self.conn, state)
    }

    fn is_empty(&self) -> Result<bool, SaveError> {
        let meta: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM meta", [], |row| row.get(0))?;
        Ok(meta == 0)
    }

    fn load_or_new(
        &mut self,
        now_unix: u64,
        config: BalanceConfig,
    ) -> Result<GameState, SaveError> {
        load_or_new(&mut self.conn, now_unix, config)
    }

    fn load_or_new_with_seed(
        &mut self,
        now_unix: u64,
        config: BalanceConfig,
        seed: u64,
    ) -> Result<GameState, SaveError> {
        load_or_new_with_seed(&mut self.conn, now_unix, config, seed)
    }
}

/// Open the backend implied by `path`. `.mikura` needs `--features mikura`.
/// `.chisei` needs `--features chisei`.
pub fn open_storage(path: &Path) -> Result<Box<dyn Storage>, SaveError> {
    match BackendKind::from_path(path) {
        BackendKind::Sqlite => Ok(Box::new(SqliteStorage::open(path)?)),
        BackendKind::Mikura => {
            #[cfg(feature = "mikura")]
            {
                Ok(Box::new(MikuraStorage::open(path)?))
            }
            #[cfg(not(feature = "mikura"))]
            {
                Err(SaveError::Backend(
                    "rebuild with --features mikura to open a .mikura save".into(),
                ))
            }
        }
        BackendKind::Chisei => {
            #[cfg(feature = "chisei")]
            {
                Ok(Box::new(ChiseiStorage::open(path)?))
            }
            #[cfg(not(feature = "chisei"))]
            {
                Err(SaveError::Backend(
                    "rebuild with --features chisei to open a .chisei save".into(),
                ))
            }
        }
    }
}

pub(crate) fn inspect_backend_save(path: &Path) -> Result<crate::SaveGlance, SaveError> {
    if crate::save_is_locked(path) {
        return Err(SaveError::Locked);
    }
    let state = load_backend_state(path)?;
    Ok(crate::SaveGlance {
        schema: crate::SCHEMA_VERSION,
        last_tick_unix: state.last_tick_unix,
        deterministic: state.deterministic,
        planets: u32::try_from(1 + state.colonies.len()).unwrap_or(u32::MAX),
        fleet_strength: state.fleet.total(),
        galaxy_seed: Some(state.galaxy_seed),
        generator_version: state.generator_version,
        species_id: state.species.as_ref().map(|run| run.id.clone()),
    })
}

pub(crate) fn inspect_backend_career(path: &Path) -> Result<crate::CareerGlance, SaveError> {
    if crate::save_is_locked(path) {
        return Err(SaveError::Locked);
    }
    let state = load_backend_state(path)?;
    let prestige = state.prestige.clone();
    let mut playtime_secs = state
        .last_tick_unix
        .saturating_sub(prestige.current.started_at);
    let mut galaxies_wonder = 0u32;
    let mut galaxies_score = 0u32;
    let mut past_wonders = 0u32;
    for run in &prestige.runs {
        let ended = run.ended_at.unwrap_or(state.last_tick_unix);
        playtime_secs = playtime_secs.saturating_add(ended.saturating_sub(run.started_at));
        past_wonders = past_wonders.saturating_add(run.wonders);
        if run.wonders > 0 {
            galaxies_wonder += 1;
        } else {
            galaxies_score += 1;
        }
    }
    let wonder_ids: std::collections::BTreeSet<String> =
        state.completed_wonders.iter().cloned().collect();
    let current_wonders = wonder_ids.len() as u32;
    let building_levels = crate::BuildingKind::ALL
        .iter()
        .map(|&kind| state.buildings.get(kind))
        .fold(0u32, u32::saturating_add);
    let research_levels = state.tech_levels.values().copied().sum();
    Ok(crate::CareerGlance {
        unlocks: state.achievements.clone(),
        playtime_secs,
        battles: u32::try_from(state.combat_logs.len()).unwrap_or(u32::MAX),
        wonders: current_wonders.saturating_add(past_wonders),
        galaxies_wonder,
        galaxies_score,
        prestige_runs: prestige.runs.len() as u32,
        prestige_currency: prestige.currency,
        prestige_lifetime: prestige.lifetime,
        prestige_owned: prestige.owned.clone(),
        prestige,
        building_levels,
        research_levels,
        colonies: u32::try_from(state.colonies.len()).unwrap_or(u32::MAX),
        fleet_ships: state.fleet.total(),
        raids_survived: state.raids_survived,
        outposts_conquered: state.outposts_conquered,
        wonder_ids,
        seasonal: state.season.is_some(),
        shares_prestige: state.season.as_ref().is_some_and(|run| run.prestige),
    })
}

fn load_backend_state(path: &Path) -> Result<GameState, SaveError> {
    let store = open_storage(path)?;
    if store.is_empty()? {
        return Err(SaveError::Corrupt("missing save"));
    }
    store.load(BalanceConfig::default())
}

#[cfg(feature = "mikura")]
pub(crate) fn encode_state_sqlite(state: &GameState) -> Result<Vec<u8>, SaveError> {
    let path = blob_path("enc");
    let bytes = (|| {
        let mut conn = open_save(&path)?;
        save_state(&mut conn, state)?;
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        drop(conn);
        Ok(std::fs::read(&path)?)
    })();
    cleanup_sqlite(&path);
    bytes
}

#[cfg(feature = "mikura")]
pub(crate) fn decode_state_sqlite(
    bytes: &[u8],
    config: BalanceConfig,
) -> Result<GameState, SaveError> {
    let path = blob_path("dec");
    let state = (|| {
        std::fs::write(&path, bytes)?;
        let conn = open_save(&path)?;
        load_state(&conn, config)
    })();
    cleanup_sqlite(&path);
    state
}

#[cfg(feature = "mikura")]
fn blob_path(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "hoshi-storage-blob-{tag}-{}-{nanos}.sqlite",
        std::process::id()
    ))
}

#[cfg(any(test, feature = "mikura", feature = "chisei"))]
pub(crate) fn cleanup_sqlite(path: &Path) {
    let _ = std::fs::remove_file(path);
    let wal = path.with_extension("sqlite-wal");
    let shm = path.with_extension("sqlite-shm");
    let _ = std::fs::remove_file(wal);
    let _ = std::fs::remove_file(shm);
    // WAL files sit beside the db as `{name}-wal` when the extension is sqlite.
    let mut wal2 = path.as_os_str().to_os_string();
    wal2.push("-wal");
    let mut shm2 = path.as_os_str().to_os_string();
    shm2.push("-shm");
    let _ = std::fs::remove_file(wal2);
    let _ = std::fs::remove_file(shm2);
}

#[cfg(test)]
pub(crate) fn assert_storage_contract(store: &mut dyn Storage) {
    use crate::clock::{Clock, FakeClock};
    use crate::fleets::Coord;
    use crate::ships::ShipKind;

    let mut cfg = BalanceConfig::default();
    cfg.raid.enabled = false;
    let now = 1_000u64;
    let mut original = GameState::with_config(now, cfg.clone());
    original.resources.metal = 12_345.0;
    original.fleet.add(ShipKind::SmallCargo, 3);
    store.save(&original).unwrap();
    let loaded = store.load(cfg.clone()).unwrap();
    assert_eq!(loaded, original, "round-trip");

    original.colonies.push(crate::Colony {
        coord: Coord {
            galaxy: 1,
            system: 1,
            position: 8,
        },
        kind: crate::PlanetType::Rocky,
        size: 8,
        rare: None,
        resources: crate::Resources::ZERO,
        buildings: crate::Buildings::default(),
        build_queue: Vec::new(),
        fleet: crate::Fleet::default(),
        defences: crate::DefenceGrid::default(),
        goods: std::collections::BTreeMap::new(),
    });
    original.outposts.push(crate::Outpost {
        coord: Coord {
            galaxy: 1,
            system: 2,
            position: 4,
        },
        kind: crate::PlanetType::Rocky,
        size: 10,
        defences: crate::Fleet::default(),
        garrison: 40,
        rare: None,
        last_rebuild_at: now,
        homeworld: false,
    });
    store.apply_batch(&original).unwrap();
    let batched = store.load(cfg.clone()).unwrap();
    assert_eq!(batched.colonies.len(), 1, "atomic colony");
    assert_eq!(
        batched.outposts.len(),
        original.outposts.len(),
        "atomic outposts"
    );
    assert_eq!(batched.fleet.get(ShipKind::SmallCargo), 3, "atomic fleet");
    assert_eq!(batched.resources.metal, 12_345.0);

    let clock = FakeClock::new(now);
    let mut live = batched;
    clock.advance(3_600);
    live.catch_up(clock.now_unix());
    let metal_after = live.resources.metal;
    let tick_after = live.last_tick_unix;
    store.save(&live).unwrap();
    let caught = store.load(cfg.clone()).unwrap();
    assert_eq!(caught.last_tick_unix, tick_after);
    assert!((caught.resources.metal - metal_after).abs() < 1e-9);

    let mut colony = GameState::with_config(now, cfg.clone());
    colony.tech_levels.insert("colonization".into(), 1);
    colony.resources.deuterium = 10_000.0;
    colony.fleet.add(ShipKind::ColonyShip, 1);
    let dest = (2u16..=15)
        .map(|position| Coord {
            galaxy: 1,
            system: 1,
            position,
        })
        .find(|coord| *coord != Coord::HOME && colony.outpost(*coord).is_none())
        .expect("empty slot");
    colony.launch_colony(dest, 100, now).unwrap();
    let outbound = colony.missions[0].outbound_secs;
    colony.catch_up(now + outbound * 2);
    assert_eq!(colony.colonies.len(), 1);
    store.save(&colony).unwrap();
    let settled = store.load(cfg).unwrap();
    assert_eq!(settled.colonies.len(), 1);
    assert_eq!(settled.colonies[0].coord, dest);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_sqlite(tag: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "hoshi-storage-{tag}-{}-{nanos}.sqlite",
            std::process::id()
        ))
    }

    #[test]
    fn sqlite_backend_meets_storage_contract() {
        let path = unique_sqlite("sqlite");
        let mut store = SqliteStorage::open(&path).unwrap();
        assert_storage_contract(&mut store);
        drop(store);
        cleanup_sqlite(&path);
        let _ = std::fs::remove_file(crate::save_lock_path(&path));
    }

    #[test]
    fn default_path_is_sqlite() {
        assert_eq!(
            BackendKind::from_path(Path::new("save.sqlite")),
            BackendKind::Sqlite
        );
        assert_eq!(
            BackendKind::from_path(Path::new("empire.mikura")),
            BackendKind::Mikura
        );
        assert_eq!(
            BackendKind::from_path(Path::new("empire.chisei")),
            BackendKind::Chisei
        );
    }

    #[test]
    fn mikura_path_without_feature_is_refused() {
        if cfg!(feature = "mikura") {
            return;
        }
        match open_storage(Path::new("x.mikura")) {
            Err(SaveError::Backend(msg)) => assert!(msg.contains("mikura"), "{msg}"),
            Err(other) => panic!("{other}"),
            Ok(_) => panic!("expected backend error"),
        }
    }

    #[test]
    fn chisei_path_without_feature_is_refused() {
        if cfg!(feature = "chisei") {
            return;
        }
        match open_storage(Path::new("x.chisei")) {
            Err(SaveError::Backend(msg)) => assert!(msg.contains("chisei"), "{msg}"),
            Err(other) => panic!("{other}"),
            Ok(_) => panic!("expected backend error"),
        }
    }

    #[cfg(feature = "mikura")]
    #[test]
    fn mikura_backend_meets_storage_contract() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "hoshi-storage-mikura-{}-{nanos}.mikura",
            std::process::id()
        ));
        let mut store = MikuraStorage::open(&path).unwrap();
        assert_storage_contract(&mut store);
        drop(store);
        let _ = std::fs::remove_file(&path);
        if let Some(name) = path.file_name().and_then(|n| n.to_str())
            && let Some(parent) = path.parent()
            && let Ok(entries) = std::fs::read_dir(parent)
        {
            let prefix = format!("{name}.state.");
            for entry in entries.flatten() {
                if entry
                    .file_name()
                    .to_str()
                    .is_some_and(|n| n.starts_with(&prefix))
                {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        let mut joins = path.as_os_str().to_os_string();
        joins.push(".joins");
        let mut delta = path.as_os_str().to_os_string();
        delta.push(".joins.delta");
        let _ = std::fs::remove_file(joins);
        let _ = std::fs::remove_file(delta);
        let _ = std::fs::remove_file(crate::save_lock_path(&path));
    }
}
