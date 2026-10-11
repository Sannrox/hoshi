//! SQLite save file. Schema version is `PRAGMA user_version`.

use crate::balance::BalanceConfig;
use crate::buildings::{BuildJob, BuildingKind, Buildings};
use crate::fleets::{Coord, Mission, MissionPhase};
use crate::planets::{Colony, PlanetType};
use crate::research::ResearchJob;
use crate::ships::{Fleet, ShipJob, ShipKind};
use crate::state::{GameEvent, GameState, Resources};
use rusqlite::{Connection, OptionalExtension, params};
use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};

/// Current schema. Bump when adding a migration.
pub const SCHEMA_VERSION: u32 = 5;

#[derive(Debug)]
pub enum SaveError {
    Sqlite(rusqlite::Error),
    SchemaTooNew { found: u32, current: u32 },
    Corrupt(&'static str),
    Io(std::io::Error),
}

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SaveError::Sqlite(e) => write!(f, "save database: {e}"),
            SaveError::SchemaTooNew { found, current } => {
                write!(
                    f,
                    "save schema v{found} is newer than this build (v{current})"
                )
            }
            SaveError::Corrupt(msg) => write!(f, "corrupt save: {msg}"),
            SaveError::Io(e) => write!(f, "save path: {e}"),
        }
    }
}

impl std::error::Error for SaveError {}

impl From<rusqlite::Error> for SaveError {
    fn from(value: rusqlite::Error) -> Self {
        SaveError::Sqlite(value)
    }
}

impl From<std::io::Error> for SaveError {
    fn from(value: std::io::Error) -> Self {
        SaveError::Io(value)
    }
}

/// Flag, then `HOSHI_SAVE`, then the per-user data directory. Never the cwd.
pub fn resolve_save_path(flag: Option<&Path>, env_value: Option<&OsStr>) -> PathBuf {
    if let Some(path) = flag {
        return path.to_path_buf();
    }
    if let Some(path) = env_value {
        return PathBuf::from(path);
    }
    default_save_path()
}

pub fn default_save_path() -> PathBuf {
    let mut dir = user_data_dir();
    dir.push("hoshi");
    dir.push("save.sqlite");
    dir
}

fn user_data_dir() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(xdg);
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        if cfg!(target_os = "macos") {
            return home.join("Library/Application Support");
        }
        return home.join(".local/share");
    }
    std::env::temp_dir()
}

/// Open (or create) the save at `path`, enable WAL, and run migrations.
pub fn open_save(path: &Path) -> Result<Connection, SaveError> {
    if path != Path::new(":memory:")
        && let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(path)?;
    // `:memory:` does not support WAL; file saves need it so a crash mid-write
    // keeps the last committed snapshot.
    if path != Path::new(":memory:") {
        conn.pragma_update(None, "journal_mode", "WAL")?;
    }
    conn.pragma_update(None, "foreign_keys", "ON")?;
    migrate(&conn)?;
    Ok(conn)
}

pub fn save_state(conn: &mut Connection, state: &GameState) -> Result<(), SaveError> {
    let tx = conn.transaction()?;
    tx.execute_batch(
        "DELETE FROM meta;
         DELETE FROM resources;
         DELETE FROM buildings;
         DELETE FROM build_queue;
         DELETE FROM tech_levels;
         DELETE FROM research;
         DELETE FROM ships;
         DELETE FROM ship_queue;
         DELETE FROM mission_ships;
         DELETE FROM missions;
         DELETE FROM raids;
         DELETE FROM event_log;
         DELETE FROM colonies;",
    )?;
    // RNG seed is stored as the same 64 bits in a signed INTEGER column.
    tx.execute(
        "INSERT INTO meta (id, last_tick_unix, rng_seed, galaxy_seed, home_kind, home_size)
         VALUES (1, ?1, ?2, ?3, ?4, ?5)",
        params![
            state.last_tick_unix as i64,
            state.rng_seed as i64,
            state.galaxy_seed as i64,
            state.home_kind.as_key(),
            state.home_size as i64,
        ],
    )?;
    tx.execute(
        "INSERT INTO resources (planet_id, metal, crystal, deuterium) VALUES (1, ?1, ?2, ?3)",
        params![
            state.resources.metal,
            state.resources.crystal,
            state.resources.deuterium
        ],
    )?;
    for kind in BuildingKind::ALL {
        tx.execute(
            "INSERT INTO buildings (planet_id, kind, level) VALUES (1, ?1, ?2)",
            params![kind.as_key(), state.buildings.get(kind)],
        )?;
    }
    for (i, job) in state.build_queue.iter().enumerate() {
        tx.execute(
            "INSERT INTO build_queue (
                planet_id, position, kind, from_level,
                paid_metal, paid_crystal, paid_deuterium,
                started_at, completes_at
             ) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                i as i64,
                job.building.as_key(),
                job.from_level,
                job.paid.metal,
                job.paid.crystal,
                job.paid.deuterium,
                job.started_at.map(|t| t as i64),
                job.completes_at.map(|t| t as i64),
            ],
        )?;
    }
    for (id, level) in &state.tech_levels {
        tx.execute(
            "INSERT INTO tech_levels (id, level) VALUES (?1, ?2)",
            params![id, *level],
        )?;
    }
    if let Some(job) = &state.research {
        tx.execute(
            "INSERT INTO research (
                tech, from_level, paid_metal, paid_crystal, paid_deuterium,
                started_at, completes_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                job.tech,
                job.from_level,
                job.paid.metal,
                job.paid.crystal,
                job.paid.deuterium,
                job.started_at as i64,
                job.completes_at as i64,
            ],
        )?;
    }
    for kind in ShipKind::ALL {
        tx.execute(
            "INSERT INTO ships (planet_id, kind, count) VALUES (1, ?1, ?2)",
            params![kind.as_key(), state.fleet.get(kind)],
        )?;
    }
    for (i, job) in state.ship_queue.iter().enumerate() {
        tx.execute(
            "INSERT INTO ship_queue (
                planet_id, position, kind, remaining,
                paid_metal, paid_crystal, paid_deuterium,
                started_at, completes_at
             ) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                i as i64,
                job.ship.as_key(),
                job.remaining,
                job.unit_paid.metal,
                job.unit_paid.crystal,
                job.unit_paid.deuterium,
                job.started_at.map(|t| t as i64),
                job.completes_at.map(|t| t as i64),
            ],
        )?;
    }
    for mission in state.missions.iter() {
        tx.execute(
            "INSERT INTO missions (
                id, origin_g, origin_s, origin_p, dest_g, dest_s, dest_p,
                speed_percent, phase, launched_at, outbound_secs, hold_secs,
                phase_ends_at, fuel_paid, colonize
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                mission.id as i64,
                mission.origin.galaxy as i64,
                mission.origin.system as i64,
                mission.origin.position as i64,
                mission.dest.galaxy as i64,
                mission.dest.system as i64,
                mission.dest.position as i64,
                mission.speed_percent as i64,
                mission.phase.as_key(),
                mission.launched_at as i64,
                mission.outbound_secs as i64,
                mission.hold_secs as i64,
                mission.phase_ends_at as i64,
                mission.fuel_paid,
                mission.colonize as i64,
            ],
        )?;
        for kind in ShipKind::ALL {
            let count = mission.ships.get(kind);
            if count == 0 {
                continue;
            }
            tx.execute(
                "INSERT INTO mission_ships (mission_id, kind, count) VALUES (?1, ?2, ?3)",
                params![mission.id as i64, kind.as_key(), count],
            )?;
        }
    }
    tx.execute(
        "INSERT INTO raids (id, next_raid_at) VALUES (1, ?1)",
        params![state.next_raid_at.map(|t| t as i64)],
    )?;
    for (i, event) in state.events.iter().enumerate() {
        tx.execute(
            "INSERT INTO event_log (position, at, text) VALUES (?1, ?2, ?3)",
            params![i as i64, event.at as i64, event.text],
        )?;
    }
    for (i, colony) in state.colonies.iter().enumerate() {
        let planet_id = (i as i64) + 2;
        tx.execute(
            "INSERT INTO colonies (planet_id, galaxy, system, position, kind, size)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                planet_id,
                colony.coord.galaxy as i64,
                colony.coord.system as i64,
                colony.coord.position as i64,
                colony.kind.as_key(),
                colony.size as i64,
            ],
        )?;
        tx.execute(
            "INSERT INTO resources (planet_id, metal, crystal, deuterium) VALUES (?1, ?2, ?3, ?4)",
            params![
                planet_id,
                colony.resources.metal,
                colony.resources.crystal,
                colony.resources.deuterium
            ],
        )?;
        for kind in BuildingKind::ALL {
            tx.execute(
                "INSERT INTO buildings (planet_id, kind, level) VALUES (?1, ?2, ?3)",
                params![planet_id, kind.as_key(), colony.buildings.get(kind)],
            )?;
        }
        for kind in ShipKind::ALL {
            tx.execute(
                "INSERT INTO ships (planet_id, kind, count) VALUES (?1, ?2, ?3)",
                params![planet_id, kind.as_key(), colony.fleet.get(kind)],
            )?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// Reconstruct player state. `config` is the caller's rules; it is not stored in the save.
pub fn load_state(conn: &Connection, config: BalanceConfig) -> Result<GameState, SaveError> {
    let mut state = GameState::with_config(0, config);
    let (last_tick, rng_seed, galaxy_seed, home_kind, home_size): (
        i64,
        i64,
        i64,
        String,
        i64,
    ) = conn
        .query_row(
            "SELECT last_tick_unix, rng_seed, galaxy_seed, home_kind, home_size FROM meta WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .optional()?
        .ok_or(SaveError::Corrupt("missing meta row"))?;
    state.last_tick_unix = unix_ts(last_tick)?;
    state.rng_seed = rng_seed as u64;
    state.galaxy_seed = galaxy_seed as u64;
    state.home_kind =
        PlanetType::from_key(&home_kind).ok_or(SaveError::Corrupt("unknown home planet type"))?;
    state.home_size = u8::try_from(home_size).map_err(|_| SaveError::Corrupt("home size"))?;

    let resources = conn
        .query_row(
            "SELECT metal, crystal, deuterium FROM resources WHERE planet_id = 1",
            [],
            |row| {
                Ok(Resources {
                    metal: row.get(0)?,
                    crystal: row.get(1)?,
                    deuterium: row.get(2)?,
                })
            },
        )
        .optional()?
        .ok_or(SaveError::Corrupt("missing resources row"))?;
    state.resources = resources;

    let mut buildings = Buildings {
        metal_mine: 0,
        crystal_mine: 0,
        deuterium_synthesizer: 0,
        solar_plant: 0,
        metal_storage: 0,
        crystal_storage: 0,
        deuterium_storage: 0,
        robotics_factory: 0,
        research_lab: 0,
        shipyard: 0,
    };
    let mut stmt = conn.prepare("SELECT kind, level FROM buildings WHERE planet_id = 1")?;
    let rows = stmt.query_map([], |row| {
        let kind: String = row.get(0)?;
        let level: u32 = row.get(1)?;
        Ok((kind, level))
    })?;
    for row in rows {
        let (kind, level) = row?;
        let kind =
            BuildingKind::from_key(&kind).ok_or(SaveError::Corrupt("unknown building kind"))?;
        buildings.set(kind, level);
    }
    state.buildings = buildings;

    let mut stmt = conn.prepare(
        "SELECT kind, from_level, paid_metal, paid_crystal, paid_deuterium, started_at, completes_at
         FROM build_queue WHERE planet_id = 1 ORDER BY position",
    )?;
    let mut queue = Vec::new();
    let jobs = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, u32>(1)?,
            row.get::<_, f64>(2)?,
            row.get::<_, f64>(3)?,
            row.get::<_, f64>(4)?,
            row.get::<_, Option<i64>>(5)?,
            row.get::<_, Option<i64>>(6)?,
        ))
    })?;
    for job in jobs {
        let (kind, from_level, metal, crystal, deuterium, started_at, completes_at) = job?;
        let building =
            BuildingKind::from_key(&kind).ok_or(SaveError::Corrupt("unknown building kind"))?;
        queue.push(BuildJob {
            building,
            from_level,
            paid: Resources {
                metal,
                crystal,
                deuterium,
            },
            started_at: unix_ts_opt(started_at)?,
            completes_at: unix_ts_opt(completes_at)?,
        });
    }
    state.build_queue = queue;

    let mut stmt = conn.prepare("SELECT id, level FROM tech_levels")?;
    let techs = stmt.query_map([], |row| {
        let id: String = row.get(0)?;
        let level: u32 = row.get(1)?;
        Ok((id, level))
    })?;
    state.tech_levels.clear();
    for row in techs {
        let (id, level) = row?;
        state.tech_levels.insert(id, level);
    }

    state.research = match conn
        .query_row(
            "SELECT tech, from_level, paid_metal, paid_crystal, paid_deuterium, started_at, completes_at
             FROM research LIMIT 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u32>(1)?,
                    row.get::<_, f64>(2)?,
                    row.get::<_, f64>(3)?,
                    row.get::<_, f64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                ))
            },
        )
        .optional()?
    {
        None => None,
        Some((tech, from_level, metal, crystal, deuterium, started_at, completes_at)) => {
            Some(ResearchJob {
                tech,
                from_level,
                paid: Resources {
                    metal,
                    crystal,
                    deuterium,
                },
                started_at: unix_ts(started_at)?,
                completes_at: unix_ts(completes_at)?,
            })
        }
    };

    let mut fleet = Fleet::default();
    let mut stmt = conn.prepare("SELECT kind, count FROM ships WHERE planet_id = 1")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
    })?;
    for row in rows {
        let (kind, count) = row?;
        let kind = ShipKind::from_key(&kind).ok_or(SaveError::Corrupt("unknown ship kind"))?;
        fleet.add(kind, count);
    }
    state.fleet = fleet;

    let mut stmt = conn.prepare(
        "SELECT kind, remaining, paid_metal, paid_crystal, paid_deuterium, started_at, completes_at
         FROM ship_queue WHERE planet_id = 1 ORDER BY position",
    )?;
    let mut queue = Vec::new();
    let jobs = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, u32>(1)?,
            row.get::<_, f64>(2)?,
            row.get::<_, f64>(3)?,
            row.get::<_, f64>(4)?,
            row.get::<_, Option<i64>>(5)?,
            row.get::<_, Option<i64>>(6)?,
        ))
    })?;
    for job in jobs {
        let (kind, remaining, metal, crystal, deuterium, started_at, completes_at) = job?;
        let ship = ShipKind::from_key(&kind).ok_or(SaveError::Corrupt("unknown ship kind"))?;
        queue.push(ShipJob {
            ship,
            remaining,
            unit_paid: Resources {
                metal,
                crystal,
                deuterium,
            },
            started_at: unix_ts_opt(started_at)?,
            completes_at: unix_ts_opt(completes_at)?,
        });
    }
    state.ship_queue = queue;

    let mut stmt = conn.prepare(
        "SELECT id, origin_g, origin_s, origin_p, dest_g, dest_s, dest_p,
                speed_percent, phase, launched_at, outbound_secs, hold_secs,
                phase_ends_at, fuel_paid, colonize
         FROM missions ORDER BY id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, i64>(5)?,
            row.get::<_, i64>(6)?,
            row.get::<_, i64>(7)?,
            row.get::<_, String>(8)?,
            row.get::<_, i64>(9)?,
            row.get::<_, i64>(10)?,
            row.get::<_, i64>(11)?,
            row.get::<_, i64>(12)?,
            row.get::<_, f64>(13)?,
            row.get::<_, i64>(14)?,
        ))
    })?;
    let mut missions = Vec::new();
    for row in rows {
        let (
            id,
            og,
            os,
            op,
            dg,
            ds,
            dp,
            speed,
            phase,
            launched_at,
            outbound_secs,
            hold_secs,
            phase_ends_at,
            fuel_paid,
            colonize,
        ) = row?;
        let phase =
            MissionPhase::from_key(&phase).ok_or(SaveError::Corrupt("unknown mission phase"))?;
        let mut ships = Fleet::default();
        let mut ship_stmt =
            conn.prepare("SELECT kind, count FROM mission_ships WHERE mission_id = ?1")?;
        let ship_rows = ship_stmt.query_map(params![id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
        })?;
        for ship_row in ship_rows {
            let (kind, count) = ship_row?;
            let kind = ShipKind::from_key(&kind).ok_or(SaveError::Corrupt("unknown ship kind"))?;
            ships.add(kind, count);
        }
        missions.push(Mission {
            id: unix_ts(id)?,
            ships,
            origin: Coord {
                galaxy: u16_coord(og)?,
                system: u16_coord(os)?,
                position: u16_coord(op)?,
            },
            dest: Coord {
                galaxy: u16_coord(dg)?,
                system: u16_coord(ds)?,
                position: u16_coord(dp)?,
            },
            speed_percent: u8::try_from(speed).map_err(|_| SaveError::Corrupt("speed percent"))?,
            phase,
            launched_at: unix_ts(launched_at)?,
            outbound_secs: unix_ts(outbound_secs)?,
            hold_secs: unix_ts(hold_secs)?,
            phase_ends_at: unix_ts(phase_ends_at)?,
            fuel_paid,
            colonize: colonize != 0,
        });
    }
    state.missions = missions;
    state.next_mission_id = state
        .missions
        .iter()
        .map(|m| m.id)
        .max()
        .map(|id| id.saturating_add(1))
        .unwrap_or(0);

    state.next_raid_at = conn
        .query_row("SELECT next_raid_at FROM raids WHERE id = 1", [], |row| {
            row.get::<_, Option<i64>>(0)
        })
        .optional()?
        .flatten()
        .map(unix_ts)
        .transpose()?;
    if state.next_raid_at.is_none() && state.config.raid.enabled {
        state.next_raid_at = Some(
            state
                .last_tick_unix
                .saturating_add(state.config.raid.first_delay_secs),
        );
    }

    let mut stmt = conn.prepare("SELECT at, text FROM event_log ORDER BY position")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    state.events.clear();
    for row in rows {
        let (at, text) = row?;
        state.events.push(GameEvent {
            at: unix_ts(at)?,
            text,
        });
    }

    let mut stmt = conn.prepare(
        "SELECT planet_id, galaxy, system, position, kind, size FROM colonies ORDER BY planet_id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, i64>(5)?,
        ))
    })?;
    let mut pending = Vec::new();
    for row in rows {
        pending.push(row?);
    }
    drop(stmt);
    state.colonies.clear();
    for (planet_id, g, s, p, kind, size) in pending {
        let kind =
            PlanetType::from_key(&kind).ok_or(SaveError::Corrupt("unknown colony planet type"))?;
        let resources = conn.query_row(
            "SELECT metal, crystal, deuterium FROM resources WHERE planet_id = ?1",
            params![planet_id],
            |row| {
                Ok(Resources {
                    metal: row.get(0)?,
                    crystal: row.get(1)?,
                    deuterium: row.get(2)?,
                })
            },
        )?;
        let mut buildings = crate::Buildings::default();
        let mut bstmt = conn.prepare("SELECT kind, level FROM buildings WHERE planet_id = ?1")?;
        let brows = bstmt.query_map(params![planet_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
        })?;
        for brow in brows {
            let (key, level) = brow?;
            let bkind =
                BuildingKind::from_key(&key).ok_or(SaveError::Corrupt("unknown building"))?;
            buildings.set(bkind, level);
        }
        let mut fleet = Fleet::default();
        let mut sstmt = conn.prepare("SELECT kind, count FROM ships WHERE planet_id = ?1")?;
        let srows = sstmt.query_map(params![planet_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
        })?;
        for srow in srows {
            let (key, count) = srow?;
            let skind = ShipKind::from_key(&key).ok_or(SaveError::Corrupt("unknown ship kind"))?;
            fleet.add(skind, count);
        }
        state.colonies.push(Colony {
            coord: Coord {
                galaxy: u16_coord(g)?,
                system: u16_coord(s)?,
                position: u16_coord(p)?,
            },
            kind,
            size: u8::try_from(size).map_err(|_| SaveError::Corrupt("colony size"))?,
            resources,
            buildings,
            fleet,
        });
    }

    Ok(state)
}

fn u16_coord(value: i64) -> Result<u16, SaveError> {
    u16::try_from(value).map_err(|_| SaveError::Corrupt("coordinate out of range"))
}

/// Load an existing save, or write a new game when the file has no meta row yet.
pub fn load_or_new(
    conn: &mut Connection,
    now_unix: u64,
    config: BalanceConfig,
) -> Result<GameState, SaveError> {
    let meta: i64 = count_rows(conn, "meta")?;
    if meta == 0 {
        let leftover = count_rows(conn, "resources")?
            + count_rows(conn, "buildings")?
            + count_rows(conn, "build_queue")?
            + count_rows(conn, "tech_levels")?
            + count_rows(conn, "research")?
            + count_rows(conn, "ships")?
            + count_rows(conn, "ship_queue")?
            + count_rows(conn, "missions")?
            + count_rows(conn, "mission_ships")?
            + count_rows(conn, "raids")?
            + count_rows(conn, "event_log")?
            + count_rows(conn, "colonies")?;
        if leftover != 0 {
            return Err(SaveError::Corrupt("missing meta row"));
        }
        let state = GameState::with_config(now_unix, config);
        save_state(conn, &state)?;
        return Ok(state);
    }
    load_state(conn, config)
}

fn unix_ts(value: i64) -> Result<u64, SaveError> {
    u64::try_from(value).map_err(|_| SaveError::Corrupt("negative timestamp"))
}

fn unix_ts_opt(value: Option<i64>) -> Result<Option<u64>, SaveError> {
    value.map(unix_ts).transpose()
}

fn count_rows(conn: &Connection, table: &str) -> Result<i64, SaveError> {
    let sql = match table {
        "meta" => "SELECT COUNT(*) FROM meta",
        "resources" => "SELECT COUNT(*) FROM resources",
        "buildings" => "SELECT COUNT(*) FROM buildings",
        "build_queue" => "SELECT COUNT(*) FROM build_queue",
        "tech_levels" => "SELECT COUNT(*) FROM tech_levels",
        "research" => "SELECT COUNT(*) FROM research",
        "ships" => "SELECT COUNT(*) FROM ships",
        "ship_queue" => "SELECT COUNT(*) FROM ship_queue",
        "missions" => "SELECT COUNT(*) FROM missions",
        "mission_ships" => "SELECT COUNT(*) FROM mission_ships",
        "raids" => "SELECT COUNT(*) FROM raids",
        "event_log" => "SELECT COUNT(*) FROM event_log",
        "colonies" => "SELECT COUNT(*) FROM colonies",
        _ => return Err(SaveError::Corrupt("unknown save table")),
    };
    Ok(conn.query_row(sql, [], |row| row.get(0))?)
}

fn migrate(conn: &Connection) -> Result<(), SaveError> {
    let found: u32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if found > SCHEMA_VERSION {
        return Err(SaveError::SchemaTooNew {
            found,
            current: SCHEMA_VERSION,
        });
    }
    if found < 1 {
        migrate_v1(conn)?;
    }
    if found < 2 {
        migrate_v2(conn)?;
    }
    if found < 3 {
        migrate_v3(conn)?;
    }
    if found < 4 {
        migrate_v4(conn)?;
    }
    if found < 5 {
        migrate_v5(conn)?;
    }
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
}

fn migrate_v1(conn: &Connection) -> Result<(), SaveError> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS meta (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            last_tick_unix INTEGER NOT NULL,
            rng_seed INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS resources (
            planet_id INTEGER PRIMARY KEY,
            metal REAL NOT NULL,
            crystal REAL NOT NULL,
            deuterium REAL NOT NULL
        );
        CREATE TABLE IF NOT EXISTS buildings (
            planet_id INTEGER NOT NULL,
            kind TEXT NOT NULL,
            level INTEGER NOT NULL,
            PRIMARY KEY (planet_id, kind)
        );
        CREATE TABLE IF NOT EXISTS build_queue (
            planet_id INTEGER NOT NULL,
            position INTEGER NOT NULL,
            kind TEXT NOT NULL,
            from_level INTEGER NOT NULL,
            paid_metal REAL NOT NULL,
            paid_crystal REAL NOT NULL,
            paid_deuterium REAL NOT NULL,
            started_at INTEGER,
            completes_at INTEGER,
            PRIMARY KEY (planet_id, position)
        );
        CREATE TABLE IF NOT EXISTS tech_levels (
            id TEXT PRIMARY KEY,
            level INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS research (
            tech TEXT PRIMARY KEY,
            from_level INTEGER NOT NULL,
            paid_metal REAL NOT NULL,
            paid_crystal REAL NOT NULL,
            paid_deuterium REAL NOT NULL,
            started_at INTEGER NOT NULL,
            completes_at INTEGER NOT NULL
        );
        ",
    )?;
    Ok(())
}

fn migrate_v2(conn: &Connection) -> Result<(), SaveError> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS ships (
            planet_id INTEGER NOT NULL,
            kind TEXT NOT NULL,
            count INTEGER NOT NULL,
            PRIMARY KEY (planet_id, kind)
        );
        CREATE TABLE IF NOT EXISTS ship_queue (
            planet_id INTEGER NOT NULL,
            position INTEGER NOT NULL,
            kind TEXT NOT NULL,
            remaining INTEGER NOT NULL,
            paid_metal REAL NOT NULL,
            paid_crystal REAL NOT NULL,
            paid_deuterium REAL NOT NULL,
            started_at INTEGER,
            completes_at INTEGER,
            PRIMARY KEY (planet_id, position)
        );
        ",
    )?;
    Ok(())
}

fn migrate_v3(conn: &Connection) -> Result<(), SaveError> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS missions (
            id INTEGER PRIMARY KEY,
            origin_g INTEGER NOT NULL,
            origin_s INTEGER NOT NULL,
            origin_p INTEGER NOT NULL,
            dest_g INTEGER NOT NULL,
            dest_s INTEGER NOT NULL,
            dest_p INTEGER NOT NULL,
            speed_percent INTEGER NOT NULL,
            phase TEXT NOT NULL,
            launched_at INTEGER NOT NULL,
            outbound_secs INTEGER NOT NULL,
            hold_secs INTEGER NOT NULL,
            phase_ends_at INTEGER NOT NULL,
            fuel_paid REAL NOT NULL
        );
        CREATE TABLE IF NOT EXISTS mission_ships (
            mission_id INTEGER NOT NULL,
            kind TEXT NOT NULL,
            count INTEGER NOT NULL,
            PRIMARY KEY (mission_id, kind)
        );
        ",
    )?;
    Ok(())
}

fn migrate_v4(conn: &Connection) -> Result<(), SaveError> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS raids (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            next_raid_at INTEGER
        );
        CREATE TABLE IF NOT EXISTS event_log (
            position INTEGER PRIMARY KEY,
            at INTEGER NOT NULL,
            text TEXT NOT NULL
        );
        ",
    )?;
    Ok(())
}

fn migrate_v5(conn: &Connection) -> Result<(), SaveError> {
    conn.execute_batch(
        "
        ALTER TABLE meta ADD COLUMN galaxy_seed INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE meta ADD COLUMN home_kind TEXT NOT NULL DEFAULT 'rocky';
        ALTER TABLE meta ADD COLUMN home_size INTEGER NOT NULL DEFAULT 163;
        ALTER TABLE missions ADD COLUMN colonize INTEGER NOT NULL DEFAULT 0;
        CREATE TABLE IF NOT EXISTS colonies (
            planet_id INTEGER PRIMARY KEY,
            galaxy INTEGER NOT NULL,
            system INTEGER NOT NULL,
            position INTEGER NOT NULL,
            kind TEXT NOT NULL,
            size INTEGER NOT NULL
        );
        ",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BuildingKind;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_path(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "hoshi-save-test-{}-{}-{tag}.sqlite",
            std::process::id(),
            nanos
        ))
    }

    fn sample_state() -> GameState {
        let mut state = GameState::new(1_700_000_000);
        state.resources.metal = 10_000.0;
        state.resources.crystal = 10_000.0;
        state.resources.deuterium = 10_000.0;
        state.buildings.metal_mine = 3;
        state.buildings.research_lab = 1;
        state.tech_levels.insert("energy".into(), 2);
        state.rng_seed = 42;
        state
            .enqueue_build(BuildingKind::SolarPlant, 1_700_000_000)
            .unwrap();
        state.start_research("computer", 1_700_000_000).unwrap();
        state.buildings.shipyard = 1;
        state.tech_levels.insert("combustion_drive".into(), 1);
        state
            .enqueue_ships(crate::ShipKind::LightFighter, 2, 1_700_000_000)
            .unwrap();
        state.fleet.add(crate::ShipKind::HeavyFighter, 3);
        state.fleet.add(crate::ShipKind::LightFighter, 2);
        state.galaxy_seed = 11;
        let dest = crate::Coord {
            galaxy: 1,
            system: 1,
            position: 14,
        };
        let (kind, size) = crate::planet_profile(11, dest);
        let mut colony_fleet = crate::Fleet::default();
        colony_fleet.add(crate::ShipKind::SmallCargo, 1);
        state.colonies.push(crate::Colony {
            coord: dest,
            kind,
            size,
            resources: Resources {
                metal: 50.0,
                crystal: 20.0,
                deuterium: 5.0,
            },
            buildings: crate::Buildings {
                metal_mine: 1,
                ..crate::Buildings::default()
            },
            fleet: colony_fleet,
        });
        state.events.push(crate::GameEvent {
            at: 1_700_000_000,
            text: "raid: pirates 1 light fighter vs none. 0 rounds. lost none. pirates lost none. plunder M0 C0 D0".into(),
        });
        let mut wing = crate::Fleet::default();
        wing.add(crate::ShipKind::LightFighter, 2);
        state
            .launch_fleet(
                wing,
                crate::Coord {
                    galaxy: 2,
                    system: 1,
                    position: 1,
                },
                100,
                0,
                1_700_000_000,
            )
            .unwrap();
        if let Some(mission) = state.missions.last_mut() {
            mission.colonize = true;
        }
        state
    }

    #[test]
    fn round_trip_save_then_load_equals_state() {
        let mut conn = open_save(Path::new(":memory:")).unwrap();
        let original = sample_state();
        save_state(&mut conn, &original).unwrap();
        let loaded = load_state(&conn, BalanceConfig::default()).unwrap();
        assert_eq!(loaded, original);
    }

    #[test]
    fn migrates_schema_v1_fixture_to_current() {
        let path = unique_path("v1");
        let _ = std::fs::remove_file(&path);
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("PRAGMA user_version = 1;").unwrap();
            migrate_v1(&conn).unwrap();
            conn.execute(
                "INSERT INTO meta (id, last_tick_unix, rng_seed) VALUES (1, 99, 7)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO resources (planet_id, metal, crystal, deuterium) VALUES (1, 10, 20, 30)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO buildings (planet_id, kind, level) VALUES (1, 'metal_mine', 4)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO tech_levels (id, level) VALUES ('weapons', 3)",
                [],
            )
            .unwrap();
        }
        let conn = open_save(&path).unwrap();
        let version: u32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        let loaded = load_state(&conn, BalanceConfig::default()).unwrap();
        assert_eq!(loaded.last_tick_unix, 99);
        assert_eq!(loaded.rng_seed, 7);
        assert_eq!(loaded.resources.metal, 10.0);
        assert_eq!(loaded.buildings.metal_mine, 4);
        assert_eq!(loaded.tech_level("weapons"), 3);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    }

    #[test]
    fn uncommitted_write_leaves_the_previous_save_loadable() {
        let path = unique_path("wal");
        let _ = std::fs::remove_file(&path);
        let original = sample_state();
        {
            let mut conn = open_save(&path).unwrap();
            let mode: String = conn
                .query_row("PRAGMA journal_mode", [], |row| row.get(0))
                .unwrap();
            assert_eq!(mode.to_lowercase(), "wal");
            save_state(&mut conn, &original).unwrap();
            conn.execute_batch("BEGIN; DELETE FROM meta; INSERT INTO meta (id, last_tick_unix, rng_seed) VALUES (1, 0, 0);")
                .unwrap();
            // Drop without commit: WAL + transaction must roll back.
        }
        let conn = open_save(&path).unwrap();
        let loaded = load_state(&conn, BalanceConfig::default()).unwrap();
        assert_eq!(loaded, original);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    }

    #[test]
    fn save_path_flag_and_env_override_default_and_default_is_not_cwd() {
        let flag = Path::new("/tmp/flag.sqlite");
        let env = OsStr::new("/tmp/env.sqlite");
        assert_eq!(
            resolve_save_path(Some(flag), Some(env)),
            PathBuf::from("/tmp/flag.sqlite")
        );
        assert_eq!(
            resolve_save_path(None, Some(env)),
            PathBuf::from("/tmp/env.sqlite")
        );
        let default = resolve_save_path(None, None);
        assert!(default.is_absolute());
        assert_eq!(default.file_name().unwrap(), "save.sqlite");
        assert!(
            default
                .components()
                .any(|c| c.as_os_str() == "hoshi" || c.as_os_str() == "Application Support")
        );
        let cwd = std::env::current_dir().unwrap();
        assert!(
            !default.starts_with(&cwd),
            "default save path must not be the working tree: {default:?}"
        );
    }

    #[test]
    fn load_or_new_writes_starting_state_when_meta_is_empty() {
        let mut conn = open_save(Path::new(":memory:")).unwrap();
        let state = load_or_new(&mut conn, 1_234, BalanceConfig::default()).unwrap();
        assert_eq!(state.last_tick_unix, 1_234);
        assert_eq!(state.resources, GameState::new(1_234).resources);
        let loaded = load_state(&conn, BalanceConfig::default()).unwrap();
        assert_eq!(loaded, state);
    }

    #[test]
    fn load_keeps_the_caller_balance_config() {
        let default = BalanceConfig::default();
        let config = BalanceConfig {
            max_build_queue: 3,
            metal_mine: crate::ProducerSpec {
                base_per_hour: 99.0,
                ..default.metal_mine
            },
            ..default
        };
        let mut state = GameState::with_config(50, config.clone());
        state.resources.metal = 123.0;
        let mut conn = open_save(Path::new(":memory:")).unwrap();
        save_state(&mut conn, &state).unwrap();
        let loaded = load_state(&conn, config.clone()).unwrap();
        assert_eq!(loaded.config.max_build_queue, 3);
        assert_eq!(loaded.config.metal_mine.base_per_hour, 99.0);
        assert_eq!(loaded, state);
    }

    #[test]
    fn rng_seed_round_trips_the_full_u64_range() {
        let mut original = GameState::new(1);
        original.rng_seed = u64::MAX;
        let mut conn = open_save(Path::new(":memory:")).unwrap();
        save_state(&mut conn, &original).unwrap();
        let loaded = load_state(&conn, BalanceConfig::default()).unwrap();
        assert_eq!(loaded.rng_seed, u64::MAX);
        assert_eq!(loaded, original);
    }

    #[test]
    fn open_save_creates_missing_parent_directories() {
        let path = unique_path("nested").join("deep").join("save.sqlite");
        let mut conn = open_save(&path).unwrap();
        save_state(&mut conn, &GameState::new(1)).unwrap();
        drop(conn);
        assert!(path.is_file());
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    }

    #[test]
    fn negative_last_tick_is_corrupt() {
        let conn = open_save(Path::new(":memory:")).unwrap();
        conn.execute(
            "INSERT INTO meta (id, last_tick_unix, rng_seed) VALUES (1, -1, 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO resources (planet_id, metal, crystal, deuterium) VALUES (1, 1, 1, 1)",
            [],
        )
        .unwrap();
        match load_state(&conn, BalanceConfig::default()) {
            Err(SaveError::Corrupt(msg)) => assert!(msg.contains("negative")),
            other => panic!("expected corrupt, got {other:?}"),
        }
    }

    #[test]
    fn load_or_new_does_not_wipe_a_save_that_lost_its_meta_row() {
        let mut conn = open_save(Path::new(":memory:")).unwrap();
        let original = sample_state();
        save_state(&mut conn, &original).unwrap();
        conn.execute("DELETE FROM meta", []).unwrap();
        match load_or_new(&mut conn, 99, BalanceConfig::default()) {
            Err(SaveError::Corrupt("missing meta row")) => {}
            other => panic!("expected corrupt, got {other:?}"),
        }
        let metal: f64 = conn
            .query_row(
                "SELECT metal FROM resources WHERE planet_id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(metal, original.resources.metal);
    }
}
