//! Scriptable JSON commands. Same core actions as the TUI; no extra powers.

use hoshi_core::{
    BalanceConfig, BuildingKind, Clock, Coord, DefenceKind, FakeClock, Fleet, GameState,
    ResourceKind, SCHEMA_VERSION as SAVE_SCHEMA, SaveError, ShipKind, SpyReport, SystemClock,
    load_or_new, load_or_new_with_seed, lock_save, open_save, resolve_save_path, save_state,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub const SCHEMA_VERSION: u32 = 1;
pub const EXIT_OK: i32 = 0;
pub const EXIT_INVALID: i32 = 2;
pub const EXIT_REFUSED: i32 = 3;
pub const EXIT_LOCKED: i32 = 4;
pub const EXIT_INTERNAL: i32 = 5;

pub struct CliOutcome {
    pub json: String,
    pub exit: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Command {
    State,
    Build {
        planet: Coord,
        building: BuildingKind,
    },
    Research {
        tech: String,
    },
    Cancel {
        item: CancelItem,
    },
    FleetSend {
        from: Coord,
        to: Coord,
        mission: MissionKind,
        ships: Fleet,
    },
    Tick {
        advance: u64,
    },
    Prestige,
    PrestigeBuy {
        id: String,
    },
    PrestigeReset,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CancelItem {
    Build(usize),
    Research,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MissionKind {
    Attack,
    Invade,
    Colonize,
    Expedition,
    Spy,
}

#[derive(Debug)]
struct CliOpts {
    save: Option<PathBuf>,
    seed: Option<u64>,
    balance: Option<PathBuf>,
    command: Command,
}

#[derive(Debug)]
enum CliError {
    Invalid(String),
    Refused(String),
    Locked,
    Internal(String),
}

impl CliError {
    fn exit(&self) -> i32 {
        match self {
            CliError::Invalid(_) => EXIT_INVALID,
            CliError::Refused(_) => EXIT_REFUSED,
            CliError::Locked => EXIT_LOCKED,
            CliError::Internal(_) => EXIT_INTERNAL,
        }
    }

    fn code(&self) -> &'static str {
        match self {
            CliError::Invalid(_) => "invalid",
            CliError::Refused(_) => "refused",
            CliError::Locked => "locked",
            CliError::Internal(_) => "internal",
        }
    }

    fn message(&self) -> String {
        match self {
            CliError::Invalid(m) | CliError::Refused(m) | CliError::Internal(m) => m.clone(),
            CliError::Locked => "save is locked".into(),
        }
    }
}

/// True when the argv is a script command (or a typo of one), not a TUI launch.
pub fn is_cli_invocation(args: &[String]) -> bool {
    let mut i = 1;
    while i < args.len() {
        let a = args[i].as_str();
        if a == "--save" || a == "--balance" {
            if i + 1 >= args.len() {
                return true;
            }
            i += 2;
            continue;
        }
        if a.starts_with("--save=")
            || a.starts_with("--balance=")
            || a == "--continue"
            || a == "new"
        {
            i += 1;
            continue;
        }
        return true;
    }
    false
}

pub fn run_cli(args: &[String]) -> CliOutcome {
    match run_cli_inner(args) {
        Ok(value) => CliOutcome {
            json: render(value),
            exit: EXIT_OK,
        },
        Err(err) => CliOutcome {
            json: render(json!({
                "schema_version": SCHEMA_VERSION,
                "ok": false,
                "error": { "code": err.code(), "message": err.message() },
            })),
            exit: err.exit(),
        },
    }
}

fn render(value: Value) -> String {
    let mut out = serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".into());
    out.push('\n');
    out
}

fn run_cli_inner(args: &[String]) -> Result<Value, CliError> {
    let opts = parse_cli(args)?;
    let save = resolve_save_path(
        opts.save.as_deref(),
        std::env::var_os("HOSHI_SAVE").as_deref(),
    );
    let _lock = lock_save(&save).map_err(|e| match e {
        SaveError::Locked => CliError::Locked,
        other => CliError::Internal(other.to_string()),
    })?;
    let config = match opts.balance.as_deref() {
        Some(path) => {
            BalanceConfig::load_file(path).map_err(|e| CliError::Invalid(e.to_string()))?
        }
        None => match std::env::var_os("HOSHI_BALANCE") {
            Some(path) => BalanceConfig::load_file(Path::new(&path))
                .map_err(|e| CliError::Invalid(e.to_string()))?,
            None => BalanceConfig::default(),
        },
    };
    let mut conn = open_save(&save).map_err(|e| CliError::Internal(e.to_string()))?;
    let mut state = if let Some(seed) = opts.seed {
        load_or_new_with_seed(&mut conn, 0, config, seed)
    } else {
        load_or_new(&mut conn, SystemClock.now_unix(), config)
    }
    .map_err(|e| CliError::Internal(e.to_string()))?;
    let now = now_for(&state);
    let value = apply(&mut state, now, &opts.command)?;
    save_state(&mut conn, &state).map_err(|e| CliError::Internal(e.to_string()))?;
    Ok(value)
}

fn now_for(state: &GameState) -> u64 {
    if state.deterministic {
        state.last_tick_unix
    } else {
        SystemClock.now_unix()
    }
}

fn apply(state: &mut GameState, now: u64, command: &Command) -> Result<Value, CliError> {
    match command {
        Command::State => {
            if !state.deterministic {
                state.catch_up(now);
            }
            Ok(state_value(state))
        }
        Command::Build { planet, building } => {
            state
                .enqueue_build_on(*planet, *building, now)
                .map_err(|e| CliError::Refused(e.to_string()))?;
            Ok(ok_state(state, "queued build"))
        }
        Command::Research { tech } => {
            state
                .start_research(tech, now)
                .map_err(|e| CliError::Refused(e.to_string()))?;
            Ok(ok_state(state, "started research"))
        }
        Command::Cancel { item } => {
            match item {
                CancelItem::Build(index) => {
                    state
                        .cancel_build(*index, now)
                        .map_err(|e| CliError::Refused(e.to_string()))?;
                }
                CancelItem::Research => {
                    state
                        .cancel_research(now)
                        .map_err(|e| CliError::Refused(e.to_string()))?;
                }
            }
            Ok(ok_state(state, "cancelled"))
        }
        Command::FleetSend {
            from,
            to,
            mission,
            ships,
        } => {
            if *from != state.home {
                return Err(CliError::Refused(
                    "fleet send from a colony is not available".into(),
                ));
            }
            match mission {
                MissionKind::Attack => {
                    let _ = state
                        .launch_attack(*ships, *to, 100, now)
                        .map_err(|e| CliError::Refused(e.to_string()))?;
                }
                MissionKind::Invade => {
                    let _ = state
                        .launch_invasion(*ships, *to, 100, now)
                        .map_err(|e| CliError::Refused(e.to_string()))?;
                }
                MissionKind::Colonize => {
                    if ships.get(ShipKind::ColonyShip) != 1 || ships.total() != 1 {
                        return Err(CliError::Invalid(
                            "colonize needs exactly colony_ship:1".into(),
                        ));
                    }
                    let _ = state
                        .launch_colony(*to, 100, now)
                        .map_err(|e| CliError::Refused(e.to_string()))?;
                }
                MissionKind::Expedition => {
                    let dest = state.expedition_dest();
                    if *to != dest {
                        return Err(CliError::Invalid(format!("expedition dest must be {dest}")));
                    }
                    let hold = state.config.expedition.min_hold_secs;
                    let _ = state
                        .launch_expedition(*ships, hold, 100, now)
                        .map_err(|e| CliError::Refused(e.to_string()))?;
                }
                MissionKind::Spy => {
                    let _ = state
                        .launch_spy(*ships, *to, 100, now)
                        .map_err(|e| CliError::Refused(e.to_string()))?;
                }
            }
            Ok(ok_state(state, "fleet launched"))
        }
        Command::Tick { advance } => {
            if !state.deterministic {
                return Err(CliError::Refused(
                    "tick --advance is refused on normal saves".into(),
                ));
            }
            let clock = FakeClock::new(state.last_tick_unix.saturating_add(*advance));
            state.catch_up(clock.now_unix());
            Ok(ok_state(state, "advanced"))
        }
        Command::Prestige => {
            if !state.deterministic {
                state.catch_up(now);
            }
            Ok(ok_state(state, "prestige"))
        }
        Command::PrestigeBuy { id } => {
            state
                .buy_prestige_bonus(id, now)
                .map_err(|e| CliError::Refused(e.to_string()))?;
            Ok(ok_state(state, "bought prestige bonus"))
        }
        Command::PrestigeReset => {
            state
                .prestige_reset(now)
                .map_err(|e| CliError::Refused(e.to_string()))?;
            Ok(ok_state(state, "prestiged"))
        }
    }
}

fn ok_state(state: &GameState, message: &str) -> Value {
    let mut value = state_value(state);
    value["message"] = json!(message);
    value
}

fn state_value(state: &GameState) -> Value {
    let raid = state.next_raid_at.map(|at| {
        json!({
            "lands_in_secs": at.saturating_sub(state.last_tick_unix),
        })
    });
    let queue: Vec<Value> = state
        .build_queue
        .iter()
        .enumerate()
        .map(|(i, job)| {
            json!({
                "index": i,
                "building": job.building.as_key(),
                "from_level": job.from_level,
                "completes_at": job.completes_at,
            })
        })
        .collect();
    let mut buildings = serde_json::Map::new();
    for kind in BuildingKind::ALL {
        buildings.insert(kind.as_key().into(), json!(state.buildings.get(kind)));
    }
    let mut fleet = serde_json::Map::new();
    for kind in ShipKind::ALL {
        fleet.insert(kind.as_key().into(), json!(state.fleet.get(kind)));
    }
    let mut defences = serde_json::Map::new();
    for kind in DefenceKind::ALL {
        defences.insert(kind.as_key().into(), json!(state.defences.get(kind)));
    }
    let mut techs = serde_json::Map::new();
    for spec in state.config.techs.iter() {
        techs.insert(spec.id.clone(), json!(state.tech_level(&spec.id)));
    }
    let planets: Vec<Value> = state
        .owned_coords()
        .into_iter()
        .map(|coord| {
            json!({
                "coord": coord.to_string(),
                "home": coord == state.home,
                "rare": state.rare_bonus_at(coord).map(|row| row.id.clone()),
            })
        })
        .collect();
    let events: Vec<Value> = state
        .events
        .iter()
        .map(|e| json!({"at": e.at, "text": e.text}))
        .collect();
    json!({
        "schema_version": SCHEMA_VERSION,
        "ok": true,
        "now": state.last_tick_unix,
        "deterministic": state.deterministic,
        "save_schema": SAVE_SCHEMA,
        "resources": {
            "metal": state.resources.metal,
            "crystal": state.resources.crystal,
            "deuterium": state.resources.deuterium,
        },
        "rates": {
            "metal": state.production_per_hour(ResourceKind::Metal),
            "crystal": state.production_per_hour(ResourceKind::Crystal),
            "deuterium": state.production_per_hour(ResourceKind::Deuterium),
        },
        "energy": {
            "produced": state.energy_produced(),
            "consumed": state.energy_consumed(),
            "factor": state.energy_factor(),
        },
        "buildings": buildings,
        "build_queue": queue,
        "research": {
            "current": state.research.as_ref().map(|j| json!({
                "tech": j.tech,
                "from_level": j.from_level,
                "completes_at": j.completes_at,
            })),
            "levels": techs,
        },
        "fleet": fleet,
        "defences": defences,
        "missions": state.missions.iter().map(|m| json!({
            "id": m.id,
            "origin": m.origin.to_string(),
            "dest": m.dest.to_string(),
            "phase": m.phase.as_key(),
            "phase_ends_at": m.phase_ends_at,
        })).collect::<Vec<_>>(),
        "planets": planets,
        "outposts": state.outposts.iter().map(|o| json!({
            "coord": o.coord.to_string(),
            "defences": o.defences.total(),
            "garrison": o.garrison,
            "rare": o.rare,
            "report": state.latest_report(o.coord).map(|r| spy_report_value(r, state.last_tick_unix)),
        })).collect::<Vec<_>>(),
        "spy_reports": state.spy_reports.iter().map(|r| spy_report_value(r, state.last_tick_unix)).collect::<Vec<_>>(),
        "events": events,
        "raid": raid,
        "prestige": {
            "currency": state.prestige.currency,
            "lifetime": state.prestige.lifetime,
            "run": state.prestige.current.index,
            "score": state.prestige_score(),
            "eligible": state.can_prestige(),
            "owned": state.prestige.owned,
        },
    })
}

fn spy_report_value(report: &SpyReport, now: u64) -> Value {
    json!({
        "id": report.id,
        "target": report.target.to_string(),
        "age_secs": report.age_secs(now),
        "tier": report.tier,
        "detected": report.detected,
        "probes_sent": report.probes_sent,
        "probes_lost": report.probes_lost,
        "score": report.score,
        "kind": report.kind.map(|k| k.as_key()),
        "size": report.size,
        "resources": report.resources.map(|r| json!({
            "metal": r.metal,
            "crystal": r.crystal,
            "deuterium": r.deuterium,
        })),
        "fleet": report.fleet.map(fleet_counts),
        "defences": report.defences.map(fleet_counts),
        "garrison": report.garrison,
    })
}

fn fleet_counts(fleet: Fleet) -> Value {
    let mut map = serde_json::Map::new();
    for kind in ShipKind::ALL {
        let n = fleet.get(kind);
        if n > 0 {
            map.insert(kind.as_key().into(), json!(n));
        }
    }
    Value::Object(map)
}

fn parse_cli(args: &[String]) -> Result<CliOpts, CliError> {
    let mut save = None;
    let mut seed = None;
    let mut balance = None;
    let mut rest = Vec::new();
    let mut i = 1;
    while i < args.len() {
        let a = args[i].as_str();
        if let Some(v) = a.strip_prefix("--save=") {
            save = Some(PathBuf::from(v));
            i += 1;
            continue;
        }
        if a == "--save" {
            save = Some(PathBuf::from(need(args, i + 1, "--save")?));
            i += 2;
            continue;
        }
        if let Some(v) = a.strip_prefix("--seed=") {
            seed = Some(parse_seed(v)?);
            i += 1;
            continue;
        }
        if a == "--seed" {
            seed = Some(parse_seed(need(args, i + 1, "--seed")?)?);
            i += 2;
            continue;
        }
        if let Some(v) = a.strip_prefix("--balance=") {
            balance = Some(PathBuf::from(v));
            i += 1;
            continue;
        }
        if a == "--balance" {
            balance = Some(PathBuf::from(need(args, i + 1, "--balance")?));
            i += 2;
            continue;
        }
        if a == "--json" {
            i += 1;
            continue;
        }
        rest.push(args[i].clone());
        i += 1;
    }
    let command = parse_command(&rest)?;
    Ok(CliOpts {
        save,
        seed,
        balance,
        command,
    })
}

fn need<'a>(args: &'a [String], i: usize, flag: &str) -> Result<&'a str, CliError> {
    args.get(i)
        .map(String::as_str)
        .ok_or_else(|| CliError::Invalid(format!("{flag} needs a value")))
}

fn parse_seed(v: &str) -> Result<u64, CliError> {
    v.parse()
        .map_err(|_| CliError::Invalid(format!("invalid --seed {v}")))
}

fn parse_command(args: &[String]) -> Result<Command, CliError> {
    let Some(name) = args.first().map(String::as_str) else {
        return Err(CliError::Invalid("missing command".into()));
    };
    match name {
        "state" => {
            expect_end(args, 1)?;
            Ok(Command::State)
        }
        "build" => {
            let planet = parse_coord(nth(args, 1, "build <planet> <building>")?)?;
            let building = BuildingKind::from_key(nth(args, 2, "build <planet> <building>")?)
                .ok_or_else(|| CliError::Invalid("unknown building".into()))?;
            expect_end(args, 3)?;
            Ok(Command::Build { planet, building })
        }
        "research" => {
            let tech = nth(args, 1, "research <tech>")?.to_string();
            expect_end(args, 2)?;
            Ok(Command::Research { tech })
        }
        "cancel" => {
            let item = parse_cancel(nth(args, 1, "cancel <queue-item>")?)?;
            expect_end(args, 2)?;
            Ok(Command::Cancel { item })
        }
        "fleet" => {
            if nth(args, 1, "fleet send ...")? != "send" {
                return Err(CliError::Invalid("expected fleet send".into()));
            }
            let from = parse_coord(nth(args, 2, "fleet send <from> <to> <mission> <ships...>")?)?;
            let to = parse_coord(nth(args, 3, "fleet send <from> <to> <mission> <ships...>")?)?;
            let mission =
                parse_mission(nth(args, 4, "fleet send <from> <to> <mission> <ships...>")?)?;
            let ships = parse_ships(&args[5..])?;
            Ok(Command::FleetSend {
                from,
                to,
                mission,
                ships,
            })
        }
        "prestige" => match args.get(1).map(String::as_str) {
            None => {
                expect_end(args, 1)?;
                Ok(Command::Prestige)
            }
            Some("buy") => {
                let id = nth(args, 2, "prestige buy <id>")?.to_string();
                expect_end(args, 3)?;
                Ok(Command::PrestigeBuy { id })
            }
            Some("reset") => {
                expect_end(args, 2)?;
                Ok(Command::PrestigeReset)
            }
            Some(other) => Err(CliError::Invalid(format!(
                "unknown prestige command {other}"
            ))),
        },
        "tick" => {
            let mut advance = None;
            let mut i = 1;
            while i < args.len() {
                let a = args[i].as_str();
                if let Some(v) = a.strip_prefix("--advance=") {
                    advance = Some(parse_advance(v)?);
                    i += 1;
                    continue;
                }
                if a == "--advance" {
                    advance = Some(parse_advance(nth(args, i + 1, "tick --advance <dur>")?)?);
                    i += 2;
                    continue;
                }
                return Err(CliError::Invalid(format!("unknown tick flag {a}")));
            }
            Ok(Command::Tick {
                advance: advance.ok_or_else(|| CliError::Invalid("tick needs --advance".into()))?,
            })
        }
        other => Err(CliError::Invalid(format!("unknown command {other}"))),
    }
}

fn nth<'a>(args: &'a [String], i: usize, usage: &str) -> Result<&'a str, CliError> {
    args.get(i)
        .map(String::as_str)
        .ok_or_else(|| CliError::Invalid(format!("usage: hoshi {usage}")))
}

fn expect_end(args: &[String], used: usize) -> Result<(), CliError> {
    match args.get(used) {
        Some(extra) => Err(CliError::Invalid(format!("unexpected argument {extra}"))),
        None => Ok(()),
    }
}

fn parse_coord(text: &str) -> Result<Coord, CliError> {
    if text == "home" {
        return Ok(Coord::HOME);
    }
    let mut parts = text.split(':');
    let galaxy = parse_u16(parts.next(), text)?;
    let system = parse_u16(parts.next(), text)?;
    let position = parse_u16(parts.next(), text)?;
    if parts.next().is_some() {
        return Err(CliError::Invalid(format!("invalid coord {text}")));
    }
    Ok(Coord {
        galaxy,
        system,
        position,
    })
}

fn parse_u16(part: Option<&str>, text: &str) -> Result<u16, CliError> {
    part.ok_or_else(|| CliError::Invalid(format!("invalid coord {text}")))?
        .parse()
        .map_err(|_| CliError::Invalid(format!("invalid coord {text}")))
}

fn parse_cancel(text: &str) -> Result<CancelItem, CliError> {
    if text == "research" {
        return Ok(CancelItem::Research);
    }
    let rest = text.strip_prefix("build:").unwrap_or(text);
    let index: usize = rest
        .parse()
        .map_err(|_| CliError::Invalid(format!("invalid queue item {text}")))?;
    Ok(CancelItem::Build(index))
}

fn parse_mission(text: &str) -> Result<MissionKind, CliError> {
    match text {
        "attack" => Ok(MissionKind::Attack),
        "invade" => Ok(MissionKind::Invade),
        "colonize" => Ok(MissionKind::Colonize),
        "expedition" => Ok(MissionKind::Expedition),
        "spy" => Ok(MissionKind::Spy),
        other => Err(CliError::Invalid(format!("unknown mission {other}"))),
    }
}

fn parse_ships(args: &[String]) -> Result<Fleet, CliError> {
    let mut fleet = Fleet::default();
    if args.is_empty() {
        return Err(CliError::Invalid("fleet send needs ships".into()));
    }
    for spec in args {
        let (kind, count) = spec
            .split_once(':')
            .ok_or_else(|| CliError::Invalid(format!("invalid ship spec {spec}")))?;
        let kind = ShipKind::from_key(kind)
            .ok_or_else(|| CliError::Invalid(format!("unknown ship {kind}")))?;
        let count: u32 = count
            .parse()
            .map_err(|_| CliError::Invalid(format!("invalid ship count {spec}")))?;
        fleet.add(kind, count);
    }
    Ok(fleet)
}

fn parse_advance(text: &str) -> Result<u64, CliError> {
    let err = || CliError::Invalid(format!("invalid --advance {text}"));
    if let Some(n) = text.strip_suffix('s') {
        return n.parse().map_err(|_| err());
    }
    if let Some(n) = text.strip_suffix('m') {
        let n: u64 = n.parse().map_err(|_| err())?;
        return Ok(n.saturating_mul(60));
    }
    if let Some(n) = text.strip_suffix('h') {
        let n: u64 = n.parse().map_err(|_| err())?;
        return Ok(n.saturating_mul(3600));
    }
    if let Some(n) = text.strip_suffix('d') {
        let n: u64 = n.parse().map_err(|_| err())?;
        return Ok(n.saturating_mul(86_400));
    }
    text.parse().map_err(|_| err())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoshi_core::BuildError;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_save(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "hoshi-cli-{}-{}-{tag}.sqlite",
            std::process::id(),
            nanos
        ))
    }

    fn args(parts: &[&str]) -> Vec<String> {
        std::iter::once("hoshi")
            .chain(parts.iter().copied())
            .map(str::to_string)
            .collect()
    }

    fn run_on(save: &Path, parts: &[&str]) -> (Value, i32) {
        let mut argv = args(parts);
        argv.insert(1, format!("--save={}", save.display()));
        let out = run_cli(&argv);
        let value: Value = serde_json::from_str(&out.json).unwrap();
        (value, out.exit)
    }

    #[test]
    fn parse_build_and_tick_and_fleet() {
        let opts = parse_cli(&args(&["--seed", "1", "build", "home", "metal_mine"])).unwrap();
        assert_eq!(opts.seed, Some(1));
        assert_eq!(
            opts.command,
            Command::Build {
                planet: Coord::HOME,
                building: BuildingKind::MetalMine,
            }
        );
        let opts = parse_cli(&args(&["tick", "--advance", "3m"])).unwrap();
        assert_eq!(opts.command, Command::Tick { advance: 180 });
        let opts = parse_cli(&args(&[
            "fleet",
            "send",
            "1:1:1",
            "1:1:8",
            "attack",
            "light_fighter:2",
        ]))
        .unwrap();
        match opts.command {
            Command::FleetSend {
                to, mission, ships, ..
            } => {
                assert_eq!(to.position, 8);
                assert_eq!(mission, MissionKind::Attack);
                assert_eq!(ships.get(ShipKind::LightFighter), 2);
            }
            other => panic!("{other:?}"),
        }
        let opts = parse_cli(&args(&[
            "fleet",
            "send",
            "1:1:1",
            "1:1:8",
            "spy",
            "espionage_probe:3",
        ]))
        .unwrap();
        match opts.command {
            Command::FleetSend { mission, ships, .. } => {
                assert_eq!(mission, MissionKind::Spy);
                assert_eq!(ships.get(ShipKind::EspionageProbe), 3);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn state_omits_rng_and_galaxy_seed() {
        let save = unique_save("visible");
        let (value, exit) = run_on(&save, &["--seed=9", "state", "--json"]);
        assert_eq!(exit, EXIT_OK);
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["ok"], true);
        assert_eq!(value["deterministic"], true);
        assert!(value.get("rng_seed").is_none());
        assert!(value.get("galaxy_seed").is_none());
        let text = value.to_string();
        assert!(!text.contains("rng_seed"));
        assert!(!text.contains("galaxy_seed"));
        let _ = std::fs::remove_file(&save);
    }

    #[test]
    fn tick_refused_on_live_save() {
        let save = unique_save("live");
        let (_, exit) = run_on(&save, &["state"]);
        assert_eq!(exit, EXIT_OK);
        let (value, exit) = run_on(&save, &["tick", "--advance", "60"]);
        assert_eq!(exit, EXIT_REFUSED);
        assert_eq!(value["error"]["code"], "refused");
        let _ = std::fs::remove_file(&save);
    }

    #[test]
    fn build_then_tick_matches_core_and_replays() {
        let save_a = unique_save("a");
        let save_b = unique_save("b");
        for save in [&save_a, &save_b] {
            let (_, e) = run_on(save, &["--seed=1", "build", "home", "metal_mine"]);
            assert_eq!(e, EXIT_OK);
            let (_, e) = run_on(save, &["tick", "--advance", "180"]);
            assert_eq!(e, EXIT_OK);
        }
        let (a, _) = run_on(&save_a, &["state"]);
        let (b, _) = run_on(&save_b, &["state"]);
        assert_eq!(a, b);
        assert_eq!(a["buildings"]["metal_mine"], 2);

        let mut core = GameState::with_seed(0, BalanceConfig::default(), 1);
        core.enqueue_build(BuildingKind::MetalMine, 0).unwrap();
        core.catch_up(180);
        assert_eq!(core.buildings.metal_mine, 2);
        assert!((core.resources.metal - a["resources"]["metal"].as_f64().unwrap()).abs() < 1e-6);

        let _ = std::fs::remove_file(&save_a);
        let _ = std::fs::remove_file(&save_b);
    }

    #[test]
    fn unaffordable_build_is_refused() {
        let save = unique_save("poor");
        let mut conn = open_save(&save).unwrap();
        let mut state = GameState::with_seed(0, BalanceConfig::default(), 1);
        state.resources = hoshi_core::Resources::ZERO;
        save_state(&mut conn, &state).unwrap();
        drop(conn);
        let (value, exit) = run_on(&save, &["build", "home", "metal_mine"]);
        assert_eq!(exit, EXIT_REFUSED);
        assert!(
            value["error"]["message"]
                .as_str()
                .unwrap()
                .contains("not enough")
        );
        let _ = std::fs::remove_file(&save);
        let _ = BuildError::QueueFull { cap: 1 };
    }

    #[test]
    fn published_schema_lists_required_response_keys() {
        let schema: Value = serde_json::from_str(include_str!("../../../docs/schema/cli-v1.json"))
            .expect("schema json");
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "schema_version"));
        assert!(required.iter().any(|v| v == "ok"));
        let save = unique_save("schema");
        let (value, exit) = run_on(&save, &["--seed=2", "state"]);
        assert_eq!(exit, EXIT_OK);
        for key in required {
            let key = key.as_str().unwrap();
            assert!(value.get(key).is_some(), "missing {key}");
        }
        let _ = std::fs::remove_file(&save);
    }

    #[test]
    fn typos_and_cli_flags_are_cli_invocations() {
        assert!(is_cli_invocation(&args(&["stete"])));
        assert!(is_cli_invocation(&args(&["--json"])));
        assert!(is_cli_invocation(&args(&["--seed", "1"])));
        assert!(is_cli_invocation(&args(&["tick", "--advance"])));
        assert!(is_cli_invocation(&args(&["--save", "x.sqlite", "state"])));
        assert!(is_cli_invocation(&args(&["--save"])));
        assert!(is_cli_invocation(&args(&["--balance"])));
        assert!(!is_cli_invocation(&args(&[])));
        assert!(!is_cli_invocation(&args(&["--save", "x.sqlite"])));
        assert!(!is_cli_invocation(&args(&["--balance", "b.toml"])));
        assert!(!is_cli_invocation(&args(&["--continue"])));
        assert!(!is_cli_invocation(&args(&["new"])));
        let out = run_cli(&args(&["stete"]));
        assert_eq!(out.exit, EXIT_INVALID);
        let value: Value = serde_json::from_str(&out.json).unwrap();
        assert_eq!(value["error"]["code"], "invalid");
        let out = run_cli(&args(&["--json"]));
        assert_eq!(out.exit, EXIT_INVALID);
        let out = run_cli(&args(&["--save"]));
        assert_eq!(out.exit, EXIT_INVALID);
    }

    #[test]
    fn extra_args_are_invalid() {
        for parts in [
            &["state", "typo"][..],
            &["build", "home", "metal_mine", "typo"],
            &["research", "energy", "typo"],
            &["cancel", "0", "typo"],
        ] {
            let err = parse_cli(&args(parts)).unwrap_err();
            assert!(
                matches!(err, CliError::Invalid(ref m) if m.contains("unexpected argument typo")),
                "{parts:?} -> {err:?}"
            );
        }
    }

    #[test]
    fn colonize_rejects_non_colony_payload() {
        let save = unique_save("colonize");
        let (_, e) = run_on(&save, &["--seed=1", "state"]);
        assert_eq!(e, EXIT_OK);
        let (value, exit) = run_on(
            &save,
            &[
                "fleet",
                "send",
                "home",
                "1:1:8",
                "colonize",
                "light_fighter:1",
            ],
        );
        assert_eq!(exit, EXIT_INVALID);
        assert_eq!(value["error"]["code"], "invalid");
        assert!(
            value["error"]["message"]
                .as_str()
                .unwrap()
                .contains("colony_ship:1")
        );
        let (value, exit) = run_on(
            &save,
            &[
                "fleet",
                "send",
                "home",
                "1:1:8",
                "colonize",
                "colony_ship:1",
                "light_fighter:1",
            ],
        );
        assert_eq!(exit, EXIT_INVALID);
        assert_eq!(value["error"]["code"], "invalid");
        let (value, exit) = run_on(
            &save,
            &[
                "fleet",
                "send",
                "home",
                "1:1:8",
                "colonize",
                "colony_ship:1",
            ],
        );
        assert_eq!(exit, EXIT_REFUSED);
        assert_eq!(value["error"]["code"], "refused");
        let _ = std::fs::remove_file(&save);
    }

    #[test]
    fn expedition_rejects_a_non_slot_destination() {
        let save = unique_save("expedition");
        let (_, e) = run_on(&save, &["--seed=1", "state"]);
        assert_eq!(e, EXIT_OK);
        let (value, exit) = run_on(
            &save,
            &[
                "fleet",
                "send",
                "home",
                "1:1:8",
                "expedition",
                "light_fighter:1",
            ],
        );
        assert_eq!(exit, EXIT_INVALID);
        assert_eq!(value["error"]["code"], "invalid");
        assert!(
            value["error"]["message"]
                .as_str()
                .unwrap()
                .contains("expedition dest")
        );
        let _ = std::fs::remove_file(&save);
    }
}
