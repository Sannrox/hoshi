//! SQLite save file. Schema version is `PRAGMA user_version`.

use crate::activity::{ActivityChannel, ActivityTracker};
use crate::balance::BalanceConfig;
use crate::buildings::{BuildJob, BuildingKind, Buildings};
use crate::fleets::{Coord, Mission, MissionPhase, TradeRoute};
use crate::outposts::{Outpost, generate_outposts};
use crate::planets::{Colony, PlanetType};
use crate::prestige::{Prestige, RunStats};
use crate::research::ResearchJob;
use crate::ships::{Fleet, ShipJob, ShipKind};
use crate::state::{GameEvent, GameState, Resources};
use crate::wonders::WonderJob;
use crate::{CombatFrame, CombatLog, CombatReport, CombatWinner, DefenceKind, GroundReport};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

/// Current schema. Bump when adding a migration.
pub const SCHEMA_VERSION: u32 = 24;

#[derive(Debug)]
pub enum SaveError {
    Sqlite(rusqlite::Error),
    SchemaTooNew { found: u32, current: u32 },
    GeneratorTooNew { found: u32, current: u32 },
    Corrupt(&'static str),
    Io(std::io::Error),
    Locked,
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
            SaveError::GeneratorTooNew { found, current } => {
                write!(
                    f,
                    "save generator v{found} is newer than this build (v{current})"
                )
            }
            SaveError::Corrupt(msg) => write!(f, "corrupt save: {msg}"),
            SaveError::Io(e) => write!(f, "save path: {e}"),
            SaveError::Locked => write!(f, "save is locked"),
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
    default_save_dir().join("save.sqlite")
}

/// Directory that holds save slots (`*.sqlite`).
pub fn default_save_dir() -> PathBuf {
    let mut dir = user_data_dir();
    dir.push("hoshi");
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
         DELETE FROM exclusive_picks;
         DELETE FROM research;
         DELETE FROM ships;
         DELETE FROM ship_queue;
         DELETE FROM mission_ships;
         DELETE FROM missions;
         DELETE FROM raids;
         DELETE FROM event_log;
         DELETE FROM colonies;
         DELETE FROM activity_boosts;
         DELETE FROM activity_limits;
         DELETE FROM route_ships;
         DELETE FROM routes;
         DELETE FROM wonder_job;
         DELETE FROM wonder_done;
         DELETE FROM outpost_ships;
         DELETE FROM outposts;
         DELETE FROM prestige_owned;
         DELETE FROM prestige_runs;
         DELETE FROM prestige;
         DELETE FROM goods;
         DELETE FROM story_pending;
         DELETE FROM market_prices;
         DELETE FROM market_history;
         DELETE FROM faction_standing;
         DELETE FROM faction_log;
         DELETE FROM spy_report_ships;
         DELETE FROM spy_reports;
         DELETE FROM planet_defences;
         DELETE FROM law_samples;
         DELETE FROM law_approvals;
         DELETE FROM law_actions;
         DELETE FROM laws;
         DELETE FROM achievements;
         DELETE FROM achievement_stats;
         DELETE FROM combat_log_ships;
         DELETE FROM combat_log_frames;
         DELETE FROM combat_logs;
         DELETE FROM daily_slots;
         DELETE FROM daily_meta;",
    )?;
    // RNG seed is stored as the same 64 bits in a signed INTEGER column.
    tx.execute(
        "INSERT INTO meta (id, last_tick_unix, rng_seed, galaxy_seed, home_kind, home_size, deterministic, outposts_seeded, generator_version, last_story_at, last_market_at, last_diplomacy_at)
         VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, ?8, ?9, ?10)",
        params![
            state.last_tick_unix as i64,
            state.rng_seed as i64,
            state.galaxy_seed as i64,
            state.home_kind.as_key(),
            state.home_size as i64,
            i64::from(state.deterministic),
            state.generator_version as i64,
            state.last_story_at as i64,
            state.last_market_at as i64,
            state.last_diplomacy_at as i64,
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
    insert_goods(&tx, 1, &state.goods)?;
    for kind in BuildingKind::ALL {
        tx.execute(
            "INSERT INTO buildings (planet_id, kind, level) VALUES (1, ?1, ?2)",
            params![kind.as_key(), state.buildings.get(kind)],
        )?;
    }
    insert_build_queue(&tx, 1, &state.build_queue)?;
    for (id, level) in &state.tech_levels {
        tx.execute(
            "INSERT INTO tech_levels (id, level) VALUES (?1, ?2)",
            params![id, *level],
        )?;
    }
    for (group, tech) in &state.exclusive_picks {
        tx.execute(
            "INSERT INTO exclusive_picks (group_id, tech_id) VALUES (?1, ?2)",
            params![group, tech],
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
    insert_defences(&tx, 1, state.defences)?;
    for (i, job) in state.ship_queue.iter().enumerate() {
        tx.execute(
            "INSERT INTO ship_queue (
                planet_id, position, kind, remaining,
                paid_metal, paid_crystal, paid_deuterium,
                started_at, completes_at
             ) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                i as i64,
                job.kind_key(),
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
                phase_ends_at, fuel_paid, colonize, expedition_seed, expedition_applied,
                cargo_metal, cargo_crystal, cargo_deuterium, transport, attack, invade, spy
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24)",
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
                mission.expedition_seed.map(|s| s as i64),
                mission.expedition_applied as i64,
                mission.cargo.metal,
                mission.cargo.crystal,
                mission.cargo.deuterium,
                mission.transport as i64,
                mission.attack as i64,
                mission.invade as i64,
                mission.spy as i64,
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
    for route in state.routes.iter() {
        tx.execute(
            "INSERT INTO routes (
                id, origin_g, origin_s, origin_p, dest_g, dest_s, dest_p,
                cargo_metal, cargo_crystal, cargo_deuterium,
                speed_percent, interval_secs, next_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                route.id as i64,
                route.origin.galaxy as i64,
                route.origin.system as i64,
                route.origin.position as i64,
                route.dest.galaxy as i64,
                route.dest.system as i64,
                route.dest.position as i64,
                route.cargo.metal,
                route.cargo.crystal,
                route.cargo.deuterium,
                route.speed_percent as i64,
                route.interval_secs as i64,
                route.next_at as i64,
            ],
        )?;
        for kind in ShipKind::ALL {
            let count = route.ships.get(kind);
            if count == 0 {
                continue;
            }
            tx.execute(
                "INSERT INTO route_ships (route_id, kind, count) VALUES (?1, ?2, ?3)",
                params![route.id as i64, kind.as_key(), count],
            )?;
        }
    }
    tx.execute(
        "INSERT INTO activity_boosts (id, prod_until, prod_factor, research_until, research_factor)
         VALUES (1, ?1, ?2, ?3, ?4)",
        params![
            state.activity.prod_boost_until.map(|t| t as i64),
            state.activity.prod_boost_factor,
            state.activity.research_boost_until.map(|t| t as i64),
            state.activity.research_boost_factor,
        ],
    )?;
    for channel in ActivityChannel::ALL {
        let i = channel.index();
        tx.execute(
            "INSERT INTO activity_limits (channel, last_applied, day_start, day_count)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                channel.as_key(),
                state.activity.last_applied[i].map(|t| t as i64),
                state.activity.day_start as i64,
                state.activity.day_count[i] as i64,
            ],
        )?;
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
    if let Some(pending) = &state.story_pending {
        tx.execute(
            "INSERT INTO story_pending (id, event_id, offered_at, timeout_at) VALUES (1, ?1, ?2, ?3)",
            params![
                pending.event_id,
                pending.offered_at as i64,
                pending.timeout_at as i64,
            ],
        )?;
    }
    for kind in crate::ResourceKind::ALL {
        tx.execute(
            "INSERT INTO market_prices (kind, price) VALUES (?1, ?2)",
            params![kind.as_key(), state.market.price(kind)],
        )?;
        for (i, price) in state.market.history(kind).iter().enumerate() {
            tx.execute(
                "INSERT INTO market_history (kind, position, price) VALUES (?1, ?2, ?3)",
                params![kind.as_key(), i as i64, price],
            )?;
        }
    }
    for (id, standing) in &state.diplomacy.standing {
        tx.execute(
            "INSERT INTO faction_standing (id, standing) VALUES (?1, ?2)",
            params![id, standing],
        )?;
    }
    for (i, change) in state.diplomacy.log.iter().enumerate() {
        tx.execute(
            "INSERT INTO faction_log (position, at, faction, delta, reason) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                i as i64,
                change.at as i64,
                change.faction,
                change.delta,
                change.reason,
            ],
        )?;
    }
    for report in &state.spy_reports {
        tx.execute(
            "INSERT INTO spy_reports (
                id, galaxy, system, position, created_at, probes_sent, probes_lost,
                detected, score, tier, metal, crystal, deuterium, garrison, kind, size,
                revealed_resources, revealed_fleet, revealed_defences, revealed_garrison
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)",
            params![
                report.id as i64,
                report.target.galaxy as i64,
                report.target.system as i64,
                report.target.position as i64,
                report.created_at as i64,
                report.probes_sent as i64,
                report.probes_lost as i64,
                i64::from(report.detected),
                report.score,
                report.tier,
                report.resources.map(|r| r.metal).unwrap_or(0.0),
                report.resources.map(|r| r.crystal).unwrap_or(0.0),
                report.resources.map(|r| r.deuterium).unwrap_or(0.0),
                report.garrison.map(|g| g as i64).unwrap_or(0),
                report.kind.map(|k| k.as_key()),
                report.size.map(|s| s as i64),
                i64::from(report.resources.is_some()),
                i64::from(report.fleet.is_some()),
                i64::from(report.defences.is_some()),
                i64::from(report.garrison.is_some()),
            ],
        )?;
        if let Some(fleet) = report.fleet {
            insert_spy_ships(&tx, report.id, "fleet", fleet)?;
        }
        if let Some(defences) = report.defences {
            insert_spy_ships(&tx, report.id, "defences", defences)?;
        }
    }
    insert_laws(&tx, state)?;
    for (id, unlocked_at) in &state.achievements {
        tx.execute(
            "INSERT INTO achievements (id, unlocked_at) VALUES (?1, ?2)",
            params![id, *unlocked_at as i64],
        )?;
    }
    tx.execute(
        "INSERT INTO achievement_stats (id, raids_survived, outposts_conquered) VALUES (1, ?1, ?2)",
        params![state.raids_survived, state.outposts_conquered],
    )?;
    insert_combat_logs(&tx, state)?;
    insert_daily(&tx, state)?;
    for (i, colony) in state.colonies.iter().enumerate() {
        let planet_id = (i as i64) + 2;
        tx.execute(
            "INSERT INTO colonies (planet_id, galaxy, system, position, kind, size, rare)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                planet_id,
                colony.coord.galaxy as i64,
                colony.coord.system as i64,
                colony.coord.position as i64,
                colony.kind.as_key(),
                colony.size as i64,
                colony.rare.as_deref(),
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
        insert_goods(&tx, planet_id, &colony.goods)?;
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
        insert_defences(&tx, planet_id, colony.defences)?;
        insert_build_queue(&tx, planet_id, &colony.build_queue)?;
    }
    if let Some(job) = &state.wonder {
        tx.execute(
            "INSERT INTO wonder_job (
                id, wonder_id, stage, metal, crystal, deuterium, completes_at
             ) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                job.id,
                job.stage as i64,
                job.contributed.metal,
                job.contributed.crystal,
                job.contributed.deuterium,
                job.completes_at.map(|t| t as i64),
            ],
        )?;
    }
    for (i, id) in state.completed_wonders.iter().enumerate() {
        tx.execute(
            "INSERT INTO wonder_done (position, wonder_id) VALUES (?1, ?2)",
            params![i as i64, id],
        )?;
    }
    for (i, outpost) in state.outposts.iter().enumerate() {
        let planet_id = (i as i64) + 1;
        tx.execute(
            "INSERT INTO outposts (
                planet_id, galaxy, system, position, kind, size, garrison, rare, last_rebuild_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                planet_id,
                outpost.coord.galaxy as i64,
                outpost.coord.system as i64,
                outpost.coord.position as i64,
                outpost.kind.as_key(),
                outpost.size as i64,
                outpost.garrison as i64,
                outpost.rare.as_deref(),
                outpost.last_rebuild_at as i64,
            ],
        )?;
        for kind in ShipKind::ALL {
            let count = outpost.defences.get(kind);
            if count == 0 {
                continue;
            }
            tx.execute(
                "INSERT INTO outpost_ships (planet_id, kind, count) VALUES (?1, ?2, ?3)",
                params![planet_id, kind.as_key(), count],
            )?;
        }
    }
    tx.execute(
        "INSERT INTO prestige (
            id, currency, lifetime, run_index, run_started_at
         ) VALUES (1, ?1, ?2, ?3, ?4)",
        params![
            state.prestige.currency as i64,
            state.prestige.lifetime as i64,
            state.prestige.current.index as i64,
            state.prestige.current.started_at as i64,
        ],
    )?;
    for (id, count) in &state.prestige.owned {
        tx.execute(
            "INSERT INTO prestige_owned (bonus_id, count) VALUES (?1, ?2)",
            params![id, *count as i64],
        )?;
    }
    for (i, run) in state.prestige.runs.iter().enumerate() {
        tx.execute(
            "INSERT INTO prestige_runs (
                position, run_index, started_at, ended_at, score, prestige_earned,
                wonders, colonies, building_levels, research_levels
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                i as i64,
                run.index as i64,
                run.started_at as i64,
                run.ended_at.map(|t| t as i64),
                run.score as i64,
                run.prestige_earned as i64,
                run.wonders as i64,
                run.colonies as i64,
                run.building_levels as i64,
                run.research_levels as i64,
            ],
        )?;
    }
    tx.commit()?;
    Ok(())
}

fn insert_spy_ships(
    conn: &Connection,
    report_id: u64,
    role: &str,
    fleet: Fleet,
) -> Result<(), SaveError> {
    for kind in ShipKind::ALL {
        let count = fleet.get(kind);
        if count == 0 {
            continue;
        }
        conn.execute(
            "INSERT INTO spy_report_ships (report_id, role, kind, count) VALUES (?1, ?2, ?3, ?4)",
            params![report_id as i64, role, kind.as_key(), count],
        )?;
    }
    Ok(())
}

fn insert_defences(
    conn: &Connection,
    planet_id: i64,
    grid: crate::DefenceGrid,
) -> Result<(), SaveError> {
    for kind in DefenceKind::ALL {
        let count = grid.get(kind);
        if count == 0 {
            continue;
        }
        conn.execute(
            "INSERT INTO planet_defences (planet_id, kind, count) VALUES (?1, ?2, ?3)",
            params![planet_id, kind.as_key(), count],
        )?;
    }
    Ok(())
}

fn read_defences(conn: &Connection, planet_id: i64) -> Result<crate::DefenceGrid, SaveError> {
    let mut grid = crate::DefenceGrid::default();
    let mut stmt = conn.prepare("SELECT kind, count FROM planet_defences WHERE planet_id = ?1")?;
    let rows = stmt.query_map(params![planet_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
    })?;
    for row in rows {
        let (key, count) = row?;
        let kind = DefenceKind::from_key(&key).ok_or(SaveError::Corrupt("unknown defence kind"))?;
        grid.set(kind, count);
    }
    Ok(grid)
}

fn insert_goods(
    conn: &Connection,
    planet_id: i64,
    goods: &BTreeMap<String, f64>,
) -> Result<(), SaveError> {
    for (good, amount) in goods {
        if !amount.is_finite() || *amount <= 0.0 {
            continue;
        }
        conn.execute(
            "INSERT INTO goods (planet_id, good, amount) VALUES (?1, ?2, ?3)",
            params![planet_id, good, amount],
        )?;
    }
    Ok(())
}

fn read_goods(conn: &Connection, planet_id: i64) -> Result<BTreeMap<String, f64>, SaveError> {
    let mut stmt = conn.prepare("SELECT good, amount FROM goods WHERE planet_id = ?1")?;
    let rows = stmt.query_map(params![planet_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
    })?;
    let mut goods = BTreeMap::new();
    for row in rows {
        let (good, amount) = row?;
        goods.insert(good, amount);
    }
    Ok(goods)
}

fn insert_build_queue(
    conn: &Connection,
    planet_id: i64,
    queue: &[BuildJob],
) -> Result<(), SaveError> {
    for (i, job) in queue.iter().enumerate() {
        conn.execute(
            "INSERT INTO build_queue (
                planet_id, position, kind, from_level,
                paid_metal, paid_crystal, paid_deuterium,
                started_at, completes_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                planet_id,
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
    Ok(())
}

/// Reconstruct player state. `config` is the caller's rules; it is not stored in the save.
pub fn load_state(conn: &Connection, config: BalanceConfig) -> Result<GameState, SaveError> {
    let mut state = GameState::with_config(0, config);
    let (last_tick, rng_seed, galaxy_seed, home_kind, home_size, deterministic, outposts_seeded, generator_version, last_story_at, last_market_at, last_diplomacy_at): (
        i64,
        i64,
        i64,
        String,
        i64,
        i64,
        i64,
        i64,
        i64,
        i64,
        i64,
    ) = conn
        .query_row(
            "SELECT last_tick_unix, rng_seed, galaxy_seed, home_kind, home_size, deterministic, outposts_seeded, generator_version, last_story_at, last_market_at, last_diplomacy_at FROM meta WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?, row.get(8)?, row.get(9)?, row.get(10)?)),
        )
        .optional()?
        .ok_or(SaveError::Corrupt("missing meta row"))?;
    state.last_tick_unix = unix_ts(last_tick)?;
    state.rng_seed = rng_seed as u64;
    state.galaxy_seed = galaxy_seed as u64;
    state.last_story_at = unix_ts(last_story_at)?;
    state.last_market_at = unix_ts(last_market_at)?;
    state.last_diplomacy_at = unix_ts(last_diplomacy_at)?;
    state.generator_version = u32_field(generator_version, "generator version")?;
    if !crate::supports_galaxy_generator(state.generator_version) {
        return Err(SaveError::GeneratorTooNew {
            found: state.generator_version,
            current: crate::GALAXY_GENERATOR_VERSION,
        });
    }
    state.home_kind =
        PlanetType::from_key(&home_kind).ok_or(SaveError::Corrupt("unknown home planet type"))?;
    state.home_size = u8::try_from(home_size).map_err(|_| SaveError::Corrupt("home size"))?;
    state.deterministic = deterministic != 0;

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
    state.goods = read_goods(conn, 1)?;

    let mut buildings = Buildings::default();
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

    state.build_queue = read_build_queue(conn, 1)?;

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

    let mut stmt = conn.prepare("SELECT group_id, tech_id FROM exclusive_picks")?;
    let picks = stmt.query_map([], |row| {
        let group: String = row.get(0)?;
        let tech: String = row.get(1)?;
        Ok((group, tech))
    })?;
    state.exclusive_picks.clear();
    for row in picks {
        let (group, tech) = row?;
        state.exclusive_picks.insert(group, tech);
    }
    state.infer_exclusive_picks();

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
    state.defences = read_defences(conn, 1)?;

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
        let (ship, defence) = if let Some(ship) = ShipKind::from_key(&kind) {
            (Some(ship), None)
        } else if let Some(defence) = DefenceKind::from_key(&kind) {
            (None, Some(defence))
        } else {
            return Err(SaveError::Corrupt("unknown yard kind"));
        };
        queue.push(ShipJob {
            ship,
            defence,
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
                phase_ends_at, fuel_paid, colonize, expedition_seed, expedition_applied,
                cargo_metal, cargo_crystal, cargo_deuterium, transport, attack, invade, spy
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
            row.get::<_, Option<i64>>(15)?,
            row.get::<_, i64>(16)?,
            row.get::<_, f64>(17)?,
            row.get::<_, f64>(18)?,
            row.get::<_, f64>(19)?,
            row.get::<_, i64>(20)?,
            row.get::<_, i64>(21)?,
            row.get::<_, i64>(22)?,
            row.get::<_, i64>(23)?,
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
            expedition_seed,
            expedition_applied,
            cargo_metal,
            cargo_crystal,
            cargo_deuterium,
            transport,
            attack,
            invade,
            spy,
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
            expedition_seed: expedition_seed.map(|s| s as u64),
            expedition_applied: expedition_applied != 0,
            cargo: Resources {
                metal: cargo_metal,
                crystal: cargo_crystal,
                deuterium: cargo_deuterium,
            },
            transport: transport != 0,
            attack: attack != 0,
            invade: invade != 0,
            spy: spy != 0,
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

    state.routes = read_routes(conn)?;
    state.next_route_id = state
        .routes
        .iter()
        .map(|r| r.id)
        .max()
        .map(|id| id.saturating_add(1))
        .unwrap_or(0);

    let raid_row: Option<Option<i64>> = conn
        .query_row("SELECT next_raid_at FROM raids WHERE id = 1", [], |row| {
            row.get::<_, Option<i64>>(0)
        })
        .optional()?;
    state.next_raid_at = match raid_row {
        Some(Some(ts)) => Some(unix_ts(ts)?),
        Some(None) => None,
        None if state.config.raid.enabled => Some(
            state
                .last_tick_unix
                .saturating_add(state.config.raid.first_delay_secs),
        ),
        None => None,
    };

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

    state.story_pending = match conn
        .query_row(
            "SELECT event_id, offered_at, timeout_at FROM story_pending WHERE id = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()?
    {
        Some((event_id, offered_at, timeout_at)) => Some(crate::PendingStory {
            event_id,
            offered_at: unix_ts(offered_at)?,
            timeout_at: unix_ts(timeout_at)?,
        }),
        None => None,
    };

    load_market(conn, &mut state)?;
    load_diplomacy(conn, &mut state)?;
    load_spy_reports(conn, &mut state)?;
    load_laws(conn, &mut state)?;
    load_achievements(conn, &mut state)?;
    load_combat_logs(conn, &mut state)?;
    load_daily(conn, &mut state)?;
    state.sync_raid_schedule_after_standing(state.last_tick_unix);

    let mut stmt = conn.prepare(
        "SELECT planet_id, galaxy, system, position, kind, size, rare FROM colonies ORDER BY planet_id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, i64>(5)?,
            row.get::<_, Option<String>>(6)?,
        ))
    })?;
    let mut pending = Vec::new();
    for row in rows {
        pending.push(row?);
    }
    drop(stmt);
    state.colonies.clear();
    for (planet_id, g, s, p, kind, size, rare) in pending {
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
            defences: read_defences(conn, planet_id)?,
            build_queue: read_build_queue(conn, planet_id)?,
            rare,
            goods: read_goods(conn, planet_id)?,
        });
    }

    state.activity = ActivityTracker::default();
    if let Some((prod_until, prod_factor, research_until, research_factor)) = conn
        .query_row(
            "SELECT prod_until, prod_factor, research_until, research_factor FROM activity_boosts WHERE id = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, Option<i64>>(0)?,
                    row.get::<_, f64>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, f64>(3)?,
                ))
            },
        )
        .optional()?
    {
        state.activity.prod_boost_until = prod_until.map(unix_ts).transpose()?;
        state.activity.prod_boost_factor = prod_factor;
        state.activity.research_boost_until = research_until.map(unix_ts).transpose()?;
        state.activity.research_boost_factor = research_factor;
    }
    let mut stmt =
        conn.prepare("SELECT channel, last_applied, day_start, day_count FROM activity_limits")?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<i64>>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
        ))
    })?;
    for row in rows {
        let (key, last, day, count) = row?;
        let Some(channel) = ActivityChannel::from_key(&key) else {
            continue;
        };
        let i = channel.index();
        state.activity.last_applied[i] = last.map(unix_ts).transpose()?;
        state.activity.day_start = unix_ts(day)?;
        state.activity.day_count[i] =
            u32::try_from(count).map_err(|_| SaveError::Corrupt("activity day count"))?;
    }

    state.wonder = match conn
        .query_row(
            "SELECT wonder_id, stage, metal, crystal, deuterium, completes_at
             FROM wonder_job WHERE id = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, f64>(2)?,
                    row.get::<_, f64>(3)?,
                    row.get::<_, f64>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                ))
            },
        )
        .optional()?
    {
        None => None,
        Some((id, stage, metal, crystal, deuterium, completes_at)) => Some(WonderJob {
            id,
            stage: usize::try_from(stage).map_err(|_| SaveError::Corrupt("wonder stage"))?,
            contributed: Resources {
                metal,
                crystal,
                deuterium,
            },
            completes_at: unix_ts_opt(completes_at)?,
        }),
    };
    let mut stmt = conn.prepare("SELECT wonder_id FROM wonder_done ORDER BY position")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    state.completed_wonders.clear();
    for row in rows {
        state.completed_wonders.push(row?);
    }

    state.outposts = read_outposts(conn)?;
    state.prestige = read_prestige(conn, state.last_tick_unix)?;
    if outposts_seeded == 0 && state.outposts.is_empty() {
        let mut occupied = vec![state.home];
        occupied.extend(state.colonies.iter().map(|c| c.coord));
        state.outposts = generate_outposts(
            state.galaxy_seed,
            state.last_tick_unix,
            &state.config.outposts,
            &occupied,
            state.generator_version,
        );
    }

    Ok(state)
}

fn read_outposts(conn: &Connection) -> Result<Vec<Outpost>, SaveError> {
    let mut stmt = conn.prepare(
        "SELECT planet_id, galaxy, system, position, kind, size, garrison, rare, last_rebuild_at
         FROM outposts ORDER BY planet_id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, i64>(5)?,
            row.get::<_, i64>(6)?,
            row.get::<_, Option<String>>(7)?,
            row.get::<_, i64>(8)?,
        ))
    })?;
    let mut pending = Vec::new();
    for row in rows {
        pending.push(row?);
    }
    drop(stmt);
    let mut outposts = Vec::new();
    for (planet_id, g, s, p, kind, size, garrison, rare, last_rebuild_at) in pending {
        let kind =
            PlanetType::from_key(&kind).ok_or(SaveError::Corrupt("unknown outpost planet type"))?;
        let mut defences = Fleet::default();
        let mut sstmt =
            conn.prepare("SELECT kind, count FROM outpost_ships WHERE planet_id = ?1")?;
        let srows = sstmt.query_map(params![planet_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
        })?;
        for srow in srows {
            let (key, count) = srow?;
            let skind = ShipKind::from_key(&key).ok_or(SaveError::Corrupt("unknown ship kind"))?;
            defences.add(skind, count);
        }
        outposts.push(Outpost {
            coord: Coord {
                galaxy: u16_coord(g)?,
                system: u16_coord(s)?,
                position: u16_coord(p)?,
            },
            kind,
            size: u8::try_from(size).map_err(|_| SaveError::Corrupt("outpost size"))?,
            defences,
            garrison: u32::try_from(garrison)
                .map_err(|_| SaveError::Corrupt("outpost garrison"))?,
            rare,
            last_rebuild_at: unix_ts(last_rebuild_at)?,
        });
    }
    Ok(outposts)
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
        let leftover = leftover_rows(conn)?;
        if leftover != 0 {
            return Err(SaveError::Corrupt("missing meta row"));
        }
        let state = GameState::with_config(now_unix, config);
        save_state(conn, &state)?;
        return Ok(state);
    }
    load_state(conn, config)
}

/// Like [`load_or_new`], but a new save with `seed` is deterministic.
pub fn load_or_new_with_seed(
    conn: &mut Connection,
    now_unix: u64,
    config: BalanceConfig,
    seed: u64,
) -> Result<GameState, SaveError> {
    let meta: i64 = count_rows(conn, "meta")?;
    if meta == 0 {
        let leftover = leftover_rows(conn)?;
        if leftover != 0 {
            return Err(SaveError::Corrupt("missing meta row"));
        }
        let state = GameState::with_seed(now_unix, config, seed);
        save_state(conn, &state)?;
        return Ok(state);
    }
    load_state(conn, config)
}

/// Advisory exclusive lock beside the save. Held for the TUI session or one CLI command.
#[derive(Debug)]
pub struct SaveLock {
    _file: File,
}

pub fn save_lock_path(save: &Path) -> PathBuf {
    let mut path = save.as_os_str().to_os_string();
    path.push(".lock");
    PathBuf::from(path)
}

/// True when another process already holds the exclusive save lock.
///
/// Opens the lock file only to probe. The caller does not keep the lock.
pub fn save_is_locked(path: &Path) -> bool {
    if path == Path::new(":memory:") {
        return false;
    }
    let lock_path = save_lock_path(&save_identity(path));
    let mut opts = OpenOptions::new();
    opts.read(true).write(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // Same exclusive share as lock_save: a held lock fails the open.
        opts.share_mode(0);
    }
    match opts.open(&lock_path) {
        Ok(file) => {
            #[cfg(unix)]
            {
                let fd = std::os::unix::io::AsRawFd::as_raw_fd(&file);
                let rc = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
                if rc == 0 {
                    unsafe {
                        libc::flock(fd, libc::LOCK_UN);
                    }
                    false
                } else {
                    true
                }
            }
            #[cfg(windows)]
            {
                let _ = file;
                false
            }
        }
        Err(err) => is_sharing_violation(&err),
    }
}

/// Read-only glance at a save. Does not migrate, create, or take the write lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveGlance {
    pub schema: u32,
    pub last_tick_unix: u64,
    pub deterministic: bool,
    pub planets: u32,
    pub fleet_strength: u32,
    /// `None` when the save predates schema v5 and never stored a galaxy seed.
    pub galaxy_seed: Option<u64>,
    pub generator_version: u32,
}

pub fn inspect_save(path: &Path) -> Result<SaveGlance, SaveError> {
    if save_is_locked(path) {
        return Err(SaveError::Locked);
    }
    if !path.exists() {
        return Err(SaveError::Corrupt("missing save"));
    }
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let schema: u32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if schema > SCHEMA_VERSION {
        return Err(SaveError::SchemaTooNew {
            found: schema,
            current: SCHEMA_VERSION,
        });
    }
    if schema >= 14 {
        let stored = conn
            .query_row(
                "SELECT generator_version FROM meta WHERE id = 1",
                [],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        if let Some(stored) = stored {
            let stored = u32_field(stored, "generator version")?;
            if !crate::supports_galaxy_generator(stored) {
                return Err(SaveError::GeneratorTooNew {
                    found: stored,
                    current: crate::GALAXY_GENERATOR_VERSION,
                });
            }
        }
    }
    let last_tick: i64 = conn
        .query_row("SELECT last_tick_unix FROM meta WHERE id = 1", [], |row| {
            row.get(0)
        })
        .optional()?
        .ok_or(SaveError::Corrupt("missing meta row"))?;
    let deterministic = conn
        .query_row("SELECT deterministic FROM meta WHERE id = 1", [], |row| {
            row.get::<_, i64>(0)
        })
        .optional()
        .ok()
        .flatten()
        .unwrap_or(0)
        != 0;
    let colonies = if table_exists(&conn, "colonies") {
        u32::try_from(count_rows(&conn, "colonies")?).unwrap_or(0)
    } else {
        0
    };
    let fleet = if table_exists(&conn, "ships") {
        conn.query_row("SELECT COALESCE(SUM(count), 0) FROM ships", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap_or(0)
    } else {
        0
    };
    let galaxy_seed = if schema >= 5 {
        Some(
            conn.query_row("SELECT galaxy_seed FROM meta WHERE id = 1", [], |row| {
                row.get::<_, i64>(0)
            })
            .optional()?
            .unwrap_or(0) as u64,
        )
    } else {
        None
    };
    let generator_version = if schema >= 14 {
        conn.query_row(
            "SELECT generator_version FROM meta WHERE id = 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .and_then(|v| u32::try_from(v).ok())
        .unwrap_or(1)
    } else {
        1
    };
    Ok(SaveGlance {
        schema,
        last_tick_unix: unix_ts(last_tick)?,
        deterministic,
        planets: 1 + colonies,
        fleet_strength: u32::try_from(fleet).unwrap_or(0),
        galaxy_seed,
        generator_version,
    })
}

/// Read-only career totals from one save. Does not migrate, create, or take the write lock.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CareerGlance {
    pub unlocks: BTreeMap<String, u64>,
    pub playtime_secs: u64,
    pub battles: u32,
    pub wonders: u32,
    pub galaxies_wonder: u32,
    pub galaxies_score: u32,
    pub prestige_runs: u32,
    pub prestige: Prestige,
    pub prestige_currency: u32,
    pub prestige_lifetime: u32,
    pub prestige_owned: BTreeMap<String, u32>,
    pub building_levels: u32,
    pub research_levels: u32,
    pub colonies: u32,
    pub fleet_ships: u32,
    pub raids_survived: u32,
    pub outposts_conquered: u32,
    pub wonder_ids: std::collections::BTreeSet<String>,
}

impl CareerGlance {
    pub fn meters(&self) -> crate::CareerMeters {
        crate::CareerMeters {
            colonies: self.colonies,
            fleet_ships: self.fleet_ships,
            prestige_runs: self.prestige_runs,
            raids_survived: self.raids_survived,
            outposts_conquered: self.outposts_conquered,
            wonders: self.wonders,
            wonder_ids: self.wonder_ids.clone(),
        }
    }

    pub fn merge(&mut self, other: Self) {
        for (id, at) in other.unlocks {
            self.unlocks
                .entry(id)
                .and_modify(|had| *had = (*had).min(at))
                .or_insert(at);
        }
        self.playtime_secs = self.playtime_secs.saturating_add(other.playtime_secs);
        self.battles = self.battles.saturating_add(other.battles);
        self.wonders = self.wonders.saturating_add(other.wonders);
        self.galaxies_wonder = self.galaxies_wonder.saturating_add(other.galaxies_wonder);
        self.galaxies_score = self.galaxies_score.saturating_add(other.galaxies_score);
        self.prestige_runs = self.prestige_runs.max(other.prestige_runs);
        let take_other = other.prestige.lifetime > self.prestige.lifetime
            || (other.prestige.lifetime == self.prestige.lifetime
                && other.prestige.currency > self.prestige.currency);
        if take_other {
            self.prestige = other.prestige;
        }
        self.prestige_currency = self.prestige_currency.max(other.prestige_currency);
        self.prestige_lifetime = self.prestige_lifetime.max(other.prestige_lifetime);
        for (id, count) in other.prestige_owned {
            self.prestige_owned
                .entry(id)
                .and_modify(|had| *had = (*had).max(count))
                .or_insert(count);
        }
        self.building_levels = self.building_levels.max(other.building_levels);
        self.research_levels = self.research_levels.max(other.research_levels);
        self.colonies = self.colonies.max(other.colonies);
        self.fleet_ships = self.fleet_ships.max(other.fleet_ships);
        self.raids_survived = self.raids_survived.max(other.raids_survived);
        self.outposts_conquered = self.outposts_conquered.max(other.outposts_conquered);
        self.wonder_ids.extend(other.wonder_ids);
    }

    /// Eligibility for this glance alone. Do not use merged career counts:
    /// those are maxima across saves and would fabricate a reset.
    pub fn reset_available(&self, cfg: &crate::PrestigeConfig) -> bool {
        if self.prestige_runs > 0 || self.prestige_lifetime > 0 || !self.prestige.owned.is_empty() {
            return true;
        }
        cfg.can_reset(
            self.wonder_ids.len() as u32,
            cfg.run_score(
                self.building_levels,
                self.research_levels,
                self.colonies,
                self.wonder_ids.len() as u32,
            ),
        )
    }
}

pub fn inspect_career(path: &Path) -> Result<CareerGlance, SaveError> {
    if save_is_locked(path) {
        return Err(SaveError::Locked);
    }
    if !path.exists() {
        return Err(SaveError::Corrupt("missing save"));
    }
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let schema: u32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if schema > SCHEMA_VERSION {
        return Err(SaveError::SchemaTooNew {
            found: schema,
            current: SCHEMA_VERSION,
        });
    }
    let last_tick: i64 = conn
        .query_row("SELECT last_tick_unix FROM meta WHERE id = 1", [], |row| {
            row.get(0)
        })
        .optional()?
        .ok_or(SaveError::Corrupt("missing meta row"))?;
    let last_tick = unix_ts(last_tick)?;
    let prestige = if table_exists(&conn, "prestige") {
        read_prestige(&conn, last_tick)?
    } else {
        Prestige::new(last_tick)
    };
    let mut playtime_secs = last_tick.saturating_sub(prestige.current.started_at);
    let mut galaxies_wonder = 0u32;
    let mut galaxies_score = 0u32;
    let mut past_wonders = 0u32;
    for run in &prestige.runs {
        let ended = run.ended_at.unwrap_or(last_tick);
        playtime_secs = playtime_secs.saturating_add(ended.saturating_sub(run.started_at));
        past_wonders = past_wonders.saturating_add(run.wonders);
        if run.wonders > 0 {
            galaxies_wonder += 1;
        } else {
            galaxies_score += 1;
        }
    }
    let mut wonder_ids = std::collections::BTreeSet::new();
    if table_exists(&conn, "wonder_done") {
        let mut stmt = conn.prepare("SELECT wonder_id FROM wonder_done")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        for row in rows {
            wonder_ids.insert(row?);
        }
    }
    let current_wonders = wonder_ids.len() as u32;
    let building_levels = if table_exists(&conn, "buildings") {
        conn.query_row("SELECT COALESCE(SUM(level), 0) FROM buildings", [], |row| {
            row.get::<_, i64>(0)
        })
        .ok()
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(0)
    } else {
        0
    };
    let research_levels = if table_exists(&conn, "tech_levels") {
        conn.query_row(
            "SELECT COALESCE(SUM(level), 0) FROM tech_levels",
            [],
            |row| row.get::<_, i64>(0),
        )
        .ok()
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(0)
    } else {
        0
    };
    let colonies = if table_exists(&conn, "colonies") {
        u32::try_from(count_rows(&conn, "colonies")?).unwrap_or(0)
    } else {
        0
    };
    let fleet_ships = if table_exists(&conn, "ships") {
        conn.query_row("SELECT COALESCE(SUM(count), 0) FROM ships", [], |row| {
            row.get::<_, i64>(0)
        })
        .ok()
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(0)
    } else {
        0
    };
    let battles = if table_exists(&conn, "combat_logs") {
        u32::try_from(count_rows(&conn, "combat_logs")?).unwrap_or(0)
    } else {
        0
    };
    let mut unlocks = BTreeMap::new();
    if table_exists(&conn, "achievements") {
        let mut stmt = conn.prepare("SELECT id, unlocked_at FROM achievements")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        for row in rows {
            let (id, unlocked_at) = row?;
            unlocks.insert(id, unix_ts(unlocked_at)?);
        }
    }
    let mut raids_survived = 0;
    let mut outposts_conquered = 0;
    if table_exists(&conn, "achievement_stats")
        && let Some((raids, conquests)) = conn
            .query_row(
                "SELECT raids_survived, outposts_conquered FROM achievement_stats WHERE id = 1",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()?
    {
        raids_survived = u32_field(raids, "raids survived")?;
        outposts_conquered = u32_field(conquests, "outposts conquered")?;
    }
    Ok(CareerGlance {
        unlocks,
        playtime_secs,
        battles,
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
        colonies,
        fleet_ships,
        raids_survived,
        outposts_conquered,
        wonder_ids,
    })
}

fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [name],
        |_| Ok(()),
    )
    .optional()
    .ok()
    .flatten()
    .is_some()
}

/// Non-blocking exclusive lock. [`SaveError::Locked`] when another process holds it.
pub fn lock_save(path: &Path) -> Result<SaveLock, SaveError> {
    if path == Path::new(":memory:") {
        let path = std::env::temp_dir().join(format!("hoshi-mem-{}.lock", std::process::id()));
        return Ok(SaveLock {
            _file: open_lock_file(&path)?,
        });
    }
    let path = save_identity(path);
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    Ok(SaveLock {
        _file: open_lock_file(&save_lock_path(&path))?,
    })
}

/// Same save file through different names (relative, absolute, symlink) shares one lock.
fn save_identity(path: &Path) -> PathBuf {
    let path = follow_save_symlinks(path);
    if let Ok(real) = path.canonicalize() {
        return real;
    }
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) if !parent.as_os_str().is_empty() => match parent.canonicalize()
        {
            Ok(real_parent) => real_parent.join(name),
            Err(_) => path,
        },
        _ => path,
    }
}

fn follow_save_symlinks(path: &Path) -> PathBuf {
    let mut current = path.to_path_buf();
    for _ in 0..8 {
        match std::fs::read_link(&current) {
            Ok(target) => {
                current = if target.is_absolute() {
                    target
                } else {
                    match current.parent() {
                        Some(parent) if !parent.as_os_str().is_empty() => parent.join(target),
                        _ => target,
                    }
                };
            }
            Err(_) => break,
        }
    }
    current
}

#[cfg(not(any(unix, windows)))]
compile_error!("save locking requires unix flock or windows exclusive share");

fn open_lock_file(path: &Path) -> Result<File, SaveError> {
    let mut opts = OpenOptions::new();
    opts.create(true).write(true).truncate(false);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_SHARE_NONE: a second open fails with ERROR_SHARING_VIOLATION.
        opts.share_mode(0);
    }
    let file = match opts.open(path) {
        Ok(file) => file,
        Err(err) => return Err(lock_open_error(err)),
    };
    #[cfg(unix)]
    {
        let fd = std::os::unix::io::AsRawFd::as_raw_fd(&file);
        let rc = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
        if rc != 0 {
            return Err(SaveError::Locked);
        }
    }
    Ok(file)
}

fn lock_open_error(err: std::io::Error) -> SaveError {
    if is_sharing_violation(&err) {
        SaveError::Locked
    } else {
        SaveError::Io(err)
    }
}

fn is_sharing_violation(err: &std::io::Error) -> bool {
    #[cfg(windows)]
    {
        // ERROR_SHARING_VIOLATION
        err.raw_os_error() == Some(32)
    }
    #[cfg(not(windows))]
    {
        let _ = err;
        false
    }
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
        "exclusive_picks" => "SELECT COUNT(*) FROM exclusive_picks",
        "research" => "SELECT COUNT(*) FROM research",
        "ships" => "SELECT COUNT(*) FROM ships",
        "ship_queue" => "SELECT COUNT(*) FROM ship_queue",
        "missions" => "SELECT COUNT(*) FROM missions",
        "mission_ships" => "SELECT COUNT(*) FROM mission_ships",
        "raids" => "SELECT COUNT(*) FROM raids",
        "event_log" => "SELECT COUNT(*) FROM event_log",
        "colonies" => "SELECT COUNT(*) FROM colonies",
        "activity_boosts" => "SELECT COUNT(*) FROM activity_boosts",
        "activity_limits" => "SELECT COUNT(*) FROM activity_limits",
        "routes" => "SELECT COUNT(*) FROM routes",
        "route_ships" => "SELECT COUNT(*) FROM route_ships",
        "wonder_job" => "SELECT COUNT(*) FROM wonder_job",
        "wonder_done" => "SELECT COUNT(*) FROM wonder_done",
        "outposts" => "SELECT COUNT(*) FROM outposts",
        "outpost_ships" => "SELECT COUNT(*) FROM outpost_ships",
        "prestige" => "SELECT COUNT(*) FROM prestige",
        "prestige_owned" => "SELECT COUNT(*) FROM prestige_owned",
        "prestige_runs" => "SELECT COUNT(*) FROM prestige_runs",
        "goods" => "SELECT COUNT(*) FROM goods",
        "story_pending" => "SELECT COUNT(*) FROM story_pending",
        "market_prices" => "SELECT COUNT(*) FROM market_prices",
        "market_history" => "SELECT COUNT(*) FROM market_history",
        "faction_standing" => "SELECT COUNT(*) FROM faction_standing",
        "faction_log" => "SELECT COUNT(*) FROM faction_log",
        "spy_reports" => "SELECT COUNT(*) FROM spy_reports",
        "spy_report_ships" => "SELECT COUNT(*) FROM spy_report_ships",
        "planet_defences" => "SELECT COUNT(*) FROM planet_defences",
        "laws" => "SELECT COUNT(*) FROM laws",
        "law_approvals" => "SELECT COUNT(*) FROM law_approvals",
        "law_actions" => "SELECT COUNT(*) FROM law_actions",
        "law_samples" => "SELECT COUNT(*) FROM law_samples",
        "achievements" => "SELECT COUNT(*) FROM achievements",
        "achievement_stats" => "SELECT COUNT(*) FROM achievement_stats",
        "combat_logs" => "SELECT COUNT(*) FROM combat_logs",
        "combat_log_frames" => "SELECT COUNT(*) FROM combat_log_frames",
        "combat_log_ships" => "SELECT COUNT(*) FROM combat_log_ships",
        "daily_meta" => "SELECT COUNT(*) FROM daily_meta",
        "daily_slots" => "SELECT COUNT(*) FROM daily_slots",
        _ => return Err(SaveError::Corrupt("unknown save table")),
    };
    Ok(conn.query_row(sql, [], |row| row.get(0))?)
}

fn leftover_rows(conn: &Connection) -> Result<i64, SaveError> {
    Ok(count_rows(conn, "resources")?
        + count_rows(conn, "buildings")?
        + count_rows(conn, "build_queue")?
        + count_rows(conn, "tech_levels")?
        + count_rows(conn, "exclusive_picks")?
        + count_rows(conn, "research")?
        + count_rows(conn, "ships")?
        + count_rows(conn, "ship_queue")?
        + count_rows(conn, "missions")?
        + count_rows(conn, "mission_ships")?
        + count_rows(conn, "raids")?
        + count_rows(conn, "event_log")?
        + count_rows(conn, "colonies")?
        + count_rows(conn, "activity_boosts")?
        + count_rows(conn, "activity_limits")?
        + count_rows(conn, "routes")?
        + count_rows(conn, "route_ships")?
        + count_rows(conn, "wonder_job")?
        + count_rows(conn, "wonder_done")?
        + count_rows(conn, "outposts")?
        + count_rows(conn, "outpost_ships")?
        + count_rows(conn, "prestige")?
        + count_rows(conn, "prestige_owned")?
        + count_rows(conn, "prestige_runs")?
        + count_rows(conn, "goods")?
        + count_rows(conn, "story_pending")?
        + count_rows(conn, "market_prices")?
        + count_rows(conn, "market_history")?
        + count_rows(conn, "faction_standing")?
        + count_rows(conn, "faction_log")?
        + count_rows(conn, "spy_reports")?
        + count_rows(conn, "spy_report_ships")?
        + count_rows(conn, "planet_defences")?
        + count_rows(conn, "laws")?
        + count_rows(conn, "law_approvals")?
        + count_rows(conn, "law_actions")?
        + count_rows(conn, "law_samples")?
        + count_rows(conn, "achievements")?
        + count_rows(conn, "achievement_stats")?
        + count_rows(conn, "combat_logs")?
        + count_rows(conn, "combat_log_frames")?
        + count_rows(conn, "combat_log_ships")?
        + count_rows(conn, "daily_meta")?
        + count_rows(conn, "daily_slots")?)
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
    if found < 6 {
        migrate_v6(conn)?;
    }
    if found < 7 {
        migrate_v7(conn)?;
    }
    if found < 8 {
        migrate_v8(conn)?;
    }
    if found < 9 {
        migrate_v9(conn)?;
    }
    if found < 10 {
        migrate_v10(conn)?;
    }
    if found < 11 {
        migrate_v11(conn)?;
    }
    if found < 12 {
        migrate_v12(conn)?;
    }
    if found < 13 {
        migrate_v13(conn)?;
    }
    if found < 14 {
        migrate_v14(conn)?;
    }
    if found < 15 {
        migrate_v15(conn)?;
    }
    if found < 16 {
        migrate_v16(conn)?;
    }
    if found < 17 {
        migrate_v17(conn)?;
    }
    if found < 18 {
        migrate_v18(conn)?;
    }
    if found < 19 {
        migrate_v19(conn)?;
    }
    if found < 20 {
        migrate_v20(conn)?;
    }
    if found < 21 {
        migrate_v21(conn)?;
    }
    if found < 22 {
        migrate_v22(conn)?;
    }
    if found < 23 {
        migrate_v23(conn)?;
    }
    if found < 24 {
        migrate_v24(conn)?;
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

fn migrate_v6(conn: &Connection) -> Result<(), SaveError> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS activity_boosts (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            prod_until INTEGER,
            prod_factor REAL NOT NULL DEFAULT 0,
            research_until INTEGER,
            research_factor REAL NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS activity_limits (
            channel TEXT PRIMARY KEY,
            last_applied INTEGER,
            day_start INTEGER NOT NULL DEFAULT 0,
            day_count INTEGER NOT NULL DEFAULT 0
        );
        ",
    )?;
    Ok(())
}

fn missions_has_column(conn: &Connection, column: &str) -> Result<bool, SaveError> {
    let mut stmt = conn.prepare("PRAGMA table_info(missions)")?;
    let names = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for name in names {
        if name? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn migrate_v7(conn: &Connection) -> Result<(), SaveError> {
    // Each ADD is skipped if a previous attempt already applied it, so a crash
    // between the two statements cannot brick the next open.
    if !missions_has_column(conn, "expedition_seed")? {
        conn.execute(
            "ALTER TABLE missions ADD COLUMN expedition_seed INTEGER",
            [],
        )?;
    }
    if !missions_has_column(conn, "expedition_applied")? {
        conn.execute(
            "ALTER TABLE missions ADD COLUMN expedition_applied INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    Ok(())
}

fn migrate_v8(conn: &Connection) -> Result<(), SaveError> {
    for (column, sql) in [
        (
            "cargo_metal",
            "ALTER TABLE missions ADD COLUMN cargo_metal REAL NOT NULL DEFAULT 0",
        ),
        (
            "cargo_crystal",
            "ALTER TABLE missions ADD COLUMN cargo_crystal REAL NOT NULL DEFAULT 0",
        ),
        (
            "cargo_deuterium",
            "ALTER TABLE missions ADD COLUMN cargo_deuterium REAL NOT NULL DEFAULT 0",
        ),
        (
            "transport",
            "ALTER TABLE missions ADD COLUMN transport INTEGER NOT NULL DEFAULT 0",
        ),
    ] {
        if !missions_has_column(conn, column)? {
            conn.execute(sql, [])?;
        }
    }
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS routes (
            id INTEGER PRIMARY KEY,
            origin_g INTEGER NOT NULL,
            origin_s INTEGER NOT NULL,
            origin_p INTEGER NOT NULL,
            dest_g INTEGER NOT NULL,
            dest_s INTEGER NOT NULL,
            dest_p INTEGER NOT NULL,
            cargo_metal REAL NOT NULL,
            cargo_crystal REAL NOT NULL,
            cargo_deuterium REAL NOT NULL,
            speed_percent INTEGER NOT NULL,
            interval_secs INTEGER NOT NULL,
            next_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS route_ships (
            route_id INTEGER NOT NULL,
            kind TEXT NOT NULL,
            count INTEGER NOT NULL,
            PRIMARY KEY (route_id, kind)
        );
        ",
    )?;
    Ok(())
}

fn meta_has_column(conn: &Connection, column: &str) -> Result<bool, SaveError> {
    let mut stmt = conn.prepare("PRAGMA table_info(meta)")?;
    let names = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for name in names {
        if name? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn migrate_v9(conn: &Connection) -> Result<(), SaveError> {
    if !meta_has_column(conn, "deterministic")? {
        conn.execute(
            "ALTER TABLE meta ADD COLUMN deterministic INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    Ok(())
}

fn migrate_v10(conn: &Connection) -> Result<(), SaveError> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS wonder_job (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            wonder_id TEXT NOT NULL,
            stage INTEGER NOT NULL,
            metal REAL NOT NULL,
            crystal REAL NOT NULL,
            deuterium REAL NOT NULL,
            completes_at INTEGER
        );
        CREATE TABLE IF NOT EXISTS wonder_done (
            position INTEGER PRIMARY KEY,
            wonder_id TEXT NOT NULL
        );
        ",
    )?;
    Ok(())
}

fn migrate_v11(conn: &Connection) -> Result<(), SaveError> {
    if !meta_has_column(conn, "outposts_seeded")? {
        conn.execute(
            "ALTER TABLE meta ADD COLUMN outposts_seeded INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    if !colonies_has_column(conn, "rare")? {
        conn.execute("ALTER TABLE colonies ADD COLUMN rare TEXT", [])?;
    }
    for (column, sql) in [
        (
            "attack",
            "ALTER TABLE missions ADD COLUMN attack INTEGER NOT NULL DEFAULT 0",
        ),
        (
            "invade",
            "ALTER TABLE missions ADD COLUMN invade INTEGER NOT NULL DEFAULT 0",
        ),
    ] {
        if !missions_has_column(conn, column)? {
            conn.execute(sql, [])?;
        }
    }
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS outposts (
            planet_id INTEGER PRIMARY KEY,
            galaxy INTEGER NOT NULL,
            system INTEGER NOT NULL,
            position INTEGER NOT NULL,
            kind TEXT NOT NULL,
            size INTEGER NOT NULL,
            garrison INTEGER NOT NULL,
            rare TEXT,
            last_rebuild_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS outpost_ships (
            planet_id INTEGER NOT NULL,
            kind TEXT NOT NULL,
            count INTEGER NOT NULL,
            PRIMARY KEY (planet_id, kind)
        );
        ",
    )?;
    Ok(())
}

fn migrate_v12(conn: &Connection) -> Result<(), SaveError> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS prestige (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            currency INTEGER NOT NULL,
            lifetime INTEGER NOT NULL,
            run_index INTEGER NOT NULL,
            run_started_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS prestige_owned (
            bonus_id TEXT PRIMARY KEY,
            count INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS prestige_runs (
            position INTEGER PRIMARY KEY,
            run_index INTEGER NOT NULL,
            started_at INTEGER NOT NULL,
            ended_at INTEGER,
            score INTEGER NOT NULL,
            prestige_earned INTEGER NOT NULL,
            wonders INTEGER NOT NULL,
            colonies INTEGER NOT NULL,
            building_levels INTEGER NOT NULL,
            research_levels INTEGER NOT NULL
        );
        ",
    )?;
    Ok(())
}

fn migrate_v13(conn: &Connection) -> Result<(), SaveError> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS goods (
            planet_id INTEGER NOT NULL,
            good TEXT NOT NULL,
            amount REAL NOT NULL,
            PRIMARY KEY (planet_id, good)
        );
        ",
    )?;
    Ok(())
}

fn migrate_v14(conn: &Connection) -> Result<(), SaveError> {
    if !meta_has_column(conn, "generator_version")? {
        conn.execute(
            "ALTER TABLE meta ADD COLUMN generator_version INTEGER NOT NULL DEFAULT 1",
            [],
        )?;
    }
    Ok(())
}

fn migrate_v15(conn: &Connection) -> Result<(), SaveError> {
    if !meta_has_column(conn, "last_story_at")? {
        conn.execute(
            "ALTER TABLE meta ADD COLUMN last_story_at INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS story_pending (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            event_id TEXT NOT NULL,
            offered_at INTEGER NOT NULL,
            timeout_at INTEGER NOT NULL
        );
        ",
    )?;
    Ok(())
}

fn migrate_v16(conn: &Connection) -> Result<(), SaveError> {
    if !meta_has_column(conn, "last_market_at")? {
        conn.execute(
            "ALTER TABLE meta ADD COLUMN last_market_at INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS market_prices (
            kind TEXT PRIMARY KEY,
            price REAL NOT NULL
        );
        CREATE TABLE IF NOT EXISTS market_history (
            kind TEXT NOT NULL,
            position INTEGER NOT NULL,
            price REAL NOT NULL,
            PRIMARY KEY (kind, position)
        );
        ",
    )?;
    Ok(())
}

fn migrate_v17(conn: &Connection) -> Result<(), SaveError> {
    if !meta_has_column(conn, "last_diplomacy_at")? {
        conn.execute(
            "ALTER TABLE meta ADD COLUMN last_diplomacy_at INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS faction_standing (
            id TEXT PRIMARY KEY,
            standing REAL NOT NULL
        );
        CREATE TABLE IF NOT EXISTS faction_log (
            position INTEGER PRIMARY KEY,
            at INTEGER NOT NULL,
            faction TEXT NOT NULL,
            delta REAL NOT NULL,
            reason TEXT NOT NULL
        );
        ",
    )?;
    Ok(())
}

fn migrate_v18(conn: &Connection) -> Result<(), SaveError> {
    if !missions_has_column(conn, "spy")? {
        conn.execute(
            "ALTER TABLE missions ADD COLUMN spy INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS spy_reports (
            id INTEGER PRIMARY KEY,
            galaxy INTEGER NOT NULL,
            system INTEGER NOT NULL,
            position INTEGER NOT NULL,
            created_at INTEGER NOT NULL,
            probes_sent INTEGER NOT NULL,
            probes_lost INTEGER NOT NULL,
            detected INTEGER NOT NULL,
            score REAL NOT NULL,
            tier TEXT NOT NULL,
            metal REAL NOT NULL DEFAULT 0,
            crystal REAL NOT NULL DEFAULT 0,
            deuterium REAL NOT NULL DEFAULT 0,
            garrison INTEGER NOT NULL DEFAULT 0,
            kind TEXT,
            size INTEGER,
            revealed_resources INTEGER NOT NULL DEFAULT 0,
            revealed_fleet INTEGER NOT NULL DEFAULT 0,
            revealed_defences INTEGER NOT NULL DEFAULT 0,
            revealed_garrison INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS spy_report_ships (
            report_id INTEGER NOT NULL,
            role TEXT NOT NULL,
            kind TEXT NOT NULL,
            count INTEGER NOT NULL,
            PRIMARY KEY (report_id, role, kind)
        );
        ",
    )?;
    Ok(())
}

fn migrate_v19(conn: &Connection) -> Result<(), SaveError> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS planet_defences (
            planet_id INTEGER NOT NULL,
            kind TEXT NOT NULL,
            count INTEGER NOT NULL,
            PRIMARY KEY (planet_id, kind)
        );
        ",
    )?;
    Ok(())
}

fn migrate_v20(conn: &Connection) -> Result<(), SaveError> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS laws (
            id INTEGER PRIMARY KEY,
            template_id TEXT NOT NULL,
            param REAL NOT NULL,
            previous_param REAL,
            owner TEXT NOT NULL,
            jurisdiction TEXT NOT NULL,
            proposed_at INTEGER NOT NULL,
            effective_at INTEGER NOT NULL,
            phase_in_secs INTEGER NOT NULL,
            status TEXT NOT NULL,
            repeal_effective_at INTEGER,
            enacted_at INTEGER,
            repealed_at INTEGER,
            baseline_output REAL NOT NULL DEFAULT 0,
            baseline_pollution REAL NOT NULL DEFAULT 0,
            baseline_research REAL NOT NULL DEFAULT 0,
            last_sample_at INTEGER NOT NULL DEFAULT 0,
            revenue_accrued REAL NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS law_approvals (
            law_id INTEGER NOT NULL,
            who TEXT NOT NULL,
            at INTEGER NOT NULL,
            PRIMARY KEY (law_id, who)
        );
        CREATE TABLE IF NOT EXISTS law_actions (
            position INTEGER PRIMARY KEY,
            at INTEGER NOT NULL,
            kind TEXT NOT NULL,
            law_id INTEGER NOT NULL,
            actor TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS law_samples (
            law_id INTEGER NOT NULL,
            at INTEGER NOT NULL,
            metric TEXT NOT NULL,
            expected REAL NOT NULL,
            actual REAL NOT NULL,
            PRIMARY KEY (law_id, at, metric)
        );
        ",
    )?;
    Ok(())
}

fn migrate_v21(conn: &Connection) -> Result<(), SaveError> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS exclusive_picks (
            group_id TEXT PRIMARY KEY,
            tech_id TEXT NOT NULL
        );
        ",
    )?;
    Ok(())
}

fn migrate_v22(conn: &Connection) -> Result<(), SaveError> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS achievements (
            id TEXT PRIMARY KEY,
            unlocked_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS achievement_stats (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            raids_survived INTEGER NOT NULL DEFAULT 0,
            outposts_conquered INTEGER NOT NULL DEFAULT 0
        );
        ",
    )?;
    Ok(())
}

fn migrate_v23(conn: &Connection) -> Result<(), SaveError> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS combat_logs (
            id INTEGER PRIMARY KEY,
            at INTEGER NOT NULL,
            seed INTEGER NOT NULL,
            winner TEXT NOT NULL,
            rounds INTEGER NOT NULL,
            layered INTEGER NOT NULL DEFAULT 0,
            ground_atk INTEGER,
            ground_def INTEGER,
            ground_atk_roll INTEGER,
            ground_def_roll INTEGER,
            ground_winner TEXT
        );
        CREATE TABLE IF NOT EXISTS combat_log_frames (
            log_id INTEGER NOT NULL,
            frame INTEGER NOT NULL,
            round INTEGER NOT NULL,
            shots INTEGER NOT NULL,
            explosions INTEGER NOT NULL,
            PRIMARY KEY (log_id, frame)
        );
        CREATE TABLE IF NOT EXISTS combat_log_ships (
            log_id INTEGER NOT NULL,
            frame INTEGER NOT NULL,
            side TEXT NOT NULL,
            kind TEXT NOT NULL,
            count INTEGER NOT NULL,
            shots INTEGER NOT NULL DEFAULT 0,
            explosions INTEGER NOT NULL DEFAULT 0
        );
        ",
    )?;
    Ok(())
}

fn migrate_v24(conn: &Connection) -> Result<(), SaveError> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS daily_meta (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            day_index INTEGER,
            streak INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS daily_slots (
            ord INTEGER PRIMARY KEY,
            spec_id TEXT NOT NULL,
            progress INTEGER NOT NULL,
            target INTEGER NOT NULL,
            rewarded INTEGER NOT NULL
        );
        ",
    )?;
    Ok(())
}

fn insert_daily(tx: &rusqlite::Transaction<'_>, state: &GameState) -> Result<(), SaveError> {
    tx.execute(
        "INSERT INTO daily_meta (id, day_index, streak) VALUES (1, ?1, ?2)",
        params![state.daily.day_index.map(|d| d as i64), state.daily.streak,],
    )?;
    for (ord, slot) in state.daily.slots.iter().enumerate() {
        tx.execute(
            "INSERT INTO daily_slots (ord, spec_id, progress, target, rewarded)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                ord as i64,
                slot.spec_id,
                slot.progress,
                slot.target,
                i64::from(slot.rewarded),
            ],
        )?;
    }
    Ok(())
}

fn load_daily(conn: &Connection, state: &mut GameState) -> Result<(), SaveError> {
    if !table_exists(conn, "daily_meta") {
        state.daily = crate::DailyBoard::default();
        state.sync_daily(state.last_tick_unix);
        return Ok(());
    }
    state.daily = crate::DailyBoard::default();
    if let Some((day, streak)) = conn
        .query_row(
            "SELECT day_index, streak FROM daily_meta WHERE id = 1",
            [],
            |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?
    {
        state.daily.day_index = unix_ts_opt(day)?;
        state.daily.streak = u32_field(streak, "daily streak")?;
    }
    if table_exists(conn, "daily_slots") {
        let mut stmt = conn
            .prepare("SELECT spec_id, progress, target, rewarded FROM daily_slots ORDER BY ord")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?;
        for row in rows {
            let (spec_id, progress, target, rewarded) = row?;
            state.daily.slots.push(crate::DailySlot {
                spec_id,
                progress: u32_field(progress, "daily progress")?,
                target: u32_field(target, "daily target")?,
                rewarded: rewarded != 0,
            });
        }
    }
    if state.daily.slots.is_empty() {
        state.sync_daily(state.last_tick_unix);
    }
    Ok(())
}

fn load_achievements(conn: &Connection, state: &mut GameState) -> Result<(), SaveError> {
    let mut stmt = conn.prepare("SELECT id, unlocked_at FROM achievements")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    state.achievements.clear();
    for row in rows {
        let (id, unlocked_at) = row?;
        state.achievements.insert(id, unix_ts(unlocked_at)?);
    }
    if let Some((raids, conquests)) = conn
        .query_row(
            "SELECT raids_survived, outposts_conquered FROM achievement_stats WHERE id = 1",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?
    {
        state.raids_survived = u32_field(raids, "raids survived")?;
        state.outposts_conquered = u32_field(conquests, "outposts conquered")?;
    }
    Ok(())
}

fn insert_combat_logs(tx: &rusqlite::Transaction<'_>, state: &GameState) -> Result<(), SaveError> {
    for (id, log) in state.combat_logs.iter().enumerate() {
        let log_id = (id as i64) + 1;
        let (g_atk, g_def, g_atk_roll, g_def_roll, g_winner) = match &log.report.ground {
            Some(g) => (
                Some(i64::from(g.attacker_strength)),
                Some(i64::from(g.defender_strength)),
                Some(i64::from(g.attacker_roll)),
                Some(i64::from(g.defender_roll)),
                Some(g.winner.as_key()),
            ),
            None => (None, None, None, None, None),
        };
        tx.execute(
            "INSERT INTO combat_logs (id, at, seed, winner, rounds, layered,
                ground_atk, ground_def, ground_atk_roll, ground_def_roll, ground_winner)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                log_id,
                log.at as i64,
                log.report.seed as i64,
                log.report.winner.as_key(),
                i64::from(log.report.rounds),
                i64::from(log.report.layered),
                g_atk,
                g_def,
                g_atk_roll,
                g_def_roll,
                g_winner,
            ],
        )?;
        for (frame_i, frame) in log.report.frames.iter().enumerate() {
            tx.execute(
                "INSERT INTO combat_log_frames (log_id, frame, round, shots, explosions)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    log_id,
                    frame_i as i64,
                    i64::from(frame.round),
                    frame.shots,
                    frame.explosions,
                ],
            )?;
            insert_combat_frame(tx, log_id, frame_i as i64, "a", frame.attacker, frame)?;
            insert_combat_frame(tx, log_id, frame_i as i64, "d", frame.defender, frame)?;
        }
    }
    Ok(())
}

fn insert_combat_frame(
    tx: &rusqlite::Transaction<'_>,
    log_id: i64,
    frame: i64,
    side: &str,
    fleet: Fleet,
    snap: &CombatFrame,
) -> Result<(), SaveError> {
    for kind in ShipKind::ALL {
        let count = fleet.get(kind);
        if count == 0 {
            continue;
        }
        tx.execute(
            "INSERT INTO combat_log_ships (log_id, frame, side, kind, count, shots, explosions)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                log_id,
                frame,
                side,
                kind.as_key(),
                count,
                snap.shots,
                snap.explosions,
            ],
        )?;
    }
    Ok(())
}

fn combat_winner_from_key(key: &str) -> Result<CombatWinner, SaveError> {
    match key {
        "attacker" => Ok(CombatWinner::Attacker),
        "defender" => Ok(CombatWinner::Defender),
        "draw" => Ok(CombatWinner::Draw),
        _ => Err(SaveError::Corrupt("unknown combat winner")),
    }
}

fn load_combat_logs(conn: &Connection, state: &mut GameState) -> Result<(), SaveError> {
    let mut stmt = conn.prepare(
        "SELECT id, at, seed, winner, rounds, layered,
                ground_atk, ground_def, ground_atk_roll, ground_def_roll, ground_winner
         FROM combat_logs ORDER BY id",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, Option<i64>>(6)?,
                row.get::<_, Option<i64>>(7)?,
                row.get::<_, Option<i64>>(8)?,
                row.get::<_, Option<i64>>(9)?,
                row.get::<_, Option<String>>(10)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    state.combat_logs.clear();
    for (id, at, seed, winner, rounds, layered, g_atk, g_def, g_atk_roll, g_def_roll, g_winner) in
        rows
    {
        let winner = combat_winner_from_key(&winner)?;
        let ground = match g_winner {
            Some(key) => Some(GroundReport {
                attacker_strength: u32_field(g_atk.unwrap_or(0), "ground attacker")?,
                defender_strength: u32_field(g_def.unwrap_or(0), "ground defender")?,
                attacker_roll: u32_field(g_atk_roll.unwrap_or(0), "ground attacker roll")?,
                defender_roll: u32_field(g_def_roll.unwrap_or(0), "ground defender roll")?,
                winner: combat_winner_from_key(&key)?,
            }),
            None => None,
        };
        let frames = load_combat_frames(conn, id)?;
        let (attacker_before, defender_before) = frames
            .first()
            .map(|f| (f.attacker, f.defender))
            .unwrap_or_default();
        let (attacker_after, defender_after) = frames
            .last()
            .map(|f| (f.attacker, f.defender))
            .unwrap_or_default();
        state.combat_logs.push(CombatLog {
            at: unix_ts(at)?,
            report: CombatReport {
                rounds: u8::try_from(rounds).map_err(|_| SaveError::Corrupt("combat rounds"))?,
                attacker_before,
                defender_before,
                attacker_after,
                defender_after,
                winner,
                ground,
                seed: seed as u64,
                frames,
                layered: layered != 0,
            },
        });
    }
    Ok(())
}

fn load_combat_frames(conn: &Connection, log_id: i64) -> Result<Vec<CombatFrame>, SaveError> {
    let mut stmt = conn.prepare(
        "SELECT frame, round, shots, explosions FROM combat_log_frames
         WHERE log_id = ?1 ORDER BY frame",
    )?;
    let header_rows = stmt.query_map([log_id], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
        ))
    })?;
    let mut by_frame: BTreeMap<i64, CombatFrame> = BTreeMap::new();
    for row in header_rows {
        let (frame, round, shots, explosions) = row?;
        by_frame.insert(
            frame,
            CombatFrame {
                round: u8::try_from(round).map_err(|_| SaveError::Corrupt("combat round"))?,
                attacker: Fleet::default(),
                defender: Fleet::default(),
                shots: u32_field(shots, "combat shots")?,
                explosions: u32_field(explosions, "combat explosions")?,
            },
        );
    }
    drop(stmt);
    let mut stmt = conn.prepare(
        "SELECT frame, side, kind, count FROM combat_log_ships
         WHERE log_id = ?1 ORDER BY frame, side, kind",
    )?;
    let rows = stmt.query_map([log_id], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
        ))
    })?;
    for row in rows {
        let (frame, side, kind, count) = row?;
        let kind = ShipKind::from_key(&kind).ok_or(SaveError::Corrupt("unknown combat ship"))?;
        let count = u32_field(count, "combat ship count")?;
        let entry = by_frame
            .get_mut(&frame)
            .ok_or(SaveError::Corrupt("combat ship without frame"))?;
        match side.as_str() {
            "a" => entry.attacker.add(kind, count),
            "d" => entry.defender.add(kind, count),
            _ => return Err(SaveError::Corrupt("unknown combat side")),
        }
    }
    Ok(by_frame.into_values().collect())
}

fn load_spy_reports(conn: &Connection, state: &mut GameState) -> Result<(), SaveError> {
    let mut stmt = conn.prepare(
        "SELECT id, galaxy, system, position, created_at, probes_sent, probes_lost,
                detected, score, tier, metal, crystal, deuterium, garrison, kind, size,
                revealed_resources, revealed_fleet, revealed_defences, revealed_garrison
         FROM spy_reports ORDER BY id",
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
            row.get::<_, f64>(8)?,
            row.get::<_, String>(9)?,
            row.get::<_, f64>(10)?,
            row.get::<_, f64>(11)?,
            row.get::<_, f64>(12)?,
            row.get::<_, i64>(13)?,
            row.get::<_, Option<String>>(14)?,
            row.get::<_, Option<i64>>(15)?,
            row.get::<_, i64>(16)?,
            row.get::<_, i64>(17)?,
            row.get::<_, i64>(18)?,
            row.get::<_, i64>(19)?,
        ))
    })?;
    let mut pending = Vec::new();
    for row in rows {
        pending.push(row?);
    }
    drop(stmt);
    state.spy_reports.clear();
    for (
        id,
        g,
        s,
        p,
        created_at,
        probes_sent,
        probes_lost,
        detected,
        score,
        tier,
        metal,
        crystal,
        deuterium,
        garrison,
        kind,
        size,
        revealed_resources,
        revealed_fleet,
        revealed_defences,
        revealed_garrison,
    ) in pending
    {
        if !score.is_finite() {
            return Err(SaveError::Corrupt("spy score"));
        }
        let target = Coord {
            galaxy: u16_coord(g)?,
            system: u16_coord(s)?,
            position: u16_coord(p)?,
        };
        let kind = match kind {
            Some(key) => {
                Some(PlanetType::from_key(&key).ok_or(SaveError::Corrupt("spy planet type"))?)
            }
            None => None,
        };
        let size = match size {
            Some(v) => Some(u8::try_from(v).map_err(|_| SaveError::Corrupt("spy size"))?),
            None => None,
        };
        let fleet = if revealed_fleet != 0 {
            Some(read_spy_ships(conn, id, "fleet")?)
        } else {
            None
        };
        let defences = if revealed_defences != 0 {
            Some(read_spy_ships(conn, id, "defences")?)
        } else {
            None
        };
        state.spy_reports.push(crate::SpyReport {
            id: unix_ts(id)?,
            target,
            created_at: unix_ts(created_at)?,
            probes_sent: u32::try_from(probes_sent)
                .map_err(|_| SaveError::Corrupt("probes sent"))?,
            probes_lost: u32::try_from(probes_lost)
                .map_err(|_| SaveError::Corrupt("probes lost"))?,
            detected: detected != 0,
            score,
            tier,
            resources: (revealed_resources != 0).then_some(Resources {
                metal,
                crystal,
                deuterium,
            }),
            fleet,
            defences,
            garrison: if revealed_garrison != 0 {
                Some(u32::try_from(garrison).map_err(|_| SaveError::Corrupt("spy garrison"))?)
            } else {
                None
            },
            kind,
            size,
        });
    }
    state.next_report_id = state
        .spy_reports
        .iter()
        .map(|r| r.id)
        .max()
        .map(|id| id.saturating_add(1))
        .unwrap_or(0);
    Ok(())
}

fn read_spy_ships(conn: &Connection, report_id: i64, role: &str) -> Result<Fleet, SaveError> {
    let mut stmt = conn
        .prepare("SELECT kind, count FROM spy_report_ships WHERE report_id = ?1 AND role = ?2")?;
    let rows = stmt.query_map(params![report_id, role], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
    })?;
    let mut fleet = Fleet::default();
    for row in rows {
        let (kind, count) = row?;
        let kind = ShipKind::from_key(&kind).ok_or(SaveError::Corrupt("unknown ship kind"))?;
        fleet.add(kind, count);
    }
    Ok(fleet)
}

fn load_diplomacy(conn: &Connection, state: &mut GameState) -> Result<(), SaveError> {
    let mut stmt = conn.prepare("SELECT id, standing FROM faction_standing")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
    })?;
    for row in rows {
        let (id, standing) = row?;
        if !standing.is_finite() {
            return Err(SaveError::Corrupt("faction standing"));
        }
        state.diplomacy.set_standing(&id, standing);
    }
    let mut stmt =
        conn.prepare("SELECT at, faction, delta, reason FROM faction_log ORDER BY position")?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, f64>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    state.diplomacy.log.clear();
    for row in rows {
        let (at, faction, delta, reason) = row?;
        if !delta.is_finite() {
            return Err(SaveError::Corrupt("faction log delta"));
        }
        state.diplomacy.log.push(crate::StandingChange {
            at: unix_ts(at)?,
            faction,
            delta,
            reason,
        });
    }
    Ok(())
}

fn insert_laws(tx: &rusqlite::Transaction<'_>, state: &GameState) -> Result<(), SaveError> {
    for law in &state.laws.laws {
        tx.execute(
            "INSERT INTO laws (
                id, template_id, param, previous_param, owner, jurisdiction,
                proposed_at, effective_at, phase_in_secs, status, repeal_effective_at,
                enacted_at, repealed_at, baseline_output, baseline_pollution,
                baseline_research, last_sample_at, revenue_accrued
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
            params![
                law.id as i64,
                law.template_id,
                law.param,
                law.previous_param,
                law.owner,
                law.jurisdiction.as_key(),
                law.proposed_at as i64,
                law.effective_at as i64,
                law.phase_in_secs as i64,
                law.status.as_key(),
                law.repeal_effective_at.map(|t| t as i64),
                law.enacted_at.map(|t| t as i64),
                law.repealed_at.map(|t| t as i64),
                law.baseline_output,
                law.baseline_pollution,
                law.baseline_research,
                law.last_sample_at as i64,
                law.revenue_accrued,
            ],
        )?;
        for approval in &law.approvals {
            tx.execute(
                "INSERT INTO law_approvals (law_id, who, at) VALUES (?1, ?2, ?3)",
                params![law.id as i64, approval.who, approval.at as i64],
            )?;
        }
        for sample in &law.samples {
            tx.execute(
                "INSERT INTO law_samples (law_id, at, metric, expected, actual)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    law.id as i64,
                    sample.at as i64,
                    sample.metric.as_key(),
                    sample.expected,
                    sample.actual,
                ],
            )?;
        }
    }
    for (i, action) in state.laws.actions.iter().enumerate() {
        tx.execute(
            "INSERT INTO law_actions (position, at, kind, law_id, actor)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                i as i64,
                action.at as i64,
                action.kind.as_key(),
                action.law_id as i64,
                action.actor,
            ],
        )?;
    }
    Ok(())
}

fn load_laws(conn: &Connection, state: &mut GameState) -> Result<(), SaveError> {
    let mut stmt = conn.prepare(
        "SELECT id, template_id, param, previous_param, owner, jurisdiction,
                proposed_at, effective_at, phase_in_secs, status, repeal_effective_at,
                enacted_at, repealed_at, baseline_output, baseline_pollution,
                baseline_research, last_sample_at, revenue_accrued
         FROM laws ORDER BY id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, f64>(2)?,
            row.get::<_, Option<f64>>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, i64>(6)?,
            row.get::<_, i64>(7)?,
            row.get::<_, i64>(8)?,
            row.get::<_, String>(9)?,
            row.get::<_, Option<i64>>(10)?,
            row.get::<_, Option<i64>>(11)?,
            row.get::<_, Option<i64>>(12)?,
            row.get::<_, f64>(13)?,
            row.get::<_, f64>(14)?,
            row.get::<_, f64>(15)?,
            row.get::<_, i64>(16)?,
            row.get::<_, f64>(17)?,
        ))
    })?;
    let mut pending = Vec::new();
    for row in rows {
        pending.push(row?);
    }
    drop(stmt);
    state.laws = crate::LawState::default();
    for (
        id,
        template_id,
        param,
        previous_param,
        owner,
        jurisdiction,
        proposed_at,
        effective_at,
        phase_in_secs,
        status,
        repeal_effective_at,
        enacted_at,
        repealed_at,
        baseline_output,
        baseline_pollution,
        baseline_research,
        last_sample_at,
        revenue_accrued,
    ) in pending
    {
        if !param.is_finite() {
            return Err(SaveError::Corrupt("law param"));
        }
        let jurisdiction = crate::Jurisdiction::parse(&jurisdiction)
            .ok_or(SaveError::Corrupt("law jurisdiction"))?;
        let status = crate::LawStatus::from_key(&status).ok_or(SaveError::Corrupt("law status"))?;
        let mut approvals = Vec::new();
        let mut stmt =
            conn.prepare("SELECT who, at FROM law_approvals WHERE law_id = ?1 ORDER BY at")?;
        let rows = stmt.query_map(params![id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        for row in rows {
            let (who, at) = row?;
            approvals.push(crate::LawApproval {
                who,
                at: unix_ts(at)?,
            });
        }
        drop(stmt);
        let mut samples = Vec::new();
        let mut stmt = conn.prepare(
            "SELECT at, metric, expected, actual FROM law_samples WHERE law_id = ?1 ORDER BY at",
        )?;
        let rows = stmt.query_map(params![id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, f64>(2)?,
                row.get::<_, f64>(3)?,
            ))
        })?;
        for row in rows {
            let (at, metric, expected, actual) = row?;
            let metric =
                crate::LawMetric::from_key(&metric).ok_or(SaveError::Corrupt("law metric"))?;
            samples.push(crate::LawSample {
                at: unix_ts(at)?,
                metric,
                expected,
                actual,
            });
        }
        drop(stmt);
        state.laws.laws.push(crate::Law {
            id: unix_ts(id)?,
            template_id,
            param,
            previous_param,
            owner,
            jurisdiction,
            proposed_at: unix_ts(proposed_at)?,
            effective_at: unix_ts(effective_at)?,
            phase_in_secs: unix_ts(phase_in_secs)?,
            status,
            repeal_effective_at: match repeal_effective_at {
                Some(t) => Some(unix_ts(t)?),
                None => None,
            },
            enacted_at: match enacted_at {
                Some(t) => Some(unix_ts(t)?),
                None => None,
            },
            repealed_at: match repealed_at {
                Some(t) => Some(unix_ts(t)?),
                None => None,
            },
            approvals,
            baseline_output,
            baseline_pollution,
            baseline_research,
            last_sample_at: unix_ts(last_sample_at)?,
            revenue_accrued,
            samples,
        });
    }
    let mut stmt =
        conn.prepare("SELECT at, kind, law_id, actor FROM law_actions ORDER BY position")?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    state.laws.actions.clear();
    for row in rows {
        let (at, kind, law_id, actor) = row?;
        let kind = crate::LawActionKind::from_key(&kind).ok_or(SaveError::Corrupt("law action"))?;
        state.laws.actions.push(crate::LawAction {
            at: unix_ts(at)?,
            kind,
            law_id: unix_ts(law_id)?,
            actor,
        });
    }
    state.laws.next_id = state
        .laws
        .laws
        .iter()
        .map(|l| l.id)
        .max()
        .map(|id| id.saturating_add(1))
        .unwrap_or(0);
    Ok(())
}

fn load_market(conn: &Connection, state: &mut GameState) -> Result<(), SaveError> {
    let mut stmt = conn.prepare("SELECT kind, price FROM market_prices")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
    })?;
    for row in rows {
        let (kind, price) = row?;
        let kind = crate::ResourceKind::from_key(&kind)
            .ok_or(SaveError::Corrupt("unknown market resource"))?;
        if !price.is_finite() || price <= 0.0 {
            return Err(SaveError::Corrupt("market price"));
        }
        state.market.set_price(kind, price);
    }
    let mut metal = Vec::new();
    let mut crystal = Vec::new();
    let mut deuterium = Vec::new();
    let mut stmt =
        conn.prepare("SELECT kind, position, price FROM market_history ORDER BY kind, position")?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, f64>(2)?,
        ))
    })?;
    for row in rows {
        let (kind, _position, price) = row?;
        let kind = crate::ResourceKind::from_key(&kind)
            .ok_or(SaveError::Corrupt("unknown market history resource"))?;
        if !price.is_finite() || price <= 0.0 {
            return Err(SaveError::Corrupt("market history price"));
        }
        match kind {
            crate::ResourceKind::Metal => metal.push(price),
            crate::ResourceKind::Crystal => crystal.push(price),
            crate::ResourceKind::Deuterium => deuterium.push(price),
        }
    }
    if !metal.is_empty() {
        state.market.metal_hist = metal;
    }
    if !crystal.is_empty() {
        state.market.crystal_hist = crystal;
    }
    if !deuterium.is_empty() {
        state.market.deuterium_hist = deuterium;
    }
    Ok(())
}

fn read_prestige(conn: &Connection, fallback_started: u64) -> Result<Prestige, SaveError> {
    let row = conn
        .query_row(
            "SELECT currency, lifetime, run_index, run_started_at FROM prestige WHERE id = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .optional()?;
    let Some((currency, lifetime, run_index, started_at)) = row else {
        return Ok(Prestige::new(fallback_started));
    };
    let mut prestige = Prestige {
        currency: u32_field(currency, "prestige currency")?,
        lifetime: u32_field(lifetime, "prestige lifetime")?,
        owned: std::collections::BTreeMap::new(),
        runs: Vec::new(),
        current: RunStats::new(u32_field(run_index, "prestige run")?, unix_ts(started_at)?),
    };
    let mut stmt = conn.prepare("SELECT bonus_id, count FROM prestige_owned")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    for row in rows {
        let (id, count) = row?;
        prestige
            .owned
            .insert(id, u32_field(count, "prestige owned")?);
    }
    let mut stmt = conn.prepare(
        "SELECT run_index, started_at, ended_at, score, prestige_earned,
                wonders, colonies, building_levels, research_levels
         FROM prestige_runs ORDER BY position",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, Option<i64>>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, i64>(5)?,
            row.get::<_, i64>(6)?,
            row.get::<_, i64>(7)?,
            row.get::<_, i64>(8)?,
        ))
    })?;
    for row in rows {
        let (index, started, ended, score, earned, wonders, colonies, buildings, research) = row?;
        prestige.runs.push(RunStats {
            index: u32_field(index, "run index")?,
            started_at: unix_ts(started)?,
            ended_at: unix_ts_opt(ended)?,
            score: u32_field(score, "run score")?,
            prestige_earned: u32_field(earned, "run earned")?,
            wonders: u32_field(wonders, "run wonders")?,
            colonies: u32_field(colonies, "run colonies")?,
            building_levels: u32_field(buildings, "run buildings")?,
            research_levels: u32_field(research, "run research")?,
        });
    }
    Ok(prestige)
}

fn u32_field(value: i64, label: &'static str) -> Result<u32, SaveError> {
    u32::try_from(value).map_err(|_| SaveError::Corrupt(label))
}

fn colonies_has_column(conn: &Connection, column: &str) -> Result<bool, SaveError> {
    let mut stmt = conn.prepare("PRAGMA table_info(colonies)")?;
    let names = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for name in names {
        if name? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn read_build_queue(conn: &Connection, planet_id: i64) -> Result<Vec<BuildJob>, SaveError> {
    let mut stmt = conn.prepare(
        "SELECT kind, from_level, paid_metal, paid_crystal, paid_deuterium, started_at, completes_at
         FROM build_queue WHERE planet_id = ?1 ORDER BY position",
    )?;
    let jobs = stmt.query_map(params![planet_id], |row| {
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
    let mut queue = Vec::new();
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
    Ok(queue)
}

fn read_routes(conn: &Connection) -> Result<Vec<TradeRoute>, SaveError> {
    let mut stmt = conn.prepare(
        "SELECT id, origin_g, origin_s, origin_p, dest_g, dest_s, dest_p,
                cargo_metal, cargo_crystal, cargo_deuterium,
                speed_percent, interval_secs, next_at
         FROM routes ORDER BY id",
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
            row.get::<_, f64>(7)?,
            row.get::<_, f64>(8)?,
            row.get::<_, f64>(9)?,
            row.get::<_, i64>(10)?,
            row.get::<_, i64>(11)?,
            row.get::<_, i64>(12)?,
        ))
    })?;
    let mut pending = Vec::new();
    for row in rows {
        pending.push(row?);
    }
    drop(stmt);
    let mut routes = Vec::new();
    for (
        id,
        og,
        os,
        op,
        dg,
        ds,
        dp,
        cargo_metal,
        cargo_crystal,
        cargo_deuterium,
        speed,
        interval_secs,
        next_at,
    ) in pending
    {
        let mut ships = Fleet::default();
        let mut ship_stmt =
            conn.prepare("SELECT kind, count FROM route_ships WHERE route_id = ?1")?;
        let ship_rows = ship_stmt.query_map(params![id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
        })?;
        for ship_row in ship_rows {
            let (kind, count) = ship_row?;
            let kind = ShipKind::from_key(&kind).ok_or(SaveError::Corrupt("unknown ship kind"))?;
            ships.add(kind, count);
        }
        routes.push(TradeRoute {
            id: unix_ts(id)?,
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
            ships,
            cargo: Resources {
                metal: cargo_metal,
                crystal: cargo_crystal,
                deuterium: cargo_deuterium,
            },
            speed_percent: u8::try_from(speed).map_err(|_| SaveError::Corrupt("speed percent"))?,
            interval_secs: unix_ts(interval_secs)?,
            next_at: unix_ts(next_at)?,
        });
    }
    Ok(routes)
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
        let (kind, size) = crate::planet_profile(11, dest, crate::GALAXY_GENERATOR_VERSION);
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
            defences: crate::DefenceGrid::default(),
            rare: None,
            goods: BTreeMap::new(),
            build_queue: vec![BuildJob {
                building: BuildingKind::SolarPlant,
                from_level: 0,
                paid: Resources {
                    metal: 1.0,
                    crystal: 1.0,
                    deuterium: 0.0,
                },
                started_at: Some(1_700_000_000),
                completes_at: Some(1_700_000_040),
            }],
        });
        let mut route_ships = crate::Fleet::default();
        route_ships.add(crate::ShipKind::SmallCargo, 2);
        state.routes.push(crate::TradeRoute {
            id: 0,
            origin: crate::Coord::HOME,
            dest,
            ships: route_ships,
            cargo: Resources {
                metal: 10.0,
                crystal: 0.0,
                deuterium: 0.0,
            },
            speed_percent: 100,
            interval_secs: 60,
            next_at: 1_700_000_000,
        });
        state.next_route_id = 1;
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
            mission.transport = true;
            mission.cargo = Resources {
                metal: 3.0,
                crystal: 2.0,
                deuterium: 1.0,
            };
        }
        state.completed_wonders.push("observatory".into());
        state.wonder = Some(crate::WonderJob {
            id: "dyson_ring".into(),
            stage: 0,
            contributed: Resources {
                metal: 100.0,
                crystal: 50.0,
                deuterium: 10.0,
            },
            completes_at: Some(1_700_000_100),
        });
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
    fn activity_tracker_round_trips() {
        let mut original = GameState::new(10);
        original.activity.prod_boost_until = Some(100);
        original.activity.prod_boost_factor = 0.25;
        original.activity.research_boost_until = Some(80);
        original.activity.research_boost_factor = 0.5;
        original.activity.last_applied[0] = Some(10);
        original.activity.day_start = 0;
        original.activity.day_count[0] = 2;
        let mut conn = open_save(Path::new(":memory:")).unwrap();
        save_state(&mut conn, &original).unwrap();
        let loaded = load_state(&conn, BalanceConfig::default()).unwrap();
        assert_eq!(loaded.activity, original.activity);
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
        assert!(loaded.goods.is_empty());
        assert_eq!(loaded.generator_version, 1);
        assert_eq!(loaded.last_story_at, 0);
        assert!(loaded.story_pending.is_none());
        assert_eq!(loaded.last_market_at, 0);
        assert_eq!(loaded.last_diplomacy_at, 0);
        assert_eq!(loaded.buildings.metal_mine, 4);
        assert_eq!(loaded.tech_level("weapons"), 3);
        assert_eq!(loaded.activity, crate::ActivityTracker::default());
        assert!(missions_has_column(&conn, "expedition_seed").unwrap());
        assert!(missions_has_column(&conn, "expedition_applied").unwrap());
        assert!(missions_has_column(&conn, "cargo_metal").unwrap());
        assert!(missions_has_column(&conn, "transport").unwrap());
        migrate_v7(&conn).unwrap();
        migrate_v8(&conn).unwrap();
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    }

    #[test]
    fn migrate_v7_retries_after_a_partial_alter() {
        let path = unique_path("v7-partial");
        let _ = std::fs::remove_file(&path);
        {
            let conn = Connection::open(&path).unwrap();
            migrate_v1(&conn).unwrap();
            migrate_v2(&conn).unwrap();
            migrate_v3(&conn).unwrap();
            migrate_v4(&conn).unwrap();
            migrate_v5(&conn).unwrap();
            migrate_v6(&conn).unwrap();
            conn.execute_batch("PRAGMA user_version = 6;").unwrap();
            conn.execute(
                "ALTER TABLE missions ADD COLUMN expedition_seed INTEGER",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO meta (id, last_tick_unix, rng_seed) VALUES (1, 5, 0)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO resources (planet_id, metal, crystal, deuterium) VALUES (1, 0, 0, 0)",
                [],
            )
            .unwrap();
        }
        let conn = open_save(&path).unwrap();
        let version: u32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        assert!(missions_has_column(&conn, "expedition_seed").unwrap());
        assert!(missions_has_column(&conn, "expedition_applied").unwrap());
        assert!(missions_has_column(&conn, "cargo_metal").unwrap());
        assert!(missions_has_column(&conn, "transport").unwrap());
        let loaded = load_state(&conn, BalanceConfig::default()).unwrap();
        assert_eq!(loaded.last_tick_unix, 5);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    }

    #[test]
    fn migrate_v8_retries_after_a_partial_alter() {
        let path = unique_path("v8-partial");
        let _ = std::fs::remove_file(&path);
        {
            let conn = Connection::open(&path).unwrap();
            migrate_v1(&conn).unwrap();
            migrate_v2(&conn).unwrap();
            migrate_v3(&conn).unwrap();
            migrate_v4(&conn).unwrap();
            migrate_v5(&conn).unwrap();
            migrate_v6(&conn).unwrap();
            migrate_v7(&conn).unwrap();
            conn.execute_batch("PRAGMA user_version = 7;").unwrap();
            conn.execute(
                "ALTER TABLE missions ADD COLUMN cargo_metal REAL NOT NULL DEFAULT 0",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO meta (id, last_tick_unix, rng_seed) VALUES (1, 8, 0)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO resources (planet_id, metal, crystal, deuterium) VALUES (1, 0, 0, 0)",
                [],
            )
            .unwrap();
        }
        let conn = open_save(&path).unwrap();
        let version: u32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        assert!(missions_has_column(&conn, "cargo_metal").unwrap());
        assert!(missions_has_column(&conn, "cargo_crystal").unwrap());
        assert!(missions_has_column(&conn, "cargo_deuterium").unwrap());
        assert!(missions_has_column(&conn, "transport").unwrap());
        let loaded = load_state(&conn, BalanceConfig::default()).unwrap();
        assert_eq!(loaded.last_tick_unix, 8);
        assert!(loaded.routes.is_empty());
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
    #[cfg(unix)]
    fn lock_follows_a_symlink_to_the_same_save() {
        let save = unique_path("lock-real");
        let alias = save.with_file_name(format!(
            "{}-alias.sqlite",
            save.file_stem().unwrap().to_string_lossy()
        ));
        std::fs::write(&save, []).unwrap();
        std::os::unix::fs::symlink(&save, &alias).unwrap();
        let holder = lock_save(&save).unwrap();
        let err = lock_save(&alias).unwrap_err();
        assert!(matches!(err, SaveError::Locked), "{err:?}");
        drop(holder);
        let _ = std::fs::remove_file(&alias);
        let _ = std::fs::remove_file(&save);
        let _ = std::fs::remove_file(save_lock_path(&save));
    }

    #[test]
    #[cfg(unix)]
    fn lock_follows_a_dangling_symlink() {
        let save = unique_path("lock-dangle");
        let alias = save.with_file_name(format!(
            "{}-dangle.sqlite",
            save.file_stem().unwrap().to_string_lossy()
        ));
        let _ = std::fs::remove_file(&save);
        std::os::unix::fs::symlink(&save, &alias).unwrap();
        let holder = lock_save(&alias).unwrap();
        let err = lock_save(&save).unwrap_err();
        assert!(matches!(err, SaveError::Locked), "{err:?}");
        drop(holder);
        let _ = std::fs::remove_file(&alias);
        let _ = std::fs::remove_file(save_lock_path(&save));
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
    fn seeded_save_round_trips_deterministic_flag() {
        let mut conn = open_save(Path::new(":memory:")).unwrap();
        let state = load_or_new_with_seed(&mut conn, 10, BalanceConfig::default(), 42).unwrap();
        assert!(state.deterministic);
        assert_eq!(state.rng_seed, 42);
        assert_eq!(state.galaxy_seed, 42);
        let loaded = load_state(&conn, BalanceConfig::default()).unwrap();
        assert_eq!(loaded, state);
        assert!(meta_has_column(&conn, "deterministic").unwrap());
    }

    #[test]
    fn migrate_v9_retries_after_a_partial_alter() {
        let mut conn = open_save(Path::new(":memory:")).unwrap();
        migrate_v9(&conn).unwrap();
        migrate_v9(&conn).unwrap();
        let mut state = GameState::new(3);
        state.deterministic = true;
        save_state(&mut conn, &state).unwrap();
        let loaded = load_state(&conn, BalanceConfig::default()).unwrap();
        assert!(loaded.deterministic);
    }

    #[test]
    fn migrate_v10_retries_and_round_trips_wonder_progress() {
        let mut conn = open_save(Path::new(":memory:")).unwrap();
        migrate_v10(&conn).unwrap();
        migrate_v10(&conn).unwrap();
        let mut state = GameState::new(3);
        state.completed_wonders.push("observatory".into());
        state.wonder = Some(crate::WonderJob {
            id: "dyson_ring".into(),
            stage: 1,
            contributed: Resources {
                metal: 5.0,
                crystal: 4.0,
                deuterium: 3.0,
            },
            completes_at: None,
        });
        save_state(&mut conn, &state).unwrap();
        let loaded = load_state(&conn, BalanceConfig::default()).unwrap();
        assert_eq!(loaded.wonder, state.wonder);
        assert_eq!(loaded.completed_wonders, state.completed_wonders);
        assert_eq!(loaded, state);
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

    #[test]
    fn migrate_v12_retries_and_round_trips_prestige() {
        let mut conn = open_save(Path::new(":memory:")).unwrap();
        migrate_v12(&conn).unwrap();
        migrate_v12(&conn).unwrap();
        let mut state = GameState::new(3);
        state.prestige.currency = 4;
        state.prestige.lifetime = 7;
        state.prestige.owned.insert("mine_focus".into(), 2);
        state.prestige.runs.push(crate::RunStats {
            index: 1,
            started_at: 1,
            ended_at: Some(2),
            score: 90,
            prestige_earned: 4,
            wonders: 1,
            colonies: 0,
            building_levels: 12,
            research_levels: 3,
        });
        state.prestige.current.index = 2;
        state.prestige.current.started_at = 3;
        save_state(&mut conn, &state).unwrap();
        let loaded = load_state(&conn, BalanceConfig::default()).unwrap();
        assert_eq!(loaded.prestige, state.prestige);
        assert_eq!(loaded, state);
    }

    #[test]
    fn migrate_v13_retries_and_round_trips_goods() {
        let mut conn = open_save(Path::new(":memory:")).unwrap();
        migrate_v13(&conn).unwrap();
        migrate_v13(&conn).unwrap();
        let mut state = GameState::new(3);
        state.goods.insert("iron_ore".into(), 12.5);
        state.goods.insert("steel".into(), 4.0);
        state.colonies.push(crate::Colony {
            coord: crate::Coord {
                galaxy: 1,
                system: 1,
                position: 2,
            },
            kind: crate::PlanetType::Ice,
            size: 80,
            resources: Resources::ZERO,
            buildings: crate::Buildings::default(),
            fleet: crate::Fleet::default(),
            defences: crate::DefenceGrid::default(),
            build_queue: Vec::new(),
            rare: None,
            goods: BTreeMap::from([("water_ice".into(), 7.0)]),
        });
        save_state(&mut conn, &state).unwrap();
        let loaded = load_state(&conn, BalanceConfig::default()).unwrap();
        assert_eq!(loaded.goods, state.goods);
        assert_eq!(loaded.colonies[0].goods, state.colonies[0].goods);
        assert_eq!(loaded.resources, state.resources);
        assert_eq!(loaded, state);
    }

    #[test]
    fn migrate_v14_retries_and_round_trips_generator_version() {
        let mut conn = open_save(Path::new(":memory:")).unwrap();
        migrate_v14(&conn).unwrap();
        migrate_v14(&conn).unwrap();
        let mut state = GameState::new(3);
        state.galaxy_seed = 99;
        state.generator_version = 1;
        save_state(&mut conn, &state).unwrap();
        let loaded = load_state(&conn, BalanceConfig::default()).unwrap();
        assert_eq!(loaded.galaxy_seed, 99);
        assert_eq!(loaded.generator_version, 1);
        assert_eq!(loaded, state);
        assert!(meta_has_column(&conn, "generator_version").unwrap());
    }

    #[test]
    fn migrate_v15_retries_and_round_trips_pending_story() {
        let mut conn = open_save(Path::new(":memory:")).unwrap();
        migrate_v15(&conn).unwrap();
        migrate_v15(&conn).unwrap();
        let mut state = GameState::new(3);
        state.last_story_at = 11;
        state.story_pending = Some(crate::PendingStory {
            event_id: "derelict".into(),
            offered_at: 5,
            timeout_at: 90,
        });
        save_state(&mut conn, &state).unwrap();
        let loaded = load_state(&conn, BalanceConfig::default()).unwrap();
        assert_eq!(loaded.last_story_at, 11);
        assert_eq!(loaded.story_pending, state.story_pending);
        assert_eq!(loaded, state);
        assert!(meta_has_column(&conn, "last_story_at").unwrap());
        assert_eq!(count_rows(&conn, "story_pending").unwrap(), 1);
    }

    #[test]
    fn migrate_v16_retries_and_round_trips_market_prices() {
        let mut conn = open_save(Path::new(":memory:")).unwrap();
        migrate_v16(&conn).unwrap();
        migrate_v16(&conn).unwrap();
        let mut state = GameState::new(3);
        state.last_market_at = 44;
        state.market.metal = 1.4;
        state.market.crystal = 1.7;
        state.market.deuterium = 3.6;
        state.market.metal_hist = vec![1.0, 1.2, 1.4];
        state.market.crystal_hist = vec![2.0, 1.8, 1.7];
        state.market.deuterium_hist = vec![3.0, 3.3, 3.6];
        save_state(&mut conn, &state).unwrap();
        let loaded = load_state(&conn, BalanceConfig::default()).unwrap();
        assert_eq!(loaded.last_market_at, 44);
        assert_eq!(loaded.market, state.market);
        assert_eq!(loaded, state);
        assert!(meta_has_column(&conn, "last_market_at").unwrap());
        assert_eq!(count_rows(&conn, "market_prices").unwrap(), 3);
        assert_eq!(count_rows(&conn, "market_history").unwrap(), 9);
    }

    #[test]
    fn migrate_v17_retries_and_round_trips_faction_standing() {
        let mut conn = open_save(Path::new(":memory:")).unwrap();
        migrate_v17(&conn).unwrap();
        migrate_v17(&conn).unwrap();
        let mut state = GameState::new(3);
        state.last_diplomacy_at = 77;
        state.diplomacy.set_standing("traders", 21.0);
        state.diplomacy.set_standing("pirates", -12.0);
        state.diplomacy.log.push(crate::StandingChange {
            at: 9,
            faction: "traders".into(),
            delta: 21.0,
            reason: "trade".into(),
        });
        save_state(&mut conn, &state).unwrap();
        let loaded = load_state(&conn, BalanceConfig::default()).unwrap();
        assert_eq!(loaded.last_diplomacy_at, 77);
        assert_eq!(loaded.diplomacy, state.diplomacy);
        assert_eq!(loaded, state);
        assert!(meta_has_column(&conn, "last_diplomacy_at").unwrap());
        assert_eq!(count_rows(&conn, "faction_standing").unwrap(), 3);
        assert_eq!(count_rows(&conn, "faction_log").unwrap(), 1);
    }

    #[test]
    fn migrate_v18_retries_and_round_trips_spy_reports() {
        let mut conn = open_save(Path::new(":memory:")).unwrap();
        migrate_v18(&conn).unwrap();
        migrate_v18(&conn).unwrap();
        let mut state = GameState::new(3);
        let dest = Coord {
            galaxy: 1,
            system: 1,
            position: 8,
        };
        let mut fleet = Fleet::default();
        fleet.add(ShipKind::LightFighter, 3);
        state.spy_reports.push(crate::SpyReport {
            id: 2,
            target: dest,
            created_at: 40,
            probes_sent: 4,
            probes_lost: 1,
            detected: true,
            score: 3.0,
            tier: "defences".into(),
            resources: Some(Resources::ZERO),
            fleet: Some(fleet),
            defences: Some(fleet),
            garrison: None,
            kind: Some(PlanetType::Rocky),
            size: Some(12),
        });
        state.next_report_id = 3;
        save_state(&mut conn, &state).unwrap();
        let loaded = load_state(&conn, BalanceConfig::default()).unwrap();
        assert_eq!(loaded.spy_reports, state.spy_reports);
        assert_eq!(loaded.next_report_id, 3);
        assert_eq!(loaded, state);
        assert!(missions_has_column(&conn, "spy").unwrap());
        assert_eq!(count_rows(&conn, "spy_reports").unwrap(), 1);
        assert_eq!(count_rows(&conn, "spy_report_ships").unwrap(), 2);
    }

    #[test]
    fn migrate_v19_retries_and_round_trips_planet_defences() {
        let mut conn = open_save(Path::new(":memory:")).unwrap();
        migrate_v19(&conn).unwrap();
        migrate_v19(&conn).unwrap();
        let mut state = GameState::new(3);
        state.defences.add(crate::DefenceKind::LightCannon, 4);
        state.defences.add(crate::DefenceKind::ShieldDome, 2);
        state.defences.add(crate::DefenceKind::Garrison, 9);
        state.ship_queue.push(ShipJob {
            ship: None,
            defence: Some(crate::DefenceKind::IonCannon),
            remaining: 2,
            unit_paid: Resources {
                metal: 200.0,
                crystal: 600.0,
                deuterium: 0.0,
            },
            started_at: Some(10),
            completes_at: Some(190),
        });
        state.colonies.push(crate::Colony {
            coord: crate::Coord {
                galaxy: 1,
                system: 1,
                position: 2,
            },
            kind: crate::PlanetType::Rocky,
            size: 80,
            resources: Resources::ZERO,
            buildings: crate::Buildings::default(),
            fleet: crate::Fleet::default(),
            defences: {
                let mut grid = crate::DefenceGrid::default();
                grid.add(crate::DefenceKind::OrbitalStation, 1);
                grid
            },
            build_queue: Vec::new(),
            rare: None,
            goods: BTreeMap::new(),
        });
        save_state(&mut conn, &state).unwrap();
        let loaded = load_state(&conn, BalanceConfig::default()).unwrap();
        assert_eq!(loaded.defences, state.defences);
        assert_eq!(loaded.ship_queue, state.ship_queue);
        assert_eq!(loaded.colonies[0].defences, state.colonies[0].defences);
        assert_eq!(loaded, state);
        assert_eq!(count_rows(&conn, "planet_defences").unwrap(), 4);
    }

    #[test]
    fn migrate_v20_retries_and_round_trips_laws() {
        let mut conn = open_save(Path::new(":memory:")).unwrap();
        migrate_v20(&conn).unwrap();
        migrate_v20(&conn).unwrap();
        let mut state = GameState::new(10);
        let id = state
            .propose_law("tax_rate", 25.0, crate::Jurisdiction::Empire, 0, 10)
            .unwrap();
        state.approve_law(id, 10).unwrap();
        save_state(&mut conn, &state).unwrap();
        let loaded = load_state(&conn, BalanceConfig::default()).unwrap();
        assert_eq!(loaded.laws, state.laws);
        assert_eq!(loaded, state);
        assert_eq!(count_rows(&conn, "laws").unwrap(), 1);
        assert_eq!(count_rows(&conn, "law_approvals").unwrap(), 1);
    }

    #[test]
    fn migrate_v23_round_trips_combat_logs() {
        let mut conn = open_save(Path::new(":memory:")).unwrap();
        migrate_v23(&conn).unwrap();
        migrate_v23(&conn).unwrap();
        let mut state = GameState::new(10);
        state.fleet.add(crate::ShipKind::LightFighter, 4);
        let _ = state.trigger_raid(10);
        assert!(!state.combat_logs.is_empty());
        save_state(&mut conn, &state).unwrap();
        let loaded = load_state(&conn, BalanceConfig::default()).unwrap();
        assert_eq!(loaded.combat_logs, state.combat_logs);
        assert_eq!(count_rows(&conn, "combat_logs").unwrap(), 1);
        assert!(count_rows(&conn, "combat_log_frames").unwrap() >= 1);
    }

    #[test]
    fn migrate_v23_keeps_empty_terminal_frames_and_ground() {
        let mut conn = open_save(Path::new(":memory:")).unwrap();
        let mut before = Fleet::default();
        before.add(crate::ShipKind::LightFighter, 1);
        let mut state = GameState::new(10);
        state.combat_logs.push(CombatLog {
            at: 10,
            report: CombatReport {
                rounds: 6,
                attacker_before: before,
                defender_before: before,
                attacker_after: Fleet::default(),
                defender_after: Fleet::default(),
                winner: CombatWinner::Draw,
                ground: Some(crate::GroundReport {
                    attacker_strength: 10,
                    defender_strength: 4,
                    attacker_roll: 9,
                    defender_roll: 3,
                    winner: CombatWinner::Attacker,
                }),
                seed: 7,
                frames: vec![
                    CombatFrame {
                        round: 0,
                        attacker: before,
                        defender: before,
                        shots: 0,
                        explosions: 0,
                    },
                    CombatFrame {
                        round: 6,
                        attacker: Fleet::default(),
                        defender: Fleet::default(),
                        shots: 2,
                        explosions: 2,
                    },
                ],
                layered: true,
            },
        });
        save_state(&mut conn, &state).unwrap();
        let loaded = load_state(&conn, BalanceConfig::default()).unwrap();
        assert_eq!(loaded.combat_logs, state.combat_logs);
        assert_eq!(loaded.combat_logs[0].report.frames.len(), 2);
        assert_eq!(loaded.combat_logs[0].report.frames[1].round, 6);
        assert!(loaded.combat_logs[0].report.frames[1].attacker.is_empty());
        assert!(loaded.combat_logs[0].report.frames[1].defender.is_empty());
        assert_eq!(count_rows(&conn, "combat_log_frames").unwrap(), 2);
    }

    #[test]
    fn migrate_v23_round_trips_layered_raid_logs() {
        let mut conn = open_save(Path::new(":memory:")).unwrap();
        let mut state = GameState::new(10);
        state.defences.add(DefenceKind::LightCannon, 6);
        state.defences.add(DefenceKind::OrbitalStation, 1);
        state.defences.add(DefenceKind::ShieldDome, 1);
        state.defences.add(DefenceKind::Garrison, 4);
        state.fleet.add(crate::ShipKind::LightFighter, 2);
        let report = state.trigger_raid(10);
        assert!(report.layered);
        save_state(&mut conn, &state).unwrap();
        let loaded = load_state(&conn, BalanceConfig::default()).unwrap();
        assert_eq!(loaded.combat_logs, state.combat_logs);
        let log = &loaded.combat_logs[0].report;
        assert_eq!(log.frames.last().map(|f| f.round), Some(log.rounds.max(1)));
        assert_eq!(
            log.frames.last().map(|f| f.attacker),
            Some(log.attacker_after)
        );
        assert_eq!(
            log.frames.last().map(|f| f.defender),
            Some(log.defender_after)
        );
    }

    #[test]
    fn migrate_v24_round_trips_daily() {
        let mut conn = open_save(Path::new(":memory:")).unwrap();
        migrate_v24(&conn).unwrap();
        migrate_v24(&conn).unwrap();
        let mut state = GameState::new(10);
        state.daily.streak = 3;
        state.daily.slots[0].progress = 1;
        state.daily.slots[0].rewarded = true;
        save_state(&mut conn, &state).unwrap();
        let loaded = load_state(&conn, BalanceConfig::default()).unwrap();
        assert_eq!(loaded.daily.day_index, state.daily.day_index);
        assert_eq!(loaded.daily.streak, 3);
        assert_eq!(loaded.daily.slots[0].progress, 1);
        assert!(loaded.daily.slots[0].rewarded);
        assert_eq!(count_rows(&conn, "daily_meta").unwrap(), 1);
        assert_eq!(
            count_rows(&conn, "daily_slots").unwrap(),
            i64::try_from(state.daily.slots.len()).unwrap()
        );
    }

    #[test]
    fn migrate_v22_round_trips_achievements() {
        let mut conn = open_save(Path::new(":memory:")).unwrap();
        migrate_v22(&conn).unwrap();
        migrate_v22(&conn).unwrap();
        let mut state = GameState::new(10);
        state.achievements.insert("first_cut".into(), 7);
        state.raids_survived = 3;
        state.outposts_conquered = 2;
        save_state(&mut conn, &state).unwrap();
        let loaded = load_state(&conn, BalanceConfig::default()).unwrap();
        assert_eq!(loaded.achievements.get("first_cut"), Some(&7));
        assert_eq!(loaded.raids_survived, 3);
        assert_eq!(loaded.outposts_conquered, 2);
        assert_eq!(count_rows(&conn, "achievements").unwrap(), 1);
        assert_eq!(count_rows(&conn, "achievement_stats").unwrap(), 1);
    }

    #[test]
    fn schema_21_save_migrates_without_losing_tech_levels() {
        let path = unique_path("v21-achievements");
        let _ = std::fs::remove_file(&path);
        {
            let mut conn = open_save(&path).unwrap();
            let mut state = GameState::new(10);
            state.tech_levels.insert("energy".into(), 3);
            save_state(&mut conn, &state).unwrap();
            conn.execute_batch("DROP TABLE achievements; PRAGMA user_version = 21;")
                .unwrap();
        }
        let conn = open_save(&path).unwrap();
        let version: u32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        let loaded = load_state(&conn, BalanceConfig::default()).unwrap();
        assert_eq!(loaded.tech_level("energy"), 3);
        assert!(loaded.achievements.is_empty());
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    }

    #[test]
    fn migrate_v21_keeps_research_progress_and_stores_exclusive_picks() {
        let mut conn = open_save(Path::new(":memory:")).unwrap();
        migrate_v21(&conn).unwrap();
        migrate_v21(&conn).unwrap();
        let mut state = GameState::new(10);
        state.buildings.research_lab = 1;
        state.tech_levels.insert("energy".into(), 2);
        state.tech_levels.insert("deep_core".into(), 1);
        save_state(&mut conn, &state).unwrap();
        let loaded = load_state(&conn, BalanceConfig::default()).unwrap();
        assert_eq!(loaded.tech_level("energy"), 2);
        assert_eq!(loaded.tech_level("deep_core"), 1);
        assert_eq!(
            loaded
                .exclusive_picks
                .get("mining_method")
                .map(String::as_str),
            Some("deep_core")
        );
        save_state(&mut conn, &loaded).unwrap();
        assert_eq!(count_rows(&conn, "exclusive_picks").unwrap(), 1);
    }

    #[test]
    fn schema_20_save_migrates_without_losing_tech_levels() {
        let path = unique_path("v20-tree");
        let _ = std::fs::remove_file(&path);
        {
            let mut conn = open_save(&path).unwrap();
            let mut state = GameState::new(10);
            state.tech_levels.insert("energy".into(), 3);
            save_state(&mut conn, &state).unwrap();
            conn.execute_batch("DROP TABLE exclusive_picks; PRAGMA user_version = 20;")
                .unwrap();
        }
        let conn = open_save(&path).unwrap();
        let version: u32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        let loaded = load_state(&conn, BalanceConfig::default()).unwrap();
        assert_eq!(loaded.tech_level("energy"), 3);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    }

    #[test]
    fn pact_keeps_an_explicit_empty_raid_schedule_across_save() {
        let mut state = GameState::new(3);
        state.diplomacy.set_standing("pirates", 60.0);
        state.next_raid_at = None;
        let mut conn = open_save(Path::new(":memory:")).unwrap();
        save_state(&mut conn, &state).unwrap();
        let loaded = load_state(&conn, BalanceConfig::default()).unwrap();
        assert_eq!(loaded.next_raid_at, None);
        close_standing_save(&loaded, "pirates", 60.0);
    }

    fn close_standing_save(state: &GameState, id: &str, expected: f64) {
        let got = state.diplomacy.standing(id);
        assert!(
            (got - expected).abs() < 1e-6,
            "{id} standing {got} != {expected}"
        );
    }

    #[test]
    fn load_rejects_a_future_generator_version() {
        let mut conn = open_save(Path::new(":memory:")).unwrap();
        let state = GameState::new(3);
        save_state(&mut conn, &state).unwrap();
        conn.execute(
            "UPDATE meta SET generator_version = ?1 WHERE id = 1",
            [crate::GALAXY_GENERATOR_VERSION as i64 + 1],
        )
        .unwrap();
        let err = load_state(&conn, BalanceConfig::default()).unwrap_err();
        assert!(
            matches!(
                err,
                SaveError::GeneratorTooNew { found, current }
                if found == crate::GALAXY_GENERATOR_VERSION + 1
                    && current == crate::GALAXY_GENERATOR_VERSION
            ),
            "{err:?}"
        );
    }

    #[test]
    fn save_is_locked_is_true_while_held() {
        let path = unique_path("probe-lock");
        assert!(!save_is_locked(&path));
        let holder = lock_save(&path).unwrap();
        assert!(save_is_locked(&path));
        drop(holder);
        assert!(!save_is_locked(&path));
        let _ = std::fs::remove_file(save_lock_path(&path));
    }

    #[test]
    fn inspect_save_reads_without_taking_the_lock() {
        let path = unique_path("inspect");
        let mut conn = open_save(&path).unwrap();
        let mut state = GameState::new(50);
        state.fleet.add(crate::ShipKind::LightFighter, 4);
        save_state(&mut conn, &state).unwrap();
        drop(conn);
        let holder = lock_save(&path).unwrap();
        let err = inspect_save(&path).unwrap_err();
        assert!(matches!(err, SaveError::Locked), "{err:?}");
        drop(holder);
        let glance = inspect_save(&path).unwrap();
        assert_eq!(glance.last_tick_unix, 50);
        assert!(!glance.deterministic);
        assert_eq!(glance.planets, 1);
        assert_eq!(glance.fleet_strength, 4);
        assert_eq!(glance.galaxy_seed, Some(0));
        assert_eq!(glance.generator_version, crate::GALAXY_GENERATOR_VERSION);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(save_lock_path(&path));
    }

    #[test]
    fn inspect_career_reads_without_writing_and_merges_saves() {
        let path = unique_path("career");
        let mut conn = open_save(&path).unwrap();
        let mut state = GameState::new(50);
        state.last_tick_unix = 150;
        state.achievements.insert("first_cut".into(), 7);
        state.completed_wonders.push("observatory".into());
        state.fleet.add(crate::ShipKind::LightFighter, 4);
        state.prestige.currency = 4;
        state.prestige.lifetime = 7;
        state.prestige.owned.insert("mine_focus".into(), 2);
        state.prestige.runs.push(crate::RunStats {
            index: 1,
            started_at: 10,
            ended_at: Some(40),
            score: 80,
            prestige_earned: 4,
            wonders: 1,
            colonies: 1,
            building_levels: 8,
            research_levels: 2,
        });
        save_state(&mut conn, &state).unwrap();
        drop(conn);
        let schema_before = inspect_save(&path).unwrap().schema;
        assert!(!save_is_locked(&path));
        let career = inspect_career(&path).unwrap();
        assert!(!save_is_locked(&path));
        assert_eq!(inspect_save(&path).unwrap().schema, schema_before);
        assert_eq!(career.unlocks.get("first_cut"), Some(&7));
        assert_eq!(career.playtime_secs, 130);
        assert_eq!(career.wonders, 2);
        assert!(career.wonder_ids.contains("observatory"));
        assert_eq!(career.galaxies_wonder, 1);
        assert_eq!(career.galaxies_score, 0);
        assert_eq!(career.prestige_runs, 1);
        assert_eq!(career.prestige_currency, 4);
        assert_eq!(career.prestige_lifetime, 7);
        assert_eq!(career.prestige_owned.get("mine_focus"), Some(&2));
        assert_eq!(career.fleet_ships, 4);
        let mut other = CareerGlance::default();
        other.unlocks.insert("first_cut".into(), 3);
        other.unlocks.insert("lab_open".into(), 9);
        other.playtime_secs = 20;
        other.galaxies_score = 2;
        let mut merged = career;
        merged.merge(other);
        assert_eq!(merged.unlocks.get("first_cut"), Some(&3));
        assert_eq!(merged.unlocks.get("lab_open"), Some(&9));
        assert_eq!(merged.playtime_secs, 150);
        assert_eq!(merged.galaxies_score, 2);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(save_lock_path(&path));
    }

    #[test]
    fn reset_available_is_per_save_not_merged_maxima() {
        let cfg = crate::PrestigeConfig::default();
        let mut a = CareerGlance {
            building_levels: cfg.min_score.saturating_sub(1),
            ..CareerGlance::default()
        };
        let b = CareerGlance {
            research_levels: cfg.min_score.saturating_sub(1),
            ..CareerGlance::default()
        };
        assert!(!a.reset_available(&cfg));
        assert!(!b.reset_available(&cfg));
        a.merge(b);
        assert!(
            a.reset_available(&cfg),
            "merged maxima can look eligible; callers must score each save"
        );
    }

    #[test]
    fn inspect_save_flags_newer_schema_corrupt_and_harness_saves() {
        let newer = unique_path("inspect-new");
        let conn = open_save(&newer).unwrap();
        conn.pragma_update(None, "user_version", SCHEMA_VERSION + 1)
            .unwrap();
        drop(conn);
        let err = inspect_save(&newer).unwrap_err();
        assert!(
            matches!(err, SaveError::SchemaTooNew { found, .. } if found == SCHEMA_VERSION + 1),
            "{err:?}"
        );

        let future_gen = unique_path("inspect-generator");
        let mut conn = open_save(&future_gen).unwrap();
        save_state(&mut conn, &GameState::new(1)).unwrap();
        conn.execute(
            "UPDATE meta SET generator_version = ?1 WHERE id = 1",
            [crate::GALAXY_GENERATOR_VERSION as i64 + 1],
        )
        .unwrap();
        drop(conn);
        let err = inspect_save(&future_gen).unwrap_err();
        assert!(
            matches!(
                err,
                SaveError::GeneratorTooNew { found, current }
                if found == crate::GALAXY_GENERATOR_VERSION + 1
                    && current == crate::GALAXY_GENERATOR_VERSION
            ),
            "{err:?}"
        );

        let corrupt = unique_path("inspect-corrupt");
        std::fs::write(&corrupt, b"not a database").unwrap();
        let err = inspect_save(&corrupt).unwrap_err();
        assert!(
            matches!(err, SaveError::Sqlite(_) | SaveError::Corrupt(_)),
            "{err:?}"
        );

        let harness = unique_path("inspect-harness");
        let mut conn = open_save(&harness).unwrap();
        let state = GameState::with_seed(9, BalanceConfig::default(), 3);
        save_state(&mut conn, &state).unwrap();
        drop(conn);
        let glance = inspect_save(&harness).unwrap();
        assert!(glance.deterministic);
        assert_eq!(glance.galaxy_seed, Some(3));

        let legacy = unique_path("inspect-pre-seed");
        let conn = Connection::open(&legacy).unwrap();
        conn.execute_batch(
            "
            CREATE TABLE meta (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                last_tick_unix INTEGER NOT NULL,
                rng_seed INTEGER NOT NULL
            );
            INSERT INTO meta (id, last_tick_unix, rng_seed) VALUES (1, 50, 1);
            PRAGMA user_version = 4;
            ",
        )
        .unwrap();
        drop(conn);
        let glance = inspect_save(&legacy).unwrap();
        assert_eq!(glance.schema, 4);
        assert_eq!(glance.galaxy_seed, None);

        let _ = std::fs::remove_file(&newer);
        let _ = std::fs::remove_file(&corrupt);
        let _ = std::fs::remove_file(&harness);
        let _ = std::fs::remove_file(&legacy);
    }
}
