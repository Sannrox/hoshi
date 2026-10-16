//! Scriptable JSON commands. Same core actions as the TUI; no extra powers.

use hoshi_core::{
    Actor, AssignmentTarget, BackendKind, BalanceConfig, Blueprint, BlueprintCatalog, BuildingKind,
    Classification, ClassifiedKind, Clock, Coord, Customer, DefenceKind, FakeClock, Fleet,
    GameState, HistoryKind, HistoryQuery, Intent, Jurisdiction, JurisdictionKind, MoonFacility,
    OrderSide, Payload, Permission, PlanRequest, ResourceKind, RoleKind, RoundAction,
    SCHEMA_VERSION as SAVE_SCHEMA, SaveError, ShipKind, SpeciesCatalog, SpyReport, SystemClock,
    TransferKind, advise, blueprint_dir, inspect_save, lock_save, open_storage, resolve_save_path,
    save_is_locked,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;

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
enum AutomationActionSpec {
    QueueBuild {
        planet: Coord,
        building: BuildingKind,
    },
    Transfer {
        from: Coord,
        to: Coord,
        resource: ResourceKind,
        amount: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Command {
    State {
        actor: Actor,
    },
    Classify {
        kind: ClassifiedKind,
        id: String,
        class: Classification,
        compartments: Vec<String>,
        actor: Actor,
    },
    Share {
        faction: String,
        class: Classification,
        compartments: Vec<String>,
    },
    Incidents {
        actor: Actor,
    },
    Build {
        planet: Coord,
        building: BuildingKind,
        actor: Actor,
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
    Goals,
    GoalsKeep,
    Automation,
    AutomationDryRun,
    AutomationToggle {
        id: u64,
    },
    AutomationRemove {
        id: u64,
    },
    AutomationAdd {
        cond: String,
        planet: Coord,
        resource: ResourceKind,
        amount: String,
        action: AutomationActionSpec,
    },
    LawPropose {
        template: String,
        param: Option<String>,
        jurisdiction: Option<String>,
        effective: u64,
    },
    LawApprove {
        id: u64,
    },
    LawRepeal {
        id: u64,
        effective: u64,
    },
    MoonBuild {
        planet: Coord,
        facility: MoonFacility,
    },
    MoonJump {
        from: Coord,
        to: Coord,
        ships: Fleet,
    },
    MoonScan {
        planet: Coord,
    },
    TutorialSkip,
    TutorialReset,
    TacticsSet {
        formation: Option<String>,
        stance: Option<String>,
        retreat: Option<String>,
    },
    TacticsAction {
        action: RoundAction,
    },
    Feedback {
        note: String,
    },
    Advise,
    Log {
        query: HistoryQuery,
        lineage: Option<u64>,
        as_of: Option<u64>,
        export: Option<PathBuf>,
    },
    Staff,
    StaffRecruit {
        name: String,
        trait_id: String,
    },
    StaffAssign {
        id: u64,
        role: RoleKind,
        target: AssignmentTarget,
    },
    StaffDelegate {
        id: u64,
        permission: Permission,
        cap: Option<String>,
    },
    StaffRevoke {
        id: u64,
        permission: Option<Permission>,
    },
    StaffClearance {
        id: u64,
        class: Classification,
        compartments: Vec<String>,
    },
    ArenaRun {
        seed: Option<u64>,
        days: u32,
        actions: u32,
        timeout_ms: u64,
        bots: Vec<PathBuf>,
        board: PathBuf,
        opt_in: bool,
        backend: Option<bool>,
    },
    ArenaBoard {
        board: PathBuf,
        opt_in: bool,
    },
    SelfPlay {
        matrix: String,
        seed: Option<u64>,
        report: Option<PathBuf>,
        opt_in: bool,
        bots: bool,
    },
    DemoGovernance {
        seed: Option<u64>,
        report: Option<PathBuf>,
        opt_in: bool,
    },
    VehicleList,
    VehicleRegister {
        planet: Coord,
        ships: Fleet,
    },
    VehicleDissolve {
        id: u64,
    },
    MissionWindows {
        from: Coord,
        to: Coord,
        transfer: TransferKind,
    },
    MissionPlan {
        to: Coord,
        vehicle: u64,
        transfer: TransferKind,
        customer: Customer,
        metal: String,
        crystal: String,
        deuterium: String,
        window: Option<usize>,
    },
    MissionSchedule,
    MarketRegion {
        region: Option<Coord>,
    },
    MarketOrder {
        region: Coord,
        side: OrderSide,
        good: ResourceKind,
        amount: String,
        limit: Option<String>,
        ttl: Option<u64>,
    },
    MarketAccept {
        id: u64,
    },
    MarketFulfil {
        id: u64,
    },
    MarketShip {
        from: Coord,
        to: Coord,
        metal: String,
        crystal: String,
        deuterium: String,
        contract: Option<u64>,
    },
    MarketHistory {
        region: Option<Coord>,
        export: Option<PathBuf>,
    },
    Species,
    SpeciesUnlock {
        id: String,
    },
    Transmute {
        from: ResourceKind,
        to: ResourceKind,
        amount: String,
    },
    Brew,
    FlagshipRelocate {
        dest: Coord,
    },
    FlagshipAnchor,
    Rivals,
    Frontier,
    BlueprintList,
    BlueprintShow {
        name: String,
    },
    BlueprintCapture {
        planet: Coord,
        name: String,
    },
    BlueprintApply {
        planet: Coord,
        name: String,
    },
    BlueprintExport {
        name: String,
    },
    BlueprintImport {
        path: PathBuf,
    },
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
    arena: bool,
    self_play: bool,
    demo: bool,
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
            || a == "--commentary"
            || a == "--advisor"
            || a == "--no-portraits"
            || a == "--arena"
            || a == "--self-play"
            || a == "--demo"
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
    if let Command::Feedback { note } = &opts.command {
        return run_feedback(opts.save.as_deref(), note);
    }
    if matches!(
        opts.command,
        Command::ArenaRun { .. } | Command::ArenaBoard { .. }
    ) {
        return run_arena_command(opts);
    }
    if matches!(opts.command, Command::SelfPlay { .. }) {
        return run_self_play_command(opts);
    }
    if matches!(opts.command, Command::DemoGovernance { .. }) {
        return run_demo_command(opts);
    }
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
    let mut store = open_storage(&save).map_err(|e| CliError::Internal(e.to_string()))?;
    let mut state = if let Some(seed) = opts.seed {
        store.load_or_new_with_seed(0, config, seed)
    } else {
        store.load_or_new(SystemClock.now_unix(), config)
    }
    .map_err(|e| CliError::Internal(e.to_string()))?;
    let now = now_for(&state);
    let value = apply(&mut state, now, &opts.command, &save)?;
    store
        .save(&state)
        .map_err(|e| CliError::Internal(e.to_string()))?;
    Ok(value)
}

fn strip_controls(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .take(4_000)
        .collect()
}

fn feedback_log_path(save: &Path) -> PathBuf {
    save.with_extension("feedback.log")
}

fn glance_for_feedback(save: &Path) -> Result<Value, SaveError> {
    match BackendKind::from_path(save) {
        BackendKind::Sqlite => {
            let glance = inspect_save(save)?;
            Ok(json!({
                "save_schema": glance.schema,
                "last_tick_unix": glance.last_tick_unix,
                "deterministic": glance.deterministic,
                "planets": glance.planets,
                "fleet_strength": glance.fleet_strength,
            }))
        }
        BackendKind::Mikura | BackendKind::Chisei => {
            let store = open_storage(save)?;
            let loaded = store.load(BalanceConfig::default())?;
            Ok(json!({
                "save_schema": hoshi_core::SCHEMA_VERSION,
                "last_tick_unix": loaded.last_tick_unix,
                "deterministic": loaded.deterministic,
                "planets": 1 + loaded.colonies.len(),
                "fleet_strength": loaded.fleet.total(),
            }))
        }
    }
}

fn run_feedback(save_flag: Option<&Path>, note: &str) -> Result<Value, CliError> {
    let save = resolve_save_path(save_flag, std::env::var_os("HOSHI_SAVE").as_deref());
    let at = SystemClock.now_unix();
    let mut state = json!(null);
    let mut save_locked = false;
    if save.exists() {
        if save_is_locked(&save) {
            save_locked = true;
        } else {
            match glance_for_feedback(&save) {
                Ok(glance) => state = glance,
                Err(SaveError::Locked) => save_locked = true,
                Err(err) => return Err(CliError::Internal(err.to_string())),
            }
        }
    }
    let entry = json!({
        "at": at,
        "version": env!("CARGO_PKG_VERSION"),
        "note": note,
        "save_locked": save_locked,
        "state": state,
    });
    let log = feedback_log_path(&save);
    if let Some(parent) = log.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|e| CliError::Internal(e.to_string()))?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)
        .map_err(|e| CliError::Internal(e.to_string()))?;
    use std::io::Write;
    writeln!(file, "{entry}").map_err(|e| CliError::Internal(e.to_string()))?;
    Ok(json!({
        "schema_version": SCHEMA_VERSION,
        "ok": true,
        "message": "feedback recorded",
        "log": log.file_name().and_then(|n| n.to_str()).unwrap_or("feedback.log"),
        "save_locked": save_locked,
    }))
}

fn now_for(state: &GameState) -> u64 {
    if state.deterministic {
        state.last_tick_unix
    } else {
        SystemClock.now_unix()
    }
}

fn governed(state: &mut GameState, intent: Intent, now: u64) -> Result<(), CliError> {
    let proposal =
        hoshi_core::propose(state, &intent, now).map_err(|e| CliError::Refused(e.to_string()))?;
    hoshi_core::apply(state, &proposal, now).map_err(|e| CliError::Refused(e.to_string()))
}

fn apply(
    state: &mut GameState,
    now: u64,
    command: &Command,
    save: &Path,
) -> Result<Value, CliError> {
    match command {
        Command::State { actor } => {
            if !state.deterministic {
                state.catch_up(now);
            }
            Ok(state_value(state, *actor))
        }
        Command::Classify {
            kind,
            id,
            class,
            compartments,
            actor,
        } => {
            state
                .classify(*actor, *kind, id, *class, compartments.clone())
                .map_err(|e| CliError::Refused(e.to_string()))?;
            Ok(ok_state_as(state, "classified", *actor))
        }
        Command::Share {
            faction,
            class,
            compartments,
        } => {
            state
                .share(faction, *class, compartments.clone())
                .map_err(|e| CliError::Refused(e.to_string()))?;
            Ok(ok_state(state, "sharing updated"))
        }
        Command::Incidents { actor } => {
            if !state.deterministic {
                state.catch_up(now);
            }
            Ok(incidents_value(state, *actor))
        }
        Command::Build {
            planet,
            building,
            actor,
        } => {
            governed(
                state,
                Intent::Build {
                    planet: *planet,
                    building: *building,
                    actor: *actor,
                },
                now,
            )?;
            Ok(ok_state(state, "queued build"))
        }
        Command::Research { tech } => {
            governed(state, Intent::Research { tech: tech.clone() }, now)?;
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
                    governed(
                        state,
                        Intent::Invade {
                            ships: *ships,
                            dest: *to,
                            speed_percent: 100,
                        },
                        now,
                    )?;
                }
                MissionKind::Colonize => {
                    if ships.get(ShipKind::ColonyShip) != 1 || ships.total() != 1 {
                        return Err(CliError::Invalid(
                            "colonize needs exactly colony_ship:1".into(),
                        ));
                    }
                    governed(
                        state,
                        Intent::Colonize {
                            dest: *to,
                            speed_percent: 100,
                        },
                        now,
                    )?;
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
        Command::Goals => {
            if !state.deterministic {
                state.catch_up(now);
            }
            Ok(goals_value(state))
        }
        Command::GoalsKeep => {
            state
                .keep_playing_after_victory()
                .map_err(|e| CliError::Refused(e.to_string()))?;
            Ok(ok_state(state, "kept playing"))
        }
        Command::Automation | Command::AutomationDryRun => {
            if !state.deterministic {
                state.catch_up(now);
            }
            Ok(automation_value(state))
        }
        Command::AutomationToggle { id } => {
            if !state.deterministic {
                state.catch_up(now);
            }
            let on = state
                .toggle_automation_rule(*id)
                .map_err(|e| CliError::Refused(e.to_string()))?;
            Ok(ok_state(state, if on { "rule on" } else { "rule off" }))
        }
        Command::AutomationRemove { id } => {
            if !state.deterministic {
                state.catch_up(now);
            }
            state
                .remove_automation_rule(*id)
                .map_err(|e| CliError::Refused(e.to_string()))?;
            Ok(ok_state(state, "rule removed"))
        }
        Command::AutomationAdd {
            cond,
            planet,
            resource,
            amount,
            action,
        } => {
            if !state.deterministic {
                state.catch_up(now);
            }
            let id = add_automation_from_cli(state, cond, *planet, *resource, amount, action)?;
            Ok(ok_state(state, &format!("rule #{id}")))
        }
        Command::LawPropose {
            template,
            param,
            jurisdiction,
            effective,
        } => {
            let (default_param, kind) = {
                let spec = state
                    .config
                    .laws
                    .get(template)
                    .ok_or_else(|| CliError::Invalid(format!("unknown law {template}")))?;
                (spec.default, spec.jurisdiction)
            };
            let param = match param {
                Some(raw) => raw
                    .parse::<f64>()
                    .map_err(|_| CliError::Invalid(format!("invalid --param {raw}")))?,
                None => default_param,
            };
            let jurisdiction = match jurisdiction.as_deref() {
                None => match kind {
                    JurisdictionKind::Empire => Jurisdiction::Empire,
                    JurisdictionKind::Planet => Jurisdiction::Planet(state.home),
                },
                Some("empire") => Jurisdiction::Empire,
                Some("home") => Jurisdiction::Planet(state.home),
                Some(text) => Jurisdiction::parse(text)
                    .ok_or_else(|| CliError::Invalid(format!("invalid jurisdiction {text}")))?,
            };
            let id = state
                .propose_law(template, param, jurisdiction, *effective, now)
                .map_err(|e| CliError::Refused(e.to_string()))?;
            Ok(ok_state(state, &format!("proposed law #{id}")))
        }
        Command::LawApprove { id } => {
            state
                .approve_law(*id, now)
                .map_err(|e| CliError::Refused(e.to_string()))?;
            Ok(ok_state(state, "approved law"))
        }
        Command::LawRepeal { id, effective } => {
            state
                .repeal_law(*id, *effective, now)
                .map_err(|e| CliError::Refused(e.to_string()))?;
            Ok(ok_state(state, "proposed repeal"))
        }
        Command::MoonBuild { planet, facility } => {
            state
                .build_moon_facility(*planet, *facility, now)
                .map_err(|e| CliError::Refused(e.to_string()))?;
            Ok(ok_state(state, "built moon facility"))
        }
        Command::MoonJump { from, to, ships } => {
            state
                .jump_between(*from, *to, *ships, now)
                .map_err(|e| CliError::Refused(e.to_string()))?;
            Ok(ok_state(state, "jumped"))
        }
        Command::TutorialSkip => {
            state.skip_tutorial();
            Ok(ok_state(state, "tutorial skipped"))
        }
        Command::TutorialReset => {
            state.reset_hints();
            Ok(ok_state(state, "tutorial reset"))
        }
        Command::TacticsSet {
            formation,
            stance,
            retreat,
        } => {
            if let Some(key) = formation {
                state
                    .set_formation(key)
                    .map_err(|e| CliError::Invalid(e.to_string()))?;
            }
            if let Some(key) = stance {
                state
                    .set_stance(key)
                    .map_err(|e| CliError::Invalid(e.to_string()))?;
            }
            if let Some(raw) = retreat {
                let value: f64 = raw
                    .parse()
                    .map_err(|_| CliError::Invalid("retreat needs a number".into()))?;
                state
                    .set_retreat_below(value)
                    .map_err(|e| CliError::Invalid(e.to_string()))?;
            }
            Ok(ok_state(state, "tactics updated"))
        }
        Command::TacticsAction { action } => {
            if state.pending_tactical.is_none() {
                return Err(CliError::Refused("no pending battle".into()));
            }
            state.queue_tactical_action(*action);
            state.progress_tactical(now);
            Ok(ok_state(state, "tactical action"))
        }
        Command::Feedback { .. } => Err(CliError::Internal(
            "feedback is handled before apply".into(),
        )),
        Command::ArenaRun { .. } | Command::ArenaBoard { .. } => {
            Err(CliError::Internal("arena is handled before apply".into()))
        }
        Command::SelfPlay { .. } => Err(CliError::Internal(
            "self-play is handled before apply".into(),
        )),
        Command::DemoGovernance { .. } => {
            Err(CliError::Internal("demo is handled before apply".into()))
        }
        Command::Advise => {
            if !state.deterministic {
                state.catch_up(now);
            }
            let tips = advise(state);
            Ok(json!({
                "schema_version": SCHEMA_VERSION,
                "ok": true,
                "message": "advisor",
                "suggestions": tips
                    .iter()
                    .map(|a| json!({ "id": a.id, "text": a.text }))
                    .collect::<Vec<_>>(),
            }))
        }
        Command::Log {
            query,
            lineage,
            as_of,
            export,
        } => apply_log(state, now, query, *lineage, *as_of, export.as_deref()),
        Command::Staff => {
            if !state.deterministic {
                state.catch_up(now);
            }
            Ok(ok_state(state, "staff"))
        }
        Command::StaffRecruit { name, trait_id } => {
            let id = state
                .recruit_leader(name, trait_id, now)
                .map_err(|e| CliError::Refused(e.to_string()))?;
            Ok(ok_state(state, &format!("recruited leader #{id}")))
        }
        Command::StaffAssign { id, role, target } => {
            state
                .assign_leader(*id, *role, *target, now)
                .map_err(|e| CliError::Refused(e.to_string()))?;
            Ok(ok_state(state, "assigned leader"))
        }
        Command::StaffDelegate {
            id,
            permission,
            cap,
        } => {
            let spend_cap = match cap {
                Some(raw) => {
                    let value = raw
                        .parse::<f64>()
                        .map_err(|_| CliError::Invalid(format!("invalid --cap {raw}")))?;
                    if !value.is_finite() || value < 0.0 {
                        return Err(CliError::Invalid(format!("invalid --cap {raw}")));
                    }
                    Some(value)
                }
                None => None,
            };
            state
                .delegate_leader(*id, *permission, spend_cap, now)
                .map_err(|e| CliError::Refused(e.to_string()))?;
            Ok(ok_state(state, "delegated"))
        }
        Command::VehicleList => Ok(ok_state(state, "vehicles")),
        Command::VehicleRegister { planet, ships } => {
            state
                .register_vehicle(*planet, *ships, now)
                .map_err(|e| CliError::Refused(e.to_string()))?;
            Ok(ok_state(state, "registered vehicle"))
        }
        Command::VehicleDissolve { id } => {
            state
                .dissolve_vehicle(*id, now)
                .map_err(|e| CliError::Refused(e.to_string()))?;
            Ok(ok_state(state, "dissolved vehicle"))
        }
        Command::MissionWindows { from, to, transfer } => {
            if !state.deterministic {
                state.catch_up(now);
            }
            let estimate = state
                .mission_windows(*from, *to, *transfer, state.last_tick_unix)
                .map_err(|e| CliError::Refused(e.to_string()))?;
            Ok(json!({
                "schema_version": SCHEMA_VERSION,
                "ok": true,
                "message": "windows",
                "now": state.last_tick_unix,
                "delta_v": estimate.delta_v,
                "coast_secs": estimate.coast_secs,
                "windows": estimate.windows.iter().enumerate().map(|(i, w)| json!({
                    "index": i,
                    "opens_at": w.opens_at,
                    "closes_at": w.closes_at,
                    "status": if w.closes_at <= state.last_tick_unix {
                        "missed"
                    } else if w.contains(state.last_tick_unix) {
                        "open"
                    } else {
                        "upcoming"
                    },
                })).collect::<Vec<_>>(),
            }))
        }
        Command::MissionPlan {
            to,
            vehicle,
            transfer,
            customer,
            metal,
            crystal,
            deuterium,
            window,
        } => {
            let payload = Payload {
                cargo: hoshi_core::Resources {
                    metal: parse_amount(metal)?,
                    crystal: parse_amount(crystal)?,
                    deuterium: parse_amount(deuterium)?,
                },
            };
            state
                .plan_mission(
                    PlanRequest {
                        vehicle_id: *vehicle,
                        dest: *to,
                        transfer: *transfer,
                        customer: customer.clone(),
                        payload,
                        window_index: *window,
                    },
                    now,
                )
                .map_err(|e| CliError::Refused(e.to_string()))?;
            Ok(ok_state(state, "planned mission"))
        }
        Command::MissionSchedule => {
            if !state.deterministic {
                state.catch_up(now);
            }
            let schedule = state.mission_schedule(state.last_tick_unix);
            Ok(json!({
                "schema_version": SCHEMA_VERSION,
                "ok": true,
                "message": "schedule",
                "now": state.last_tick_unix,
                "vehicles": schedule.vehicles.iter().map(|v| json!({
                    "id": v.id,
                    "name": v.name,
                    "coord": v.coord.to_string(),
                    "free_at": v.free_at,
                    "booked": v.free_at > state.last_tick_unix,
                })).collect::<Vec<_>>(),
                "missions": schedule.missions.iter().map(|m| json!({
                    "id": m.id,
                    "vehicle_id": m.vehicle_id,
                    "origin": m.route.origin.to_string(),
                    "dest": m.route.dest.to_string(),
                    "transfer": m.transfer.as_key(),
                    "customer": m.customer.as_key(),
                    "arrives_at": m.arrives_at,
                    "delta_v": m.delta_v,
                })).collect::<Vec<_>>(),
                "alerts": schedule.alerts.iter().map(|a| json!({
                    "kind": match a.kind {
                        hoshi_core::ScheduleAlertKind::DoubleBook => "double_book",
                        hoshi_core::ScheduleAlertKind::MissedWindow => "missed_window",
                    },
                    "vehicle_id": a.vehicle_id,
                    "detail": a.detail,
                })).collect::<Vec<_>>(),
            }))
        }
        Command::MarketRegion { region } => {
            if !state.deterministic {
                state.catch_up(now);
            }
            Ok(market_region_value(state, *region))
        }
        Command::MarketOrder {
            region,
            side,
            good,
            amount,
            limit,
            ttl,
        } => {
            let amount = parse_amount(amount)?;
            let limit = match limit.as_deref() {
                Some(text) => Some(parse_amount(text)?),
                None => None,
            };
            governed(
                state,
                Intent::PlaceOrder {
                    region: *region,
                    side: *side,
                    good: *good,
                    amount,
                    limit,
                    ttl_secs: ttl.unwrap_or(0),
                },
                now,
            )?;
            Ok(ok_state(state, "placed order"))
        }
        Command::MarketAccept { id } => {
            governed(state, Intent::AcceptContract { id: *id }, now)?;
            Ok(ok_state(state, "accepted contract"))
        }
        Command::MarketFulfil { id } => {
            governed(state, Intent::FulfilContract { id: *id }, now)?;
            Ok(ok_state(state, "fulfilled contract"))
        }
        Command::MarketShip {
            from,
            to,
            metal,
            crystal,
            deuterium,
            contract,
        } => {
            governed(
                state,
                Intent::ShipGoods {
                    origin: *from,
                    dest: *to,
                    cargo: hoshi_core::Resources {
                        metal: parse_amount(metal)?,
                        crystal: parse_amount(crystal)?,
                        deuterium: parse_amount(deuterium)?,
                    },
                    contract: *contract,
                },
                now,
            )?;
            Ok(ok_state(state, "launched shipment"))
        }
        Command::MarketHistory { region, export } => {
            if !state.deterministic {
                state.catch_up(now);
            }
            apply_market_history(state, *region, export.as_deref())
        }
        Command::Species => {
            if !state.deterministic {
                state.catch_up(now);
            }
            Ok(ok_state(state, "species"))
        }
        Command::SpeciesUnlock { id } => {
            state
                .unlock_species(&SpeciesCatalog::default(), id)
                .map_err(|e| CliError::Refused(e.to_string()))?;
            Ok(ok_state(state, "unlocked species"))
        }
        Command::Transmute { from, to, amount } => {
            let amount: f64 = amount
                .parse()
                .map_err(|_| CliError::Invalid("invalid amount".into()))?;
            state
                .transmute(*from, *to, amount)
                .map_err(|e| CliError::Refused(e.to_string()))?;
            Ok(ok_state(state, "transmuted"))
        }
        Command::Brew => {
            state
                .brew_buff(now)
                .map_err(|e| CliError::Refused(e.to_string()))?;
            Ok(ok_state(state, "brewed"))
        }
        Command::FlagshipRelocate { dest } => {
            if !state.deterministic {
                state.catch_up(now);
            }
            state
                .relocate_flagship(*dest, now)
                .map_err(|e| CliError::Refused(e.to_string()))?;
            Ok(ok_state(state, "flagship relocated"))
        }
        Command::Frontier => {
            if !state.deterministic {
                state.catch_up(now);
            }
            Ok(frontier_value(state))
        }
        Command::BlueprintList => {
            let catalog = BlueprintCatalog::open(blueprint_dir(save));
            let names = catalog
                .list()
                .map_err(|e| CliError::Internal(e.to_string()))?;
            Ok(json!({
                "schema_version": SCHEMA_VERSION,
                "ok": true,
                "message": "blueprint",
                "blueprints": names,
            }))
        }
        Command::BlueprintShow { name } => {
            let catalog = BlueprintCatalog::open(blueprint_dir(save));
            let bp = catalog
                .load(name)
                .map_err(|e| CliError::Invalid(e.to_string()))?;
            Ok(blueprint_value(&bp, "blueprint"))
        }
        Command::BlueprintCapture { planet, name } => {
            if !state.deterministic {
                state.catch_up(now);
            }
            let bp = state
                .capture_blueprint(*planet, name)
                .map_err(|e| CliError::Refused(e.to_string()))?;
            BlueprintCatalog::open(blueprint_dir(save))
                .save(&bp)
                .map_err(|e| CliError::Internal(e.to_string()))?;
            Ok(blueprint_value(&bp, "blueprint captured"))
        }
        Command::BlueprintApply { planet, name } => {
            if !state.deterministic {
                state.catch_up(now);
            }
            let catalog = BlueprintCatalog::open(blueprint_dir(save));
            let bp = catalog
                .load(name)
                .map_err(|e| CliError::Invalid(e.to_string()))?;
            let preview = state
                .preview_blueprint(*planet, &bp)
                .map_err(|e| CliError::Refused(e.to_string()))?;
            let applied = state
                .apply_blueprint(*planet, &bp, now)
                .map_err(|e| CliError::Refused(e.to_string()))?;
            Ok(json!({
                "schema_version": SCHEMA_VERSION,
                "ok": true,
                "message": "blueprint applied",
                "name": bp.name,
                "planet": planet.to_string(),
                "planet_type": bp.planet_type.map(|k| k.as_key()),
                "queued": applied.queued.iter().map(|s| json!({
                    "building": s.building.as_key(),
                    "from_level": s.from_level,
                })).collect::<Vec<_>>(),
                "mismatches": applied.mismatches.iter().map(|m| json!({
                    "building": m.building.as_key(),
                    "reason": m.reason,
                })).collect::<Vec<_>>(),
                "type_mismatch": applied.type_mismatch,
                "skipped": applied.skipped,
                "planned": preview.steps.len(),
            }))
        }
        Command::BlueprintExport { name } => {
            let catalog = BlueprintCatalog::open(blueprint_dir(save));
            let bp = catalog
                .load(name)
                .map_err(|e| CliError::Invalid(e.to_string()))?;
            let mut value = blueprint_value(&bp, "blueprint export");
            value["text"] = json!(bp.to_toml());
            Ok(value)
        }
        Command::BlueprintImport { path } => {
            let text =
                std::fs::read_to_string(path).map_err(|e| CliError::Invalid(e.to_string()))?;
            let bp = Blueprint::from_toml(&text).map_err(|e| CliError::Invalid(e.to_string()))?;
            BlueprintCatalog::open(blueprint_dir(save))
                .save(&bp)
                .map_err(|e| CliError::Internal(e.to_string()))?;
            Ok(blueprint_value(&bp, "blueprint imported"))
        }
        Command::Rivals => {
            if !state.deterministic {
                state.catch_up(now);
            }
            Ok(ok_state(state, "rivals"))
        }
        Command::FlagshipAnchor => {
            if !state.deterministic {
                state.catch_up(now);
            }
            state
                .anchor_flagship(now)
                .map_err(|e| CliError::Refused(e.to_string()))?;
            Ok(ok_state(state, "flagship anchored"))
        }
        Command::StaffRevoke { id, permission } => {
            state
                .revoke_delegation(*id, *permission, now)
                .map_err(|e| CliError::Refused(e.to_string()))?;
            Ok(ok_state(state, "revoked delegation"))
        }
        Command::StaffClearance {
            id,
            class,
            compartments,
        } => {
            state
                .grant_clearance(*id, *class, compartments.clone())
                .map_err(|e| CliError::Refused(e.to_string()))?;
            Ok(ok_state(state, "clearance granted"))
        }
        Command::MoonScan { planet } => {
            if !state.deterministic {
                state.catch_up(now);
            }
            let contacts = state
                .phalanx_contacts(*planet, state.last_tick_unix)
                .map_err(|e| CliError::Refused(e.to_string()))?;
            let mut value = ok_state(state, "phalanx");
            value["contacts"] = json!(
                contacts
                    .iter()
                    .map(|c| json!({
                        "id": c.id,
                        "origin": c.origin.to_string(),
                        "dest": c.dest.to_string(),
                        "phase": c.phase,
                        "eta_secs": c.eta_secs,
                        "ships": c.ships,
                    }))
                    .collect::<Vec<_>>()
            );
            Ok(value)
        }
    }
}

fn path_json(path: &hoshi_core::PathProgress) -> Value {
    json!({
        "ready": path.ready,
        "next": path.next_step,
        "ratio": path.ratio,
    })
}

fn frontier_json(state: &GameState) -> Value {
    let glance = state.frontier_glance();
    json!({
        "galaxy": glance.galaxy,
        "sectors": glance.sectors,
        "revealed": glance.revealed,
        "research_open": glance.research_open,
        "wonder_open": glance.wonder_open,
        "bridge_sectors": glance.bridge_sectors,
        "worlds": glance.worlds.iter().map(|world| json!({
            "coord": world.coord.to_string(),
            "sector": world.sector,
            "kind": world.kind.as_key(),
            "size": world.size,
            "rare": world.rare,
            "production_bonus": state.config.frontier.production_bonus,
            "fields_bonus": state.config.frontier.fields_bonus,
        })).collect::<Vec<_>>(),
    })
}

fn frontier_value(state: &GameState) -> Value {
    let mut value = state_value(state, Actor::Player);
    value["message"] = json!("frontier");
    value
}

fn ladder_json(state: &GameState) -> Value {
    let progress = state.ladder_progress();
    json!({
        "current": progress.current,
        "completed": progress.completed.iter().map(|row| json!({
            "id": row.id,
            "at": row.at,
        })).collect::<Vec<_>>(),
        "next": progress.next.as_ref().map(|goal| json!({
            "id": goal.id,
            "name": goal.name,
            "blurb": goal.blurb,
            "ratio": goal.ratio,
            "next_step": goal.next_step,
            "ready": goal.ready,
        })),
    })
}

fn victory_json(state: &GameState) -> Value {
    let progress = state.victory_progress();
    json!({
        "enabled": state.config.victory.enabled,
        "homeworld": state.pirate_homeworld().map(|coord| coord.to_string()),
        "record": state.victory.as_ref().map(|record| json!({
            "path": record.path.as_key(),
            "at": record.at,
            "prestige": record.prestige,
            "elapsed_secs": record.elapsed_secs,
            "kept_playing": record.kept_playing,
        })),
        "paths": {
            "conquest": path_json(&progress.conquest),
            "wonder": path_json(&progress.wonder),
            "diplomacy": path_json(&progress.diplomacy),
        },
    })
}

fn automation_value(state: &hoshi_core::GameState) -> serde_json::Value {
    let catalog = state.automation_catalog();
    let tiers: Vec<serde_json::Value> = catalog
        .tier
        .iter()
        .map(|tier| {
            json!({
                "id": tier.id,
                "name": tier.name,
                "research": tier.research,
                "unlocked": state.automation_tier_unlocked(&tier.id),
            })
        })
        .collect();
    let preview = state.preview_automation();
    let rules: Vec<serde_json::Value> = state
        .automation_rules
        .iter()
        .map(|rule| {
            let pre = preview.iter().find(|p| p.rule_id == rule.id);
            json!({
                "id": rule.id,
                "enabled": rule.enabled,
                "condition": rule.condition.describe(),
                "action": rule.action.describe(),
                "would_fire": pre.map(|p| p.would_fire).unwrap_or(false),
                "detail": pre.map(|p| p.detail.clone()).unwrap_or_default(),
            })
        })
        .collect();
    json!({
        "ok": true,
        "schema_version": SCHEMA_VERSION,
        "tiers": tiers,
        "rules": rules,
        "control_energy": state.automation_control_energy(),
    })
}

fn add_automation_from_cli(
    state: &mut GameState,
    cond: &str,
    planet: Coord,
    resource: ResourceKind,
    amount: &str,
    action: &AutomationActionSpec,
) -> Result<u64, CliError> {
    let kind =
        hoshi_core::RuleConditionKind::parse(cond).map_err(|e| CliError::Invalid(e.to_string()))?;
    let amount = parse_amount(amount)?;
    let condition = hoshi_core::RuleCondition {
        kind,
        planet,
        resource,
        amount,
    };
    let action = match action {
        AutomationActionSpec::QueueBuild { planet, building } => {
            hoshi_core::RuleAction::QueueBuild {
                planet: *planet,
                building: *building,
            }
        }
        AutomationActionSpec::Transfer {
            from,
            to,
            resource,
            amount,
        } => hoshi_core::RuleAction::Transfer {
            from: *from,
            to: *to,
            resource: *resource,
            amount: parse_amount(amount)?,
        },
    };
    state
        .add_automation_rule(condition, action)
        .map_err(|e| CliError::Refused(e.to_string()))
}

fn parse_automation_add(args: &[String]) -> Result<Command, CliError> {
    let usage = "automation add <stock_below|stock_above> <planet> <resource> <amount> queue_build <planet> <building> | transfer <from> <to> <resource> <amount>";
    let cond = nth(args, 2, usage)?.to_string();
    hoshi_core::RuleConditionKind::parse(&cond).map_err(|e| CliError::Invalid(e.to_string()))?;
    let planet = parse_coord(nth(args, 3, usage)?)?;
    let resource = ResourceKind::from_key(nth(args, 4, usage)?)
        .ok_or_else(|| CliError::Invalid("unknown resource".into()))?;
    let amount = nth(args, 5, usage)?.to_string();
    match nth(args, 6, usage)? {
        "queue_build" => {
            let dest = parse_coord(nth(args, 7, usage)?)?;
            let building = BuildingKind::from_key(nth(args, 8, usage)?)
                .ok_or_else(|| CliError::Invalid("unknown building".into()))?;
            expect_end(args, 9)?;
            Ok(Command::AutomationAdd {
                cond,
                planet,
                resource,
                amount,
                action: AutomationActionSpec::QueueBuild {
                    planet: dest,
                    building,
                },
            })
        }
        "transfer" => {
            let from = parse_coord(nth(args, 7, usage)?)?;
            let to = parse_coord(nth(args, 8, usage)?)?;
            let act_resource = ResourceKind::from_key(nth(args, 9, usage)?)
                .ok_or_else(|| CliError::Invalid("unknown resource".into()))?;
            let act_amount = nth(args, 10, usage)?.to_string();
            expect_end(args, 11)?;
            Ok(Command::AutomationAdd {
                cond,
                planet,
                resource,
                amount,
                action: AutomationActionSpec::Transfer {
                    from,
                    to,
                    resource: act_resource,
                    amount: act_amount,
                },
            })
        }
        other => Err(CliError::Invalid(format!(
            "unknown automation action {other}"
        ))),
    }
}

fn goals_value(state: &GameState) -> Value {
    let mut value = state_value(state, Actor::Player);
    value["message"] = json!("goals");
    value
}

fn blueprint_value(blueprint: &Blueprint, message: &str) -> Value {
    json!({
        "schema_version": SCHEMA_VERSION,
        "ok": true,
        "message": message,
        "name": blueprint.name,
        "planet_type": blueprint.planet_type.map(|k| k.as_key()),
        "buildings": blueprint
            .buildings
            .iter()
            .map(|(kind, level)| json!({ "id": kind.as_key(), "level": level }))
            .collect::<Vec<_>>(),
        "text": blueprint.to_toml(),
    })
}

fn ok_state(state: &GameState, message: &str) -> Value {
    ok_state_as(state, message, Actor::Player)
}

fn ok_state_as(state: &GameState, message: &str, actor: Actor) -> Value {
    let mut value = state_value(state, actor);
    value["message"] = json!(message);
    value
}

fn incidents_value(state: &GameState, actor: Actor) -> Value {
    json!({
        "schema_version": SCHEMA_VERSION,
        "ok": true,
        "incidents": state.visible_incidents(actor).iter().map(|row| json!({
            "id": row.id,
            "at": row.at,
            "source": row.source,
            "kind": row.kind.as_key(),
            "object_id": row.object_id,
            "via": row.via,
        })).collect::<Vec<_>>(),
    })
}

fn marking_value(state: &GameState, kind: ClassifiedKind, id: &str) -> Value {
    let marking = state.marking_of(kind, id);
    json!({
        "class": marking.classification.as_key(),
        "compartments": marking.compartments.iter().cloned().collect::<Vec<_>>(),
    })
}

fn state_value(state: &GameState, actor: Actor) -> Value {
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
        .visible_events(actor)
        .into_iter()
        .map(|e| json!({"at": e.at, "text": e.text}))
        .collect();
    let admiral = state
        .admirals
        .iter()
        .find(|a| Some(a.id) == state.home_admiral_id)
        .map(|a| {
            json!({
                "id": a.id,
                "name": a.name,
                "trait": a.trait_id,
                "xp": a.xp,
            })
        });
    let tactics = json!({
        "formation": state.fleet_tactics.formation.as_key(),
        "stance": state.fleet_tactics.stance.as_key(),
        "retreat_below": state.fleet_tactics.retreat_below,
        "admiral": admiral,
        "pending": state.pending_tactical.is_some(),
    });
    let leaders: Vec<Value> = state
        .leaders
        .iter()
        .map(|leader| {
            let assignment = state.assignment_of(leader.id);
            json!({
                "id": leader.id,
                "name": leader.name,
                "species": leader.species,
                "trait": leader.trait_id,
                "xp": leader.xp,
                "skills": {
                    "command": leader.skill_command,
                    "logistics": leader.skill_logistics,
                    "policy": leader.skill_policy,
                },
                "born_at": leader.born_at,
                "retires_at": leader.retires_at(),
                "retired": leader.retired,
                "role": assignment.map(|row| row.role.as_key()),
                "target": assignment.map(|row| row.target.as_key()),
                "ready_at": assignment.map(|row| row.ready_at),
                "delegations": state.delegations.iter().filter(|row| row.leader_id == leader.id).map(|row| {
                    json!({
                        "permission": row.permission.as_key(),
                        "spend_cap": row.spend_cap,
                        "spent": row.spent,
                    })
                }).collect::<Vec<_>>(),
            })
        })
        .collect();
    let laws: Vec<Value> = state
        .visible_laws(actor)
        .into_iter()
        .map(|law| {
            json!({
                "id": law.id,
                "template": law.template_id,
                "param": law.param,
                "owner": law.owner,
                "jurisdiction": law.jurisdiction.as_key(),
                "status": law.status.as_key(),
                "effective_at": law.effective_at,
                "approvals": law.approvals.iter().map(|a| json!({
                    "who": a.who,
                    "at": a.at,
                })).collect::<Vec<_>>(),
                "samples": state.laws.latest_samples(law.id).iter().map(|s| json!({
                    "metric": s.metric.as_key(),
                    "expected": s.expected,
                    "actual": s.actual,
                    "at": s.at,
                })).collect::<Vec<_>>(),
                "marking": marking_value(state, ClassifiedKind::Law, &law.id.to_string()),
            })
        })
        .collect();
    let waste = state.waste_home();
    let mut value = json!({
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
        "waste": {
            "heat_load": waste.heat_load,
            "heat_sink": waste.heat_sink,
            "pollution_load": waste.pollution_load,
            "pollution_sink": waste.pollution_sink,
            "unrest": state.unrest_factor(),
            "warn": waste.warns(state.config.waste.warn_ratio),
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
            "exclusive_picks": state.exclusive_picks,
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
            "report": state.latest_report_for(actor, o.coord).map(|r| spy_report_value(r, state.last_tick_unix, state)),
        })).collect::<Vec<_>>(),
        "spy_reports": state.visible_reports(actor).into_iter().map(|r| spy_report_value(r, state.last_tick_unix, state)).collect::<Vec<_>>(),
        "incidents": state.visible_incidents(actor).into_iter().map(|row| json!({
            "id": row.id,
            "at": row.at,
            "source": row.source,
            "kind": row.kind.as_key(),
            "object_id": row.object_id,
            "via": row.via,
        })).collect::<Vec<_>>(),
        "events": events,
        "raid": raid,
        "prestige": {
            "currency": state.prestige.currency,
            "lifetime": state.prestige.lifetime,
            "run": state.prestige.current.index,
            "score": state.prestige_score(),
            "eligible": state.can_prestige(),
            "owned": state.prestige.owned,
            "unlocked_species": state.prestige.unlocked_species.iter().cloned().collect::<Vec<_>>(),
        },
        "achievements": state.achievements.iter().map(|(id, at)| {
            json!({"id": id, "unlocked_at": at})
        }).collect::<Vec<_>>(),
        "daily": {
            "day_index": state.daily.day_index,
            "streak": state.daily.streak,
            "resets_in_secs": state.daily_resets_in(),
            "slots": state.daily.slots.iter().map(|slot| {
                json!({
                    "id": slot.spec_id,
                    "name": state.config.daily.get(&slot.spec_id).map(|s| s.name.clone()).unwrap_or_default(),
                    "progress": slot.progress,
                    "target": slot.target,
                    "done": slot.done(),
                })
            }).collect::<Vec<_>>(),
        },
        "tutorial": {
            "enabled": state.tutorial.enabled,
            "skipped": state.tutorial.skipped,
            "pending": state.pending_hint().map(|h| h.id),
        },
        "tactics": tactics,
        "moons": state.moons.iter().map(|moon| {
            json!({
                "parent": moon.parent.to_string(),
                "phalanx": moon.phalanx,
                "jump_gate": moon.jump_gate,
                "jump_ready_at": moon.jump_ready_at,
            })
        }).collect::<Vec<_>>(),
        "laws": laws,
        "leaders": leaders,
        "vehicles": state.vehicles.iter().map(|v| json!({
            "id": v.id,
            "name": v.name,
            "coord": v.coord.to_string(),
            "free_at": v.free_at,
        })).collect::<Vec<_>>(),
        "planned": state.visible_plans(actor).into_iter().map(|m| json!({
            "id": m.id,
            "vehicle_id": m.vehicle_id,
            "origin": m.route.origin.to_string(),
            "dest": m.route.dest.to_string(),
            "transfer": m.transfer.as_key(),
            "customer": m.customer.as_key(),
            "arrives_at": m.arrives_at,
            "delta_v": m.delta_v,
            "marking": marking_value(state, ClassifiedKind::Plan, &m.id.to_string()),
        })).collect::<Vec<_>>(),
    });
    value["victory"] = victory_json(state);
    value["ladder"] = ladder_json(state);
    value["frontier"] = frontier_json(state);
    value["species"] = match &state.species {
        Some(run) => json!({
            "id": run.id,
            "name": run.name,
            "brew_until": run.brew_until,
            "flagship": run.flagship.as_ref().map(|ship| json!({
                "coord": ship.coord.to_string(),
                "arrives_at": ship.arrives_at,
                "anchored": ship.anchored,
            })),
        }),
        None => Value::Null,
    };
    value["rivals"] = json!({
        "enabled": state.rivals.enabled,
        "empires": state.rival_intel().into_iter().map(|row| json!({
            "id": row.id,
            "personality": row.personality,
            "name": row.name,
            "home": row.home.to_string(),
            "last_action": row.last_action,
            "resources": row.resources.map(|r| json!({
                "metal": r.metal,
                "crystal": r.crystal,
                "deuterium": r.deuterium,
            })),
            "fleet": row.fleet.map(fleet_counts),
            "buildings": row.buildings.map(|b| json!({
                "metal_mine": b.metal_mine,
                "crystal_mine": b.crystal_mine,
                "solar_plant": b.solar_plant,
                "shipyard": b.shipyard,
            })),
        })).collect::<Vec<_>>(),
    });
    value["season"] = match &state.season {
        Some(run) => json!({
            "id": run.ruleset_id,
            "window_start": run.window_start,
            "window_end": run.window_end,
            "seed": run.seed,
            "prestige": run.prestige,
        }),
        None => Value::Null,
    };
    value
}

fn spy_report_value(report: &SpyReport, now: u64, state: &GameState) -> Value {
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
        "formation": report.formation,
        "stance": report.stance,
        "marking": marking_value(state, ClassifiedKind::Report, &report.id.to_string()),
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

fn parse_blueprint_command(args: &[String]) -> Result<Command, CliError> {
    match args.get(1).map(String::as_str) {
        None | Some("list") => {
            expect_end(args, if args.get(1).is_some() { 2 } else { 1 })?;
            Ok(Command::BlueprintList)
        }
        Some("show") => {
            let name = nth(args, 2, "blueprint show <name>")?.to_string();
            expect_end(args, 3)?;
            Ok(Command::BlueprintShow { name })
        }
        Some("capture") => {
            let planet = parse_coord(nth(args, 2, "blueprint capture <planet> <name>")?)?;
            let name = nth(args, 3, "blueprint capture <planet> <name>")?.to_string();
            expect_end(args, 4)?;
            Ok(Command::BlueprintCapture { planet, name })
        }
        Some("apply") => {
            let name = nth(args, 2, "blueprint apply <name> <planet>")?.to_string();
            let planet = parse_coord(nth(args, 3, "blueprint apply <name> <planet>")?)?;
            expect_end(args, 4)?;
            Ok(Command::BlueprintApply { planet, name })
        }
        Some("export") => {
            let name = nth(args, 2, "blueprint export <name>")?.to_string();
            expect_end(args, 3)?;
            Ok(Command::BlueprintExport { name })
        }
        Some("import") => {
            let path = PathBuf::from(nth(args, 2, "blueprint import <path>")?);
            expect_end(args, 3)?;
            Ok(Command::BlueprintImport { path })
        }
        Some(other) => Err(CliError::Invalid(format!(
            "unknown blueprint command {other}"
        ))),
    }
}

fn parse_cli(args: &[String]) -> Result<CliOpts, CliError> {
    let mut save = None;
    let mut seed = None;
    let mut balance = None;
    let mut arena = false;
    let mut self_play = false;
    let mut demo = false;
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
        if a == "--json" || a == "--arena" || a == "--self-play" || a == "--demo" {
            if a == "--arena" {
                arena = true;
            }
            if a == "--self-play" {
                self_play = true;
            }
            if a == "--demo" {
                demo = true;
            }
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
        arena,
        self_play,
        demo,
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
        "state" => parse_actor_command(args, |actor| Command::State { actor }),
        "classify" => parse_classify_command(args),
        "share" => parse_share_command(args),
        "incidents" => parse_actor_command(args, |actor| Command::Incidents { actor }),
        "advise" => {
            expect_end(args, 1)?;
            Ok(Command::Advise)
        }
        "log" => parse_log_command(args),
        "arena" => parse_arena_command(args),
        "self-play" => parse_self_play_command(args),
        "demo" => parse_demo_command(args),
        "build" => {
            let planet = parse_coord(nth(args, 1, "build <planet> <building>")?)?;
            let building = BuildingKind::from_key(nth(args, 2, "build <planet> <building>")?)
                .ok_or_else(|| CliError::Invalid("unknown building".into()))?;
            let mut actor = Actor::Player;
            let mut i = 3;
            while i < args.len() {
                let a = args[i].as_str();
                if a == "--as" {
                    let raw = nth(args, i + 1, "build --as <actor>")?;
                    actor = Actor::parse(raw)
                        .ok_or_else(|| CliError::Invalid(format!("invalid --as {raw}")))?;
                    i += 2;
                    continue;
                }
                return Err(CliError::Invalid(format!("unexpected argument {a}")));
            }
            Ok(Command::Build {
                planet,
                building,
                actor,
            })
        }
        "staff" => parse_staff_command(args),
        "vehicle" => parse_vehicle_command(args),
        "mission" => parse_mission_command(args),
        "market" => parse_market_command(args),
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
        "automation" => match args.get(1).map(String::as_str) {
            None => {
                expect_end(args, 1)?;
                Ok(Command::Automation)
            }
            Some("dry-run") => {
                expect_end(args, 2)?;
                Ok(Command::AutomationDryRun)
            }
            Some("toggle") => {
                let id = parse_law_id(nth(args, 2, "automation toggle <id>")?)?;
                expect_end(args, 3)?;
                Ok(Command::AutomationToggle { id })
            }
            Some("remove") => {
                let id = parse_law_id(nth(args, 2, "automation remove <id>")?)?;
                expect_end(args, 3)?;
                Ok(Command::AutomationRemove { id })
            }
            Some("add") => parse_automation_add(args),
            Some(other) => Err(CliError::Invalid(format!(
                "unknown automation command {other}"
            ))),
        },
        "goals" => match args.get(1).map(String::as_str) {
            None => {
                expect_end(args, 1)?;
                Ok(Command::Goals)
            }
            Some("keep") => {
                expect_end(args, 2)?;
                Ok(Command::GoalsKeep)
            }
            Some(other) => Err(CliError::Invalid(format!("unknown goals command {other}"))),
        },
        "rivals" => {
            expect_end(args, 1)?;
            Ok(Command::Rivals)
        }
        "frontier" => {
            expect_end(args, 1)?;
            Ok(Command::Frontier)
        }
        "blueprint" => parse_blueprint_command(args),
        "species" => match args.get(1).map(String::as_str) {
            None => {
                expect_end(args, 1)?;
                Ok(Command::Species)
            }
            Some("unlock") => {
                let id = nth(args, 2, "species unlock <id>")?.to_string();
                expect_end(args, 3)?;
                Ok(Command::SpeciesUnlock { id })
            }
            Some(other) => Err(CliError::Invalid(format!(
                "unknown species command {other}"
            ))),
        },
        "transmute" => {
            let from = ResourceKind::from_key(nth(args, 1, "transmute <from> <to> <amount>")?)
                .ok_or_else(|| CliError::Invalid("unknown resource".into()))?;
            let to = ResourceKind::from_key(nth(args, 2, "transmute <from> <to> <amount>")?)
                .ok_or_else(|| CliError::Invalid("unknown resource".into()))?;
            let amount = nth(args, 3, "transmute <from> <to> <amount>")?.to_string();
            expect_end(args, 4)?;
            Ok(Command::Transmute { from, to, amount })
        }
        "brew" => {
            expect_end(args, 1)?;
            Ok(Command::Brew)
        }
        "flagship" => match args.get(1).map(String::as_str) {
            Some("relocate") => {
                let dest = parse_coord(nth(args, 2, "flagship relocate <coord>")?)?;
                expect_end(args, 3)?;
                Ok(Command::FlagshipRelocate { dest })
            }
            Some("anchor") => {
                expect_end(args, 2)?;
                Ok(Command::FlagshipAnchor)
            }
            Some(other) => Err(CliError::Invalid(format!(
                "unknown flagship command {other}"
            ))),
            None => Err(CliError::Invalid("flagship relocate <coord>|anchor".into())),
        },
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
        "law" => parse_law_command(args),
        "moon" => parse_moon_command(args),
        "feedback" => {
            let note = args.get(1..).unwrap_or(&[]).join(" ");
            Ok(Command::Feedback {
                note: strip_controls(&note),
            })
        }
        "tactics" => parse_tactics_command(args),
        "tutorial" => match args.get(1).map(String::as_str) {
            Some("skip") => {
                expect_end(args, 2)?;
                Ok(Command::TutorialSkip)
            }
            Some("reset") => {
                expect_end(args, 2)?;
                Ok(Command::TutorialReset)
            }
            Some(other) => Err(CliError::Invalid(format!(
                "unknown tutorial command {other}"
            ))),
            None => Err(CliError::Invalid("tutorial needs skip or reset".into())),
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

fn run_arena_command(opts: CliOpts) -> Result<Value, CliError> {
    let save = resolve_save_path(
        opts.save.as_deref(),
        std::env::var_os("HOSHI_SAVE").as_deref(),
    );
    let cfg_dir = match save.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let cfg = crate::config::UserConfig::load(cfg_dir);
    let cfg_arena = cfg.arena;
    let arena_flag = opts.arena;
    let global_seed = opts.seed;
    let enabled = |opt_in: bool| arena_flag || opt_in || cfg_arena;
    match opts.command {
        Command::ArenaRun {
            seed,
            days,
            actions,
            timeout_ms,
            bots,
            board,
            opt_in,
            backend,
        } => {
            if !enabled(opt_in) {
                return Err(CliError::Refused(
                    "arena is off; set arena = true in config.toml or pass --arena".into(),
                ));
            }
            let seed = seed
                .or(global_seed)
                .ok_or_else(|| CliError::Invalid("arena run needs --seed".into()))?;
            let board = if board.is_absolute() {
                board
            } else {
                cfg_dir.join(board)
            };
            crate::arena::run(&crate::arena::ArenaSpec {
                seed,
                days,
                actions,
                timeout: Duration::from_millis(timeout_ms.max(1)),
                bots,
                board,
                chisei: backend.unwrap_or(cfg.chisei),
            })
            .map_err(CliError::Internal)
        }
        Command::ArenaBoard { board, opt_in } => {
            if !enabled(opt_in) {
                return Err(CliError::Refused(
                    "arena is off; set arena = true in config.toml or pass --arena".into(),
                ));
            }
            let board = if board.is_absolute() {
                board
            } else {
                cfg_dir.join(board)
            };
            crate::arena::board_markdown(&board).map_err(CliError::Internal)
        }
        _ => Err(CliError::Internal("arena is handled before apply".into())),
    }
}

fn run_demo_command(opts: CliOpts) -> Result<Value, CliError> {
    let save = resolve_save_path(
        opts.save.as_deref(),
        std::env::var_os("HOSHI_SAVE").as_deref(),
    );
    let cfg_dir = match save.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let cfg = crate::config::UserConfig::load(cfg_dir);
    let Command::DemoGovernance {
        seed,
        report,
        opt_in,
    } = opts.command
    else {
        return Err(CliError::Internal("demo is handled before apply".into()));
    };
    if !(opts.demo || opt_in || cfg.demo) {
        return Err(CliError::Refused(
            "demo is off; set demo = true in config.toml or pass --demo".into(),
        ));
    }
    let seed = seed
        .or(opts.seed)
        .ok_or_else(|| CliError::Invalid("demo governance needs --seed".into()))?;
    crate::demo::run(&crate::demo::DemoSpec {
        seed,
        report,
        model_url: String::new(),
        allow_lan: cfg.advisor_allow_lan,
    })
    .map_err(CliError::Internal)
}

fn parse_demo_command(args: &[String]) -> Result<Command, CliError> {
    if nth(args, 1, "demo governance")? != "governance" {
        return Err(CliError::Invalid("expected demo governance".into()));
    }
    let mut seed = None;
    let mut report = None;
    let mut opt_in = false;
    let mut i = 2;
    while i < args.len() {
        let a = args[i].as_str();
        if a == "--demo" {
            opt_in = true;
            i += 1;
            continue;
        }
        if let Some(v) = a.strip_prefix("--seed=") {
            seed = Some(parse_seed(v)?);
            i += 1;
            continue;
        }
        if a == "--seed" {
            seed = Some(parse_seed(nth(args, i + 1, "demo governance --seed")?)?);
            i += 2;
            continue;
        }
        if let Some(v) = a.strip_prefix("--report=") {
            report = Some(PathBuf::from(v));
            i += 1;
            continue;
        }
        if a == "--report" {
            report = Some(PathBuf::from(nth(
                args,
                i + 1,
                "demo governance --report <path>",
            )?));
            i += 2;
            continue;
        }
        return Err(CliError::Invalid(format!("unknown demo flag {a}")));
    }
    Ok(Command::DemoGovernance {
        seed,
        report,
        opt_in,
    })
}

fn parse_arena_backend(value: &str) -> Result<bool, CliError> {
    match value {
        "chisei" => Ok(true),
        "sqlite" => Ok(false),
        other => Err(CliError::Invalid(format!(
            "arena --backend must be sqlite or chisei, not {other}"
        ))),
    }
}

fn parse_arena_command(args: &[String]) -> Result<Command, CliError> {
    match args.get(1).map(String::as_str) {
        Some("run") => {
            let mut seed = None;
            let mut days = None;
            let mut actions = 32u32;
            let mut timeout_ms = 2_000u64;
            let mut board = PathBuf::from("arena-board.json");
            let mut bots = Vec::new();
            let mut opt_in = false;
            let mut backend = None;
            let mut i = 2;
            while i < args.len() {
                let a = args[i].as_str();
                if a == "--arena" {
                    opt_in = true;
                    i += 1;
                    continue;
                }
                if let Some(v) = a.strip_prefix("--seed=") {
                    seed = Some(parse_seed(v)?);
                    i += 1;
                    continue;
                }
                if a == "--seed" {
                    seed = Some(parse_seed(nth(args, i + 1, "arena run --seed <n>")?)?);
                    i += 2;
                    continue;
                }
                if let Some(v) = a.strip_prefix("--days=") {
                    days = Some(parse_positive_u32(v, "--days")?);
                    i += 1;
                    continue;
                }
                if a == "--days" {
                    days = Some(parse_positive_u32(
                        nth(args, i + 1, "arena run --days <n>")?,
                        "--days",
                    )?);
                    i += 2;
                    continue;
                }
                if let Some(v) = a.strip_prefix("--actions=") {
                    actions = parse_u32_flag(v, "--actions")?;
                    i += 1;
                    continue;
                }
                if a == "--actions" {
                    actions =
                        parse_u32_flag(nth(args, i + 1, "arena run --actions <n>")?, "--actions")?;
                    i += 2;
                    continue;
                }
                if let Some(v) = a.strip_prefix("--timeout-ms=") {
                    timeout_ms = parse_u64_flag(v, "--timeout-ms")?;
                    i += 1;
                    continue;
                }
                if a == "--timeout-ms" {
                    timeout_ms = parse_u64_flag(
                        nth(args, i + 1, "arena run --timeout-ms <n>")?,
                        "--timeout-ms",
                    )?;
                    i += 2;
                    continue;
                }
                if let Some(v) = a.strip_prefix("--board=") {
                    board = PathBuf::from(v);
                    i += 1;
                    continue;
                }
                if a == "--board" {
                    board = PathBuf::from(nth(args, i + 1, "arena run --board <path>")?);
                    i += 2;
                    continue;
                }
                if let Some(v) = a.strip_prefix("--backend=") {
                    backend = Some(parse_arena_backend(v)?);
                    i += 1;
                    continue;
                }
                if a == "--backend" {
                    backend = Some(parse_arena_backend(nth(
                        args,
                        i + 1,
                        "arena run --backend sqlite|chisei",
                    )?)?);
                    i += 2;
                    continue;
                }
                if a.starts_with('-') {
                    return Err(CliError::Invalid(format!("unknown arena run flag {a}")));
                }
                bots.push(PathBuf::from(a));
                i += 1;
            }
            let days = days.ok_or_else(|| CliError::Invalid("arena run needs --days".into()))?;
            if bots.is_empty() {
                return Err(CliError::Invalid("arena run needs at least one bot".into()));
            }
            Ok(Command::ArenaRun {
                seed,
                days,
                actions,
                timeout_ms,
                bots,
                board,
                opt_in,
                backend,
            })
        }
        Some("board") => {
            let mut board = PathBuf::from("arena-board.json");
            let mut opt_in = false;
            let mut i = 2;
            while i < args.len() {
                let a = args[i].as_str();
                if a == "--arena" {
                    opt_in = true;
                    i += 1;
                    continue;
                }
                if let Some(v) = a.strip_prefix("--board=") {
                    board = PathBuf::from(v);
                    i += 1;
                    continue;
                }
                if a == "--board" {
                    board = PathBuf::from(nth(args, i + 1, "arena board --board <path>")?);
                    i += 2;
                    continue;
                }
                return Err(CliError::Invalid(format!("unknown arena board flag {a}")));
            }
            Ok(Command::ArenaBoard { board, opt_in })
        }
        Some(other) => Err(CliError::Invalid(format!("unknown arena command {other}"))),
        None => Err(CliError::Invalid("arena needs run or board".into())),
    }
}

fn parse_self_play_command(args: &[String]) -> Result<Command, CliError> {
    let mut matrix = "pr".to_string();
    let mut seed = None;
    let mut report = None;
    let mut opt_in = false;
    let mut bots = false;
    let mut i = 1;
    while i < args.len() {
        let a = args[i].as_str();
        if a == "--self-play" {
            opt_in = true;
            i += 1;
            continue;
        }
        if a == "--bots" {
            bots = true;
            i += 1;
            continue;
        }
        if let Some(v) = a.strip_prefix("--matrix=") {
            matrix = v.to_string();
            i += 1;
            continue;
        }
        if a == "--matrix" {
            matrix = nth(args, i + 1, "self-play --matrix pr|nightly")?.to_string();
            i += 2;
            continue;
        }
        if let Some(v) = a.strip_prefix("--seed=") {
            seed = Some(parse_seed(v)?);
            i += 1;
            continue;
        }
        if a == "--seed" {
            seed = Some(parse_seed(nth(args, i + 1, "self-play --seed <n>")?)?);
            i += 2;
            continue;
        }
        if let Some(path) = a.strip_prefix("--report=") {
            report = Some(PathBuf::from(path));
            i += 1;
            continue;
        }
        if a == "--report" {
            report = Some(PathBuf::from(nth(
                args,
                i + 1,
                "self-play --report <path>",
            )?));
            i += 2;
            continue;
        }
        return Err(CliError::Invalid(format!("unknown self-play flag {a}")));
    }
    if matrix != "pr" && matrix != "nightly" {
        return Err(CliError::Invalid(
            "self-play --matrix must be pr or nightly".into(),
        ));
    }
    Ok(Command::SelfPlay {
        matrix,
        seed,
        report,
        opt_in,
        bots,
    })
}

fn run_self_play_command(opts: CliOpts) -> Result<Value, CliError> {
    let save = resolve_save_path(
        opts.save.as_deref(),
        std::env::var_os("HOSHI_SAVE").as_deref(),
    );
    let cfg_dir = match save.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let cfg = crate::config::UserConfig::load(cfg_dir);
    let Command::SelfPlay {
        matrix,
        seed,
        report,
        opt_in,
        bots,
    } = opts.command
    else {
        return Err(CliError::Internal(
            "self-play is handled before apply".into(),
        ));
    };
    if !(opts.self_play || opt_in || cfg.self_play) {
        return Err(CliError::Refused(
            "self-play is off; set self_play = true in config.toml or pass --self-play".into(),
        ));
    }
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
    let seeds: Vec<u64> = if let Some(seed) = seed.or(opts.seed) {
        vec![seed]
    } else if matrix == "nightly" {
        config.self_play.nightly_seeds.clone()
    } else {
        config.self_play.pr_seeds.clone()
    };
    let result = hoshi_core::run_self_play(config, &seeds);
    let markdown = result.to_markdown();
    if let Some(path) = report {
        std::fs::write(&path, &markdown).map_err(|e| CliError::Internal(e.to_string()))?;
    }
    if bots {
        run_self_play_bots(&seeds)?;
    }
    if !result.passed() {
        return Err(CliError::Refused(markdown));
    }
    Ok(json!({
        "schema_version": SCHEMA_VERSION,
        "ok": true,
        "message": "self-play",
        "seeds": result.seeds,
        "passed": true,
        "markdown": markdown,
    }))
}

fn run_self_play_bots(seeds: &[u64]) -> Result<(), CliError> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let bots: Vec<PathBuf> = hoshi_core::path_bot_relpaths()
        .into_iter()
        .map(|rel| root.join(rel))
        .collect();
    for bot in &bots {
        if !bot.is_file() {
            return Err(CliError::Invalid(format!(
                "missing path bot {}",
                bot.display()
            )));
        }
    }
    let seed = seeds.first().copied().unwrap_or(1);
    let dir = std::env::temp_dir().join(format!(
        "hoshi-self-play-bots-{}-{}",
        std::process::id(),
        seed
    ));
    std::fs::create_dir_all(&dir).map_err(|e| CliError::Internal(e.to_string()))?;
    let board = dir.join("board.json");
    let payload = crate::arena::run(&crate::arena::ArenaSpec {
        seed,
        days: 1,
        actions: 8,
        timeout: Duration::from_millis(2_000),
        bots,
        board,
        chisei: false,
    })
    .map_err(CliError::Internal)?;
    let _ = std::fs::remove_dir_all(&dir);
    let failed: Vec<String> = payload
        .get("bots")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|row| row.get("status").and_then(Value::as_str) != Some("ok"))
        .map(|row| {
            format!(
                "{}:{}",
                row.get("name").and_then(Value::as_str).unwrap_or("bot"),
                row.get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("failed")
            )
        })
        .collect();
    if !failed.is_empty() {
        return Err(CliError::Internal(format!(
            "self-play bots failed: {}",
            failed.join(", ")
        )));
    }
    Ok(())
}

fn parse_positive_u32(text: &str, flag: &str) -> Result<u32, CliError> {
    let n = parse_u32_flag(text, flag)?;
    if n == 0 {
        return Err(CliError::Invalid(format!("{flag} must be at least 1")));
    }
    Ok(n)
}

fn parse_u32_flag(text: &str, flag: &str) -> Result<u32, CliError> {
    text.parse()
        .map_err(|_| CliError::Invalid(format!("invalid {flag} {text}")))
}

fn parse_u64_flag(text: &str, flag: &str) -> Result<u64, CliError> {
    text.parse()
        .map_err(|_| CliError::Invalid(format!("invalid {flag} {text}")))
}

fn parse_log_command(args: &[String]) -> Result<Command, CliError> {
    let mut query = HistoryQuery::default();
    let mut lineage = None;
    let mut as_of = None;
    let mut export = None;
    let mut i = 1;
    while i < args.len() {
        let a = args[i].as_str();
        if let Some(v) = a.strip_prefix("--type=") {
            query.kind = Some(
                HistoryKind::from_key(v)
                    .ok_or_else(|| CliError::Invalid("unknown log type".into()))?,
            );
            i += 1;
            continue;
        }
        if a == "--type" {
            let v = nth(args, i + 1, "log --type <kind>")?;
            query.kind = Some(
                HistoryKind::from_key(v)
                    .ok_or_else(|| CliError::Invalid("unknown log type".into()))?,
            );
            i += 2;
            continue;
        }
        if let Some(v) = a.strip_prefix("--actor=") {
            query.actor = Some(v.to_string());
            i += 1;
            continue;
        }
        if a == "--actor" {
            query.actor = Some(nth(args, i + 1, "log --actor <id>")?.to_string());
            i += 2;
            continue;
        }
        if let Some(v) = a.strip_prefix("--object=") {
            query.object = Some(v.to_string());
            i += 1;
            continue;
        }
        if a == "--object" {
            query.object = Some(nth(args, i + 1, "log --object <id>")?.to_string());
            i += 2;
            continue;
        }
        if let Some(v) = a.strip_prefix("--from=") {
            query.from = Some(parse_unix(v)?);
            i += 1;
            continue;
        }
        if a == "--from" {
            query.from = Some(parse_unix(nth(args, i + 1, "log --from <unix>")?)?);
            i += 2;
            continue;
        }
        if let Some(v) = a.strip_prefix("--to=") {
            query.to = Some(parse_unix(v)?);
            i += 1;
            continue;
        }
        if a == "--to" {
            query.to = Some(parse_unix(nth(args, i + 1, "log --to <unix>")?)?);
            i += 2;
            continue;
        }
        if let Some(v) = a.strip_prefix("--lineage=") {
            lineage = Some(parse_unix(v)?);
            i += 1;
            continue;
        }
        if a == "--lineage" {
            lineage = Some(parse_unix(nth(args, i + 1, "log --lineage <id>")?)?);
            i += 2;
            continue;
        }
        if let Some(v) = a.strip_prefix("--at=") {
            as_of = Some(parse_unix(v)?);
            i += 1;
            continue;
        }
        if a == "--at" {
            as_of = Some(parse_unix(nth(args, i + 1, "log --at <unix>")?)?);
            i += 2;
            continue;
        }
        if let Some(v) = a.strip_prefix("--export=") {
            export = Some(PathBuf::from(v));
            i += 1;
            continue;
        }
        if a == "--export" {
            export = Some(PathBuf::from(nth(args, i + 1, "log --export <path>")?));
            i += 2;
            continue;
        }
        return Err(CliError::Invalid(format!("unknown log flag {a}")));
    }
    Ok(Command::Log {
        query,
        lineage,
        as_of,
        export,
    })
}

fn parse_unix(v: &str) -> Result<u64, CliError> {
    v.parse()
        .map_err(|_| CliError::Invalid(format!("invalid unix time {v}")))
}

fn apply_log(
    state: &mut GameState,
    now: u64,
    query: &HistoryQuery,
    lineage: Option<u64>,
    as_of: Option<u64>,
    export: Option<&Path>,
) -> Result<Value, CliError> {
    if !state.deterministic {
        state.catch_up(now);
    }
    let events: Vec<Value> = if let Some(id) = lineage {
        state
            .history
            .lineage(id)
            .into_iter()
            .map(history_event_value)
            .collect()
    } else {
        state
            .history
            .search(query)
            .into_iter()
            .map(history_event_value)
            .collect()
    };
    let snapshot = as_of.and_then(|at| state.history.as_of(at)).map(|snap| {
        json!({
            "at": snap.at,
            "event_id": snap.event_id,
            "metal": snap.metal,
            "crystal": snap.crystal,
            "deuterium": snap.deuterium,
            "energy": snap.energy,
            "heat": snap.heat,
            "home_kind": snap.home_kind,
            "building_levels": snap.building_levels,
        })
    });
    if let Some(path) = export {
        let mut buf = Vec::new();
        state
            .history
            .export_jsonl(&mut buf)
            .map_err(|e| CliError::Internal(e.to_string()))?;
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|e| CliError::Internal(e.to_string()))?;
        }
        std::fs::write(path, buf).map_err(|e| CliError::Internal(e.to_string()))?;
    }
    Ok(json!({
        "schema_version": SCHEMA_VERSION,
        "ok": true,
        "message": "log",
        "now": state.last_tick_unix,
        "events": events,
        "snapshot": snapshot,
        "exported": export.map(|p| p.display().to_string()),
    }))
}

fn history_event_value(event: &hoshi_core::HistoryEvent) -> Value {
    json!({
        "id": event.id,
        "at": event.at,
        "kind": event.kind.as_key(),
        "actor": event.actor,
        "action": event.action,
        "objects": event.objects,
        "inputs": event.inputs,
        "outcome": event.outcome,
        "reason": event.reason,
        "cause_key": event.cause_key,
        "parent_ids": event.parent_ids,
        "energy": event.energy,
        "heat": event.heat,
        "mass": event.mass,
    })
}

fn parse_tactics_command(args: &[String]) -> Result<Command, CliError> {
    match args.get(1).map(String::as_str) {
        Some("set") => {
            let mut formation = None;
            let mut stance = None;
            let mut retreat = None;
            let mut i = 2;
            while i < args.len() {
                match args[i].as_str() {
                    "formation" => {
                        formation = Some(nth(args, i + 1, "tactics set formation <id>")?.into());
                        i += 2;
                    }
                    "stance" => {
                        stance = Some(nth(args, i + 1, "tactics set stance <id>")?.into());
                        i += 2;
                    }
                    "retreat" => {
                        retreat = Some(nth(args, i + 1, "tactics set retreat <0-1>")?.into());
                        i += 2;
                    }
                    other => {
                        return Err(CliError::Invalid(format!(
                            "unknown tactics set field {other}"
                        )));
                    }
                }
            }
            if formation.is_none() && stance.is_none() && retreat.is_none() {
                return Err(CliError::Invalid(
                    "tactics set needs formation, stance, or retreat".into(),
                ));
            }
            Ok(Command::TacticsSet {
                formation,
                stance,
                retreat,
            })
        }
        Some("action") => {
            let key = nth(args, 2, "tactics action <hold|retreat|commit>")?;
            let action = RoundAction::from_key(key)
                .ok_or_else(|| CliError::Invalid("unknown tactical action".into()))?;
            expect_end(args, 3)?;
            Ok(Command::TacticsAction { action })
        }
        Some(other) => Err(CliError::Invalid(format!(
            "unknown tactics command {other}"
        ))),
        None => Err(CliError::Invalid("tactics needs set or action".into())),
    }
}

fn parse_staff_command(args: &[String]) -> Result<Command, CliError> {
    match args.get(1).map(String::as_str) {
        None => {
            expect_end(args, 1)?;
            Ok(Command::Staff)
        }
        Some("recruit") => {
            let mut name = String::new();
            let mut trait_id = "none".to_string();
            let mut i = 2;
            while i < args.len() {
                let a = args[i].as_str();
                if a == "--name" {
                    name = nth(args, i + 1, "staff recruit --name <text>")?.to_string();
                    i += 2;
                    continue;
                }
                if a == "--trait" {
                    trait_id = nth(args, i + 1, "staff recruit --trait <id>")?.to_string();
                    i += 2;
                    continue;
                }
                return Err(CliError::Invalid(format!("unknown staff recruit flag {a}")));
            }
            Ok(Command::StaffRecruit { name, trait_id })
        }
        Some("assign") => {
            let id = parse_leader_id(nth(args, 2, "staff assign <id> <role>")?)?;
            let role = RoleKind::from_key(nth(args, 3, "staff assign <id> <role>")?)
                .ok_or_else(|| CliError::Invalid("unknown role".into()))?;
            let mut target = match role {
                RoleKind::Admiral => AssignmentTarget::HomeFleet,
                RoleKind::Governor => AssignmentTarget::Planet(Coord::HOME),
                RoleKind::Minister => AssignmentTarget::Empire,
            };
            let mut i = 4;
            while i < args.len() {
                let a = args[i].as_str();
                if a == "--target" {
                    let raw = nth(args, i + 1, "staff assign --target <id>")?;
                    target = AssignmentTarget::parse(raw)
                        .ok_or_else(|| CliError::Invalid(format!("invalid --target {raw}")))?;
                    i += 2;
                    continue;
                }
                return Err(CliError::Invalid(format!("unknown staff assign flag {a}")));
            }
            Ok(Command::StaffAssign { id, role, target })
        }
        Some("delegate") => {
            let id = parse_leader_id(nth(args, 2, "staff delegate <id>")?)?;
            let mut permission = Permission::QueueBuild;
            let mut cap = None;
            let mut i = 3;
            while i < args.len() {
                let a = args[i].as_str();
                if a == "--permission" {
                    let raw = nth(args, i + 1, "staff delegate --permission <id>")?;
                    permission = Permission::from_key(raw)
                        .ok_or_else(|| CliError::Invalid(format!("unknown permission {raw}")))?;
                    i += 2;
                    continue;
                }
                if a == "--cap" {
                    cap = Some(nth(args, i + 1, "staff delegate --cap <n>")?.to_string());
                    i += 2;
                    continue;
                }
                return Err(CliError::Invalid(format!(
                    "unknown staff delegate flag {a}"
                )));
            }
            Ok(Command::StaffDelegate {
                id,
                permission,
                cap,
            })
        }
        Some("revoke") => {
            let id = parse_leader_id(nth(args, 2, "staff revoke <id>")?)?;
            let mut permission = None;
            let mut i = 3;
            while i < args.len() {
                let a = args[i].as_str();
                if a == "--permission" {
                    let raw = nth(args, i + 1, "staff revoke --permission <id>")?;
                    permission =
                        Some(Permission::from_key(raw).ok_or_else(|| {
                            CliError::Invalid(format!("unknown permission {raw}"))
                        })?);
                    i += 2;
                    continue;
                }
                return Err(CliError::Invalid(format!("unknown staff revoke flag {a}")));
            }
            Ok(Command::StaffRevoke { id, permission })
        }
        Some("clearance") => {
            let id = parse_leader_id(nth(args, 2, "staff clearance <id>")?)?;
            let mut class = None;
            let mut compartments = Vec::new();
            let mut i = 3;
            while i < args.len() {
                let a = args[i].as_str();
                if a == "--class" {
                    let raw = nth(args, i + 1, "staff clearance --class <id>")?;
                    class = Some(Classification::from_key(raw).ok_or_else(|| {
                        CliError::Invalid(format!("unknown classification {raw}"))
                    })?);
                    i += 2;
                    continue;
                }
                if a == "--compartment" {
                    compartments
                        .push(nth(args, i + 1, "staff clearance --compartment <id>")?.to_string());
                    i += 2;
                    continue;
                }
                return Err(CliError::Invalid(format!(
                    "unknown staff clearance flag {a}"
                )));
            }
            Ok(Command::StaffClearance {
                id,
                class: class
                    .ok_or_else(|| CliError::Invalid("staff clearance needs --class".into()))?,
                compartments,
            })
        }
        Some(other) => Err(CliError::Invalid(format!("unknown staff command {other}"))),
    }
}

fn parse_leader_id(text: &str) -> Result<u64, CliError> {
    text.parse()
        .map_err(|_| CliError::Invalid(format!("invalid leader id {text}")))
}

fn parse_amount(text: &str) -> Result<f64, CliError> {
    let n: f64 = text
        .parse()
        .map_err(|_| CliError::Invalid(format!("invalid amount {text}")))?;
    if !n.is_finite() || n < 0.0 {
        return Err(CliError::Invalid(format!("invalid amount {text}")));
    }
    Ok(n)
}

fn parse_vehicle_command(args: &[String]) -> Result<Command, CliError> {
    match args.get(1).map(String::as_str) {
        Some("list") | None => {
            if args.len() > 2 {
                return Err(CliError::Invalid("vehicle list takes no arguments".into()));
            }
            Ok(Command::VehicleList)
        }
        Some("register") => {
            let planet = parse_coord(nth(args, 2, "vehicle register <planet> <ships...>")?)?;
            let ships = parse_ships(&args[3..])?;
            Ok(Command::VehicleRegister { planet, ships })
        }
        Some("dissolve") => {
            let id = parse_leader_id(nth(args, 2, "vehicle dissolve <id>")?)?;
            expect_end(args, 3)?;
            Ok(Command::VehicleDissolve { id })
        }
        Some(other) => Err(CliError::Invalid(format!(
            "unknown vehicle command {other}"
        ))),
    }
}

fn parse_mission_command(args: &[String]) -> Result<Command, CliError> {
    match args.get(1).map(String::as_str) {
        Some("windows") => {
            let from = parse_coord(nth(args, 2, "mission windows <from> <to>")?)?;
            let to = parse_coord(nth(args, 3, "mission windows <from> <to>")?)?;
            let mut transfer = TransferKind::Hohmann;
            let mut i = 4;
            while i < args.len() {
                let a = args[i].as_str();
                if a == "--transfer" {
                    let raw = nth(args, i + 1, "mission windows --transfer <kind>")?;
                    transfer = TransferKind::from_key(raw)
                        .ok_or_else(|| CliError::Invalid(format!("unknown transfer {raw}")))?;
                    i += 2;
                    continue;
                }
                return Err(CliError::Invalid(format!(
                    "unknown mission windows flag {a}"
                )));
            }
            Ok(Command::MissionWindows { from, to, transfer })
        }
        Some("plan") => {
            let to = parse_coord(nth(args, 2, "mission plan <to>")?)?;
            let mut vehicle = None;
            let mut transfer = TransferKind::Hohmann;
            let mut customer = Customer::Colony;
            let mut metal = "0".to_string();
            let mut crystal = "0".to_string();
            let mut deuterium = "0".to_string();
            let mut window = None;
            let mut i = 3;
            while i < args.len() {
                let a = args[i].as_str();
                if a == "--vehicle" {
                    vehicle = Some(parse_leader_id(nth(
                        args,
                        i + 1,
                        "mission plan --vehicle <id>",
                    )?)?);
                    i += 2;
                    continue;
                }
                if a == "--transfer" {
                    let raw = nth(args, i + 1, "mission plan --transfer <kind>")?;
                    transfer = TransferKind::from_key(raw)
                        .ok_or_else(|| CliError::Invalid(format!("unknown transfer {raw}")))?;
                    i += 2;
                    continue;
                }
                if a == "--customer" {
                    let raw = nth(args, i + 1, "mission plan --customer <id>")?;
                    customer = Customer::parse(raw)
                        .ok_or_else(|| CliError::Invalid(format!("unknown customer {raw}")))?;
                    i += 2;
                    continue;
                }
                if a == "--metal" {
                    metal = nth(args, i + 1, "mission plan --metal <n>")?.to_string();
                    i += 2;
                    continue;
                }
                if a == "--crystal" {
                    crystal = nth(args, i + 1, "mission plan --crystal <n>")?.to_string();
                    i += 2;
                    continue;
                }
                if a == "--deuterium" {
                    deuterium = nth(args, i + 1, "mission plan --deuterium <n>")?.to_string();
                    i += 2;
                    continue;
                }
                if a == "--window" {
                    let raw = nth(args, i + 1, "mission plan --window <n>")?;
                    window = Some(
                        raw.parse()
                            .map_err(|_| CliError::Invalid(format!("invalid --window {raw}")))?,
                    );
                    i += 2;
                    continue;
                }
                return Err(CliError::Invalid(format!("unknown mission plan flag {a}")));
            }
            Ok(Command::MissionPlan {
                to,
                vehicle: vehicle
                    .ok_or_else(|| CliError::Invalid("mission plan needs --vehicle".into()))?,
                transfer,
                customer,
                metal,
                crystal,
                deuterium,
                window,
            })
        }
        Some("schedule") => {
            expect_end(args, 2)?;
            Ok(Command::MissionSchedule)
        }
        Some(other) => Err(CliError::Invalid(format!(
            "unknown mission command {other}"
        ))),
        None => Err(CliError::Invalid(
            "mission needs windows, plan, or schedule".into(),
        )),
    }
}

fn parse_market_command(args: &[String]) -> Result<Command, CliError> {
    match args.get(1).map(String::as_str) {
        Some("region") => {
            let region = match args.get(2) {
                Some(text) => Some(parse_coord(text)?),
                None => None,
            };
            if args.len() > 3 {
                return Err(CliError::Invalid(
                    "market region takes at most a coord".into(),
                ));
            }
            Ok(Command::MarketRegion { region })
        }
        Some("order") => {
            let region = parse_coord(nth(
                args,
                2,
                "market order <region> buy|sell <good> <amount>",
            )?)?;
            let side = OrderSide::from_key(nth(
                args,
                3,
                "market order <region> buy|sell <good> <amount>",
            )?)
            .ok_or_else(|| CliError::Invalid("market order needs buy or sell".into()))?;
            let good = ResourceKind::from_key(nth(
                args,
                4,
                "market order <region> buy|sell <good> <amount>",
            )?)
            .ok_or_else(|| CliError::Invalid("unknown market good".into()))?;
            let amount =
                nth(args, 5, "market order <region> buy|sell <good> <amount>")?.to_string();
            let mut limit = None;
            let mut ttl = None;
            let mut i = 6;
            while i < args.len() {
                let a = args[i].as_str();
                if a == "--limit" {
                    limit = Some(nth(args, i + 1, "market order --limit <price>")?.to_string());
                    i += 2;
                    continue;
                }
                if a == "--ttl" {
                    ttl = Some(parse_u64_flag(
                        nth(args, i + 1, "market order --ttl <secs>")?,
                        "--ttl",
                    )?);
                    i += 2;
                    continue;
                }
                return Err(CliError::Invalid(format!("unknown market order flag {a}")));
            }
            Ok(Command::MarketOrder {
                region,
                side,
                good,
                amount,
                limit,
                ttl,
            })
        }
        Some("contract") => match args.get(2).map(String::as_str) {
            Some("list") | None => {
                if args.get(2).is_some() && args.len() > 3 {
                    return Err(CliError::Invalid(
                        "market contract list takes no arguments".into(),
                    ));
                }
                Ok(Command::MarketRegion { region: None })
            }
            Some("accept") => {
                let id =
                    parse_u64_flag(nth(args, 3, "market contract accept <id>")?, "contract id")?;
                expect_end(args, 4)?;
                Ok(Command::MarketAccept { id })
            }
            Some("fulfil") => {
                let id =
                    parse_u64_flag(nth(args, 3, "market contract fulfil <id>")?, "contract id")?;
                expect_end(args, 4)?;
                Ok(Command::MarketFulfil { id })
            }
            Some(other) => Err(CliError::Invalid(format!(
                "unknown market contract command {other}"
            ))),
        },
        Some("ship") => {
            let from = parse_coord(nth(args, 2, "market ship <from> <to>")?)?;
            let to = parse_coord(nth(args, 3, "market ship <from> <to>")?)?;
            let mut metal = "0".to_string();
            let mut crystal = "0".to_string();
            let mut deuterium = "0".to_string();
            let mut contract = None;
            let mut i = 4;
            while i < args.len() {
                let a = args[i].as_str();
                if a == "--metal" {
                    metal = nth(args, i + 1, "market ship --metal <n>")?.to_string();
                    i += 2;
                    continue;
                }
                if a == "--crystal" {
                    crystal = nth(args, i + 1, "market ship --crystal <n>")?.to_string();
                    i += 2;
                    continue;
                }
                if a == "--deuterium" {
                    deuterium = nth(args, i + 1, "market ship --deuterium <n>")?.to_string();
                    i += 2;
                    continue;
                }
                if a == "--contract" {
                    contract = Some(parse_u64_flag(
                        nth(args, i + 1, "market ship --contract <id>")?,
                        "--contract",
                    )?);
                    i += 2;
                    continue;
                }
                return Err(CliError::Invalid(format!("unknown market ship flag {a}")));
            }
            Ok(Command::MarketShip {
                from,
                to,
                metal,
                crystal,
                deuterium,
                contract,
            })
        }
        Some("history") => {
            let mut region = None;
            let mut export = None;
            let mut i = 2;
            while i < args.len() {
                let a = args[i].as_str();
                if a == "--export" {
                    export = Some(PathBuf::from(nth(
                        args,
                        i + 1,
                        "market history --export <path>",
                    )?));
                    i += 2;
                    continue;
                }
                if a.starts_with('-') {
                    return Err(CliError::Invalid(format!(
                        "unknown market history flag {a}"
                    )));
                }
                if region.is_some() {
                    return Err(CliError::Invalid(
                        "market history takes at most one coord".into(),
                    ));
                }
                region = Some(parse_coord(a)?);
                i += 1;
            }
            Ok(Command::MarketHistory { region, export })
        }
        Some(other) => Err(CliError::Invalid(format!("unknown market command {other}"))),
        None => Ok(Command::MarketRegion { region: None }),
    }
}

fn market_region_value(state: &GameState, filter: Option<Coord>) -> Value {
    let orders: Vec<Value> = state
        .board
        .orders
        .values()
        .filter(|o| filter.is_none_or(|c| o.region == c))
        .map(|o| {
            json!({
                "id": o.id,
                "region": o.region.to_string(),
                "trader": o.trader.as_key(),
                "side": o.side.as_key(),
                "good": o.good.as_key(),
                "remaining": o.remaining,
                "price": o.price,
                "expires_at": o.expires_at,
                "market": o.is_market,
            })
        })
        .collect();
    let contracts: Vec<Value> = state
        .board
        .contracts
        .values()
        .filter(|c| filter.is_none_or(|r| c.region == r))
        .map(|c| {
            json!({
                "id": c.id,
                "kind": c.kind.as_key(),
                "region": c.region.to_string(),
                "good": c.good.as_key(),
                "remaining": c.remaining,
                "amount": c.amount,
                "price": c.price,
                "deadline": c.deadline,
                "penalty": c.penalty,
                "status": c.status.as_key(),
                "holder": c.holder.map(|t| t.as_key()),
            })
        })
        .collect();
    let shipments: Vec<Value> = state
        .board
        .shipments
        .values()
        .filter(|s| filter.is_none_or(|c| s.origin == c || s.dest == c))
        .map(|s| {
            json!({
                "id": s.id,
                "origin": s.origin.to_string(),
                "dest": s.dest.to_string(),
                "metal": s.cargo.metal,
                "crystal": s.cargo.crystal,
                "deuterium": s.cargo.deuterium,
                "fuel": s.fuel,
                "arrives_at": s.arrives_at,
                "delivered": s.delivered,
                "contract": s.contract_id,
            })
        })
        .collect();
    let regions: Vec<Value> = state
        .board
        .regions
        .values()
        .filter(|r| filter.is_none_or(|c| r.coord == c))
        .map(|r| {
            json!({
                "coord": r.coord.to_string(),
                "npc": {
                    "metal": r.npc_warehouse.metal,
                    "crystal": r.npc_warehouse.crystal,
                    "deuterium": r.npc_warehouse.deuterium,
                },
                "player": {
                    "metal": r.player_warehouse.metal,
                    "crystal": r.player_warehouse.crystal,
                    "deuterium": r.player_warehouse.deuterium,
                },
            })
        })
        .collect();
    json!({
        "schema_version": SCHEMA_VERSION,
        "ok": true,
        "message": "market",
        "now": state.last_tick_unix,
        "hub": state.config.market.hub().to_string(),
        "mids": {
            "metal": state.market.metal,
            "crystal": state.market.crystal,
            "deuterium": state.market.deuterium,
        },
        "regions": regions,
        "orders": orders,
        "contracts": contracts,
        "shipments": shipments,
    })
}

fn apply_market_history(
    state: &GameState,
    filter: Option<Coord>,
    export: Option<&Path>,
) -> Result<Value, CliError> {
    let mut rows: Vec<Value> = Vec::new();
    for region in state.board.regions.values() {
        if let Some(coord) = filter
            && region.coord != coord
        {
            continue;
        }
        for kind in ResourceKind::ALL {
            rows.push(json!({
                "region": region.coord.to_string(),
                "good": kind.as_key(),
                "prices": region.history(kind),
            }));
        }
    }
    if rows.is_empty() {
        for kind in ResourceKind::ALL {
            rows.push(json!({
                "region": state.home.to_string(),
                "good": kind.as_key(),
                "prices": state.market.history(kind),
            }));
        }
    }
    if let Some(path) = export {
        let mut buf = String::new();
        for row in &rows {
            buf.push_str(&row.to_string());
            buf.push('\n');
        }
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|e| CliError::Internal(e.to_string()))?;
        }
        std::fs::write(path, buf).map_err(|e| CliError::Internal(e.to_string()))?;
    }
    Ok(json!({
        "schema_version": SCHEMA_VERSION,
        "ok": true,
        "message": "market history",
        "now": state.last_tick_unix,
        "history": rows,
        "exported": export.map(|p| p.display().to_string()),
    }))
}

fn parse_moon_command(args: &[String]) -> Result<Command, CliError> {
    match args.get(1).map(String::as_str) {
        Some("build") => {
            let planet = parse_coord(nth(args, 2, "moon build <planet> <facility>")?)?;
            let facility = MoonFacility::from_key(nth(args, 3, "moon build <planet> <facility>")?)
                .ok_or_else(|| CliError::Invalid("unknown moon facility".into()))?;
            expect_end(args, 4)?;
            Ok(Command::MoonBuild { planet, facility })
        }
        Some("jump") => {
            let from = parse_coord(nth(args, 2, "moon jump <from> <to> <ships...>")?)?;
            let to = parse_coord(nth(args, 3, "moon jump <from> <to> <ships...>")?)?;
            let ships = parse_ships(&args[4..])?;
            Ok(Command::MoonJump { from, to, ships })
        }
        Some("scan") => {
            let planet = parse_coord(nth(args, 2, "moon scan <planet>")?)?;
            expect_end(args, 3)?;
            Ok(Command::MoonScan { planet })
        }
        Some(other) => Err(CliError::Invalid(format!("unknown moon command {other}"))),
        None => Err(CliError::Invalid("moon needs build, jump, or scan".into())),
    }
}

fn parse_law_command(args: &[String]) -> Result<Command, CliError> {
    match args.get(1).map(String::as_str) {
        Some("propose") => {
            let template = nth(args, 2, "law propose <template>")?.to_string();
            let mut param = None;
            let mut jurisdiction = None;
            let mut effective = 0;
            let mut i = 3;
            while i < args.len() {
                let a = args[i].as_str();
                if a == "--param" {
                    param = Some(nth(args, i + 1, "law propose --param <n>")?.to_string());
                    i += 2;
                    continue;
                }
                if a == "--jurisdiction" {
                    jurisdiction =
                        Some(nth(args, i + 1, "law propose --jurisdiction <id>")?.into());
                    i += 2;
                    continue;
                }
                if a == "--effective" {
                    effective = parse_advance(nth(args, i + 1, "law propose --effective <dur>")?)?;
                    i += 2;
                    continue;
                }
                return Err(CliError::Invalid(format!("unknown law propose flag {a}")));
            }
            Ok(Command::LawPropose {
                template,
                param,
                jurisdiction,
                effective,
            })
        }
        Some("approve") => {
            let id = parse_law_id(nth(args, 2, "law approve <id>")?)?;
            expect_end(args, 3)?;
            Ok(Command::LawApprove { id })
        }
        Some("repeal") => {
            let id = parse_law_id(nth(args, 2, "law repeal <id>")?)?;
            let mut effective = 0;
            let mut i = 3;
            while i < args.len() {
                let a = args[i].as_str();
                if a == "--effective" {
                    effective = parse_advance(nth(args, i + 1, "law repeal --effective <dur>")?)?;
                    i += 2;
                    continue;
                }
                return Err(CliError::Invalid(format!("unknown law repeal flag {a}")));
            }
            Ok(Command::LawRepeal { id, effective })
        }
        Some(other) => Err(CliError::Invalid(format!("unknown law command {other}"))),
        None => Err(CliError::Invalid(
            "usage: hoshi law propose|approve|repeal".into(),
        )),
    }
}

fn parse_law_id(text: &str) -> Result<u64, CliError> {
    text.parse()
        .map_err(|_| CliError::Invalid(format!("invalid law id {text}")))
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

fn parse_actor_flag(args: &[String], start: usize) -> Result<Actor, CliError> {
    let mut actor = Actor::Player;
    let mut i = start;
    while i < args.len() {
        let a = args[i].as_str();
        if a == "--as" {
            let raw = nth(args, i + 1, "--as <actor>")?;
            actor = Actor::parse(raw)
                .ok_or_else(|| CliError::Invalid(format!("invalid --as {raw}")))?;
            i += 2;
            continue;
        }
        return Err(CliError::Invalid(format!("unexpected argument {a}")));
    }
    Ok(actor)
}

fn parse_actor_command(
    args: &[String],
    wrap: impl FnOnce(Actor) -> Command,
) -> Result<Command, CliError> {
    Ok(wrap(parse_actor_flag(args, 1)?))
}

fn parse_marking_flags(
    args: &[String],
    mut i: usize,
) -> Result<(Classification, Vec<String>, Actor, usize), CliError> {
    let mut class = None;
    let mut compartments = Vec::new();
    let mut actor = Actor::Player;
    while i < args.len() {
        let a = args[i].as_str();
        if a == "--class" {
            let raw = nth(args, i + 1, "--class <id>")?;
            class = Some(
                Classification::from_key(raw)
                    .ok_or_else(|| CliError::Invalid(format!("unknown classification {raw}")))?,
            );
            i += 2;
            continue;
        }
        if a == "--compartment" {
            compartments.push(nth(args, i + 1, "--compartment <id>")?.to_string());
            i += 2;
            continue;
        }
        if a == "--as" {
            let raw = nth(args, i + 1, "--as <actor>")?;
            actor = Actor::parse(raw)
                .ok_or_else(|| CliError::Invalid(format!("invalid --as {raw}")))?;
            i += 2;
            continue;
        }
        return Err(CliError::Invalid(format!("unexpected argument {a}")));
    }
    Ok((
        class.ok_or_else(|| CliError::Invalid("needs --class".into()))?,
        compartments,
        actor,
        i,
    ))
}

fn parse_classify_command(args: &[String]) -> Result<Command, CliError> {
    let kind_raw = nth(args, 1, "classify <kind> <id>")?;
    let kind = ClassifiedKind::from_key(kind_raw)
        .ok_or_else(|| CliError::Invalid(format!("unknown classified kind {kind_raw}")))?;
    let id = nth(args, 2, "classify <kind> <id>")?.to_string();
    let (class, compartments, actor, _) = parse_marking_flags(args, 3)?;
    Ok(Command::Classify {
        kind,
        id,
        class,
        compartments,
        actor,
    })
}

fn parse_share_command(args: &[String]) -> Result<Command, CliError> {
    let faction = nth(args, 1, "share <faction>")?.to_string();
    let (class, compartments, _, _) = parse_marking_flags(args, 2)?;
    Ok(Command::Share {
        faction,
        class,
        compartments,
    })
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
    use hoshi_core::{BuildError, open_save, save_state};
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
                actor: Actor::Player,
            }
        );
        let opts = parse_cli(&args(&["tick", "--advance", "3m"])).unwrap();
        assert_eq!(opts.command, Command::Tick { advance: 180 });
        let opts = parse_cli(&args(&["goals"])).unwrap();
        assert_eq!(opts.command, Command::Goals);
        let opts = parse_cli(&args(&[
            "automation",
            "add",
            "stock_below",
            "home",
            "metal",
            "2000",
            "queue_build",
            "home",
            "metal_mine",
        ]))
        .unwrap();
        match opts.command {
            Command::AutomationAdd { cond, action, .. } => {
                assert_eq!(cond, "stock_below");
                assert!(matches!(action, AutomationActionSpec::QueueBuild { .. }));
            }
            other => panic!("{other:?}"),
        }
        let opts = parse_cli(&args(&["goals", "keep"])).unwrap();
        assert_eq!(opts.command, Command::GoalsKeep);
        let opts = parse_cli(&args(&[
            "market", "order", "home", "buy", "crystal", "10", "--limit", "2",
        ]))
        .unwrap();
        assert_eq!(
            opts.command,
            Command::MarketOrder {
                region: Coord::HOME,
                side: OrderSide::Buy,
                good: ResourceKind::Crystal,
                amount: "10".into(),
                limit: Some("2".into()),
                ttl: None,
            }
        );
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
        assert!(value["research"]["exclusive_picks"].is_object());
        assert!(value["achievements"].is_array());
        assert!(value["daily"]["slots"].is_array());
        assert_eq!(value["daily"]["slots"].as_array().unwrap().len(), 3);
        assert!(value["moons"].is_array());
        assert!(value["leaders"].is_array());
        assert_eq!(value["leaders"].as_array().unwrap().len(), 2);
        assert!(value["ladder"]["next"].is_object());
        assert_eq!(value["ladder"]["next"]["id"], "water_propellant");
        assert_eq!(value["frontier"]["galaxy"], 2);
        assert_eq!(value["frontier"]["revealed"], 0);
        assert!(value["frontier"]["worlds"].as_array().unwrap().is_empty());
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
    fn law_propose_approve_enacts_and_state_lists_samples() {
        let save = unique_save("law");
        let (value, exit) = run_on(
            &save,
            &["--seed=1", "law", "propose", "tax_rate", "--param", "50"],
        );
        assert_eq!(exit, EXIT_OK, "{value}");
        let (value, exit) = run_on(&save, &["law", "approve", "0"]);
        assert_eq!(exit, EXIT_OK, "{value}");
        let (tick, exit) = run_on(&save, &["tick", "--advance", "1h"]);
        assert_eq!(exit, EXIT_OK, "{tick}");
        let (state, exit) = run_on(&save, &["state"]);
        assert_eq!(exit, EXIT_OK, "{state}");
        assert_eq!(state["laws"][0]["status"], "enacted");
        assert_eq!(state["laws"][0]["template"], "tax_rate");
        assert_eq!(state["laws"][0]["approvals"][0]["who"], "player");
        let sample = &state["laws"][0]["samples"][0];
        assert_eq!(sample["metric"], "revenue");
        let expected = sample["expected"].as_f64().unwrap();
        let actual = sample["actual"].as_f64().unwrap();
        assert!(
            (expected - 50.0).abs() < 1.0,
            "expected {expected} after 1h at 50 metal/hour"
        );
        assert!(
            (actual - 50.0).abs() < 1.0,
            "actual {actual} after 1h at 50 metal/hour"
        );
        let _ = std::fs::remove_file(&save);
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
        assert!(!is_cli_invocation(&args(&["--commentary"])));
        assert!(!is_cli_invocation(&args(&["--advisor"])));
        assert!(!is_cli_invocation(&args(&["--no-portraits"])));
        assert!(!is_cli_invocation(&args(&["--arena"])));
        assert!(!is_cli_invocation(&args(&["--demo"])));
        assert!(is_cli_invocation(&args(&["arena"])));
        assert!(is_cli_invocation(&args(&["demo"])));
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
    fn feedback_appends_a_versioned_line_without_the_seed() {
        let save = unique_save("feedback");
        let (created, exit) = run_on(&save, &["--seed=7", "state"]);
        assert_eq!(exit, EXIT_OK);
        assert!(created.get("galaxy_seed").is_none());
        let (value, exit) = run_on(&save, &["feedback", "mines", "felt", "slow"]);
        assert_eq!(exit, EXIT_OK, "{value}");
        assert_eq!(value["ok"], true);
        assert_eq!(value["message"], "feedback recorded");
        let log = save.with_extension("feedback.log");
        let text = std::fs::read_to_string(&log).unwrap();
        let line: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        assert_eq!(line["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(line["note"], "mines felt slow");
        assert_eq!(line["state"]["save_schema"], SAVE_SCHEMA);
        assert!(line.get("galaxy_seed").is_none());
        assert!(line["state"].get("galaxy_seed").is_none());
        let before = created["resources"]["metal"].clone();
        let (after, _) = run_on(&save, &["state"]);
        assert_eq!(after["resources"]["metal"], before);
        let _ = std::fs::remove_file(&save);
        let _ = std::fs::remove_file(&log);
    }

    #[test]
    fn advise_lists_suggestions_without_acting() {
        let save = unique_save("advise");
        let (created, exit) = run_on(&save, &["--seed=1", "state"]);
        assert_eq!(exit, EXIT_OK);
        let metal = created["resources"]["metal"].clone();
        let (value, exit) = run_on(&save, &["advise"]);
        assert_eq!(exit, EXIT_OK, "{value}");
        assert_eq!(value["ok"], true);
        assert_eq!(value["message"], "advisor");
        let tips = value["suggestions"].as_array().expect("suggestions");
        assert!(!tips.is_empty());
        assert!(
            tips.iter()
                .all(|row| row.get("id").is_some() && row.get("text").is_some())
        );
        let (after, _) = run_on(&save, &["state"]);
        assert_eq!(after["resources"]["metal"], metal);
        assert_eq!(after["build_queue"], created["build_queue"]);
        let _ = std::fs::remove_file(&save);
    }

    #[test]
    fn tactics_set_persists_in_state() {
        let save = unique_save("tactics");
        let (value, exit) = run_on(
            &save,
            &[
                "--seed=1",
                "tactics",
                "set",
                "formation",
                "wedge",
                "stance",
                "defensive",
                "retreat",
                "0.25",
            ],
        );
        assert_eq!(exit, EXIT_OK, "{value}");
        let (state, exit) = run_on(&save, &["state"]);
        assert_eq!(exit, EXIT_OK, "{state}");
        assert_eq!(state["tactics"]["formation"], "wedge");
        assert_eq!(state["tactics"]["stance"], "defensive");
        assert!((state["tactics"]["retreat_below"].as_f64().unwrap() - 0.25).abs() < 1e-12);
        let _ = std::fs::remove_file(&save);
    }

    #[test]
    fn mission_plan_uses_orbital_checks() {
        let save = unique_save("mission");
        let mut conn = open_save(&save).unwrap();
        let mut seeded = GameState::with_seed(0, BalanceConfig::default(), 1);
        seeded.fleet.add(ShipKind::SmallCargo, 2);
        seeded.resources.metal = 50_000.0;
        seeded.resources.crystal = 50_000.0;
        seeded.resources.deuterium = 50_000.0;
        save_state(&mut conn, &seeded).unwrap();
        drop(conn);
        let (reg, exit) = run_on(&save, &["vehicle", "register", "home", "small_cargo:1"]);
        assert_eq!(exit, EXIT_OK, "{reg}");
        let id = reg["vehicles"][0]["id"].as_u64().unwrap();
        let (win, exit) = run_on(
            &save,
            &[
                "mission",
                "windows",
                "home",
                "1:1:8",
                "--transfer",
                "hohmann",
            ],
        );
        assert_eq!(exit, EXIT_OK, "{win}");
        assert!(win["delta_v"].as_f64().unwrap() > 0.0);
        let (sched, exit) = run_on(&save, &["mission", "schedule"]);
        assert_eq!(exit, EXIT_OK, "{sched}");
        assert!(sched["alerts"].as_array().unwrap().is_empty());
        let (plan, exit) = run_on(
            &save,
            &[
                "mission",
                "plan",
                "1:1:8",
                "--vehicle",
                &id.to_string(),
                "--transfer",
                "hohmann",
                "--customer",
                "colony",
                "--metal",
                "1",
            ],
        );
        assert_eq!(exit, EXIT_REFUSED, "{plan}");
        assert!(
            plan["error"]["message"].as_str().unwrap().contains("owned")
                || plan["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("window"),
            "{plan}"
        );
        let _ = std::fs::remove_file(&save);
    }

    #[test]
    fn extra_args_are_invalid() {
        for parts in [
            &["state", "typo"][..],
            &["advise", "typo"],
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

    #[test]
    fn log_search_lineage_and_jsonl_export() {
        let save = unique_save("log");
        let (created, exit) = run_on(&save, &["--seed=1", "build", "home", "metal_mine"]);
        assert_eq!(exit, EXIT_OK, "{created}");
        let (tick, exit) = run_on(&save, &["tick", "--advance", "1h"]);
        assert_eq!(exit, EXIT_OK, "{tick}");
        let (value, exit) = run_on(&save, &["log", "--type", "build"]);
        assert_eq!(exit, EXIT_OK, "{value}");
        let events = value["events"].as_array().expect("events");
        assert!(events.iter().any(|e| e["action"] == "queue"), "{value}");
        assert!(events.iter().any(|e| e["action"] == "complete"), "{value}");
        let queued = events.iter().find(|e| e["action"] == "queue").unwrap();
        let complete = events.iter().find(|e| e["action"] == "complete").unwrap();
        assert_eq!(complete["parent_ids"][0], queued["id"]);
        let id = complete["id"].as_u64().unwrap();
        let (lineage, exit) = run_on(&save, &["log", "--lineage", &id.to_string()]);
        assert_eq!(exit, EXIT_OK, "{lineage}");
        assert_eq!(lineage["events"].as_array().unwrap().len(), 2);
        let at = queued["at"].as_u64().unwrap();
        let (travel, exit) = run_on(&save, &["log", "--at", &at.to_string()]);
        assert_eq!(exit, EXIT_OK, "{travel}");
        assert_eq!(travel["snapshot"]["event_id"], queued["id"]);
        let jsonl = save.with_extension("jsonl");
        let (exported, exit) = run_on(&save, &["log", "--export", jsonl.to_str().unwrap()]);
        assert_eq!(exit, EXIT_OK, "{exported}");
        let text = std::fs::read_to_string(&jsonl).unwrap();
        assert!(text.contains("\"action\":\"queue\""));
        let _ = std::fs::remove_file(&save);
        let _ = std::fs::remove_file(&jsonl);
    }

    #[test]
    fn staff_recruit_and_as_build_respects_permissions() {
        let save = unique_save("staff");
        let (value, exit) = run_on(&save, &["--seed=1", "staff"]);
        assert_eq!(exit, EXIT_OK, "{value}");
        assert_eq!(value["leaders"].as_array().unwrap().len(), 2);
        let (value, exit) = run_on(
            &save,
            &["staff", "recruit", "--name", "Kai", "--trait", "logistics"],
        );
        assert_eq!(exit, EXIT_OK, "{value}");
        let id = value["leaders"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["name"] == "Kai")
            .unwrap()["id"]
            .as_u64()
            .unwrap();
        let (denied, exit) = run_on(
            &save,
            &[
                "build",
                "home",
                "metal_mine",
                "--as",
                &format!("leader:{id}"),
            ],
        );
        assert_eq!(exit, EXIT_REFUSED, "{denied}");
        let (ok, exit) = run_on(&save, &["staff", "assign", &id.to_string(), "governor"]);
        assert_eq!(exit, EXIT_OK, "{ok}");
        let (bad, exit) = run_on(
            &save,
            &[
                "staff",
                "delegate",
                &id.to_string(),
                "--permission",
                "queue_build",
                "--cap",
                "NaN",
            ],
        );
        assert_eq!(exit, EXIT_INVALID, "{bad}");
        let (ok, exit) = run_on(
            &save,
            &[
                "staff",
                "delegate",
                &id.to_string(),
                "--permission",
                "queue_build",
            ],
        );
        assert_eq!(exit, EXIT_OK, "{ok}");
        let (tick, exit) = run_on(&save, &["tick", "--advance", "4h"]);
        assert_eq!(exit, EXIT_OK, "{tick}");
        let (queued, exit) = run_on(
            &save,
            &[
                "build",
                "home",
                "metal_mine",
                "--as",
                &format!("leader:{id}"),
            ],
        );
        assert_eq!(exit, EXIT_OK, "{queued}");
        let _ = std::fs::remove_file(&save);
    }

    #[test]
    fn self_play_is_off_until_flag_or_config() {
        let off = run_cli(&args(&["self-play", "--matrix", "pr"]));
        assert_eq!(off.exit, EXIT_REFUSED, "{}", off.json);
        assert!(off.json.contains("self-play is off"), "{}", off.json);

        let on = parse_cli(&args(&[
            "--self-play",
            "self-play",
            "--matrix",
            "pr",
            "--seed",
            "1",
        ]))
        .unwrap();
        assert!(on.self_play);
        assert_eq!(on.seed, Some(1));
        match on.command {
            Command::SelfPlay { matrix, bots, .. } => {
                assert_eq!(matrix, "pr");
                assert!(!bots);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn self_play_pr_matrix_passes_through_the_cli() {
        let out = run_cli(&args(&["--self-play", "self-play", "--matrix", "pr"]));
        assert_eq!(out.exit, EXIT_OK, "{}", out.json);
        assert!(out.json.contains("self-play"), "{}", out.json);
        assert!(out.json.contains("passed"), "{}", out.json);
    }

    #[test]
    fn arena_is_off_until_flag_or_config() {
        let out = run_cli(&args(&[
            "arena",
            "run",
            "--seed",
            "1",
            "--days",
            "1",
            "missing.sh",
        ]));
        assert_eq!(out.exit, EXIT_REFUSED);
        let opts = parse_cli(&args(&[
            "--arena",
            "arena",
            "run",
            "--seed",
            "9",
            "--days",
            "2",
            "--actions",
            "4",
            "bot.sh",
        ]))
        .unwrap();
        assert!(opts.arena);
        assert_eq!(opts.seed, Some(9));
        match opts.command {
            Command::ArenaRun {
                days,
                actions,
                bots,
                opt_in,
                backend,
                ..
            } => {
                assert_eq!(days, 2);
                assert_eq!(actions, 4);
                assert_eq!(bots, vec![PathBuf::from("bot.sh")]);
                assert!(!opt_in);
                assert_eq!(backend, None);
            }
            other => panic!("{other:?}"),
        }
        let opts = parse_cli(&args(&[
            "arena", "run", "--arena", "--seed", "3", "--days", "1", "a.sh", "b.py",
        ]))
        .unwrap();
        assert!(opts.arena);
        match opts.command {
            Command::ArenaRun { bots, .. } => {
                assert_eq!(bots.len(), 2);
            }
            other => panic!("{other:?}"),
        }
        let opts = parse_cli(&args(&[
            "--arena",
            "arena",
            "run",
            "--seed",
            "1",
            "--days",
            "1",
            "--backend",
            "chisei",
            "bot.sh",
        ]))
        .unwrap();
        match opts.command {
            Command::ArenaRun { backend, .. } => assert_eq!(backend, Some(true)),
            other => panic!("{other:?}"),
        }
        let opts = parse_cli(&args(&[
            "--arena",
            "arena",
            "run",
            "--seed",
            "1",
            "--days",
            "1",
            "--backend",
            "sqlite",
            "bot.sh",
        ]))
        .unwrap();
        match opts.command {
            Command::ArenaRun { backend, .. } => assert_eq!(backend, Some(false)),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn arena_config_true_runs_without_the_flag() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "hoshi-cli-arena-cfg-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), "arena = true\n").unwrap();
        let save = dir.join("save.sqlite");
        let board = dir.join("board.json");
        let bot = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/bots/greedy.sh");
        let argv = vec![
            "hoshi".into(),
            format!("--save={}", save.display()),
            "arena".into(),
            "run".into(),
            "--seed".into(),
            "5".into(),
            "--days".into(),
            "1".into(),
            "--actions".into(),
            "3".into(),
            "--board".into(),
            board.display().to_string(),
            bot.display().to_string(),
        ];
        let out = run_cli(&argv);
        assert_eq!(out.exit, EXIT_OK, "{}", out.json);
        let value: Value = serde_json::from_str(&out.json).unwrap();
        assert_eq!(value["bots"][0]["status"], "ok");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn state_hides_secret_reports_from_uncleared_leader() {
        let save = unique_save("classify");
        let mut conn = open_save(&save).unwrap();
        let mut state = GameState::with_seed(10, BalanceConfig::default(), 1);
        state.spy_reports.push(SpyReport {
            id: 0,
            target: Coord::HOME,
            created_at: 10,
            probes_sent: 1,
            probes_lost: 0,
            detected: false,
            score: 1.0,
            tier: "fleet".into(),
            resources: None,
            fleet: None,
            defences: None,
            garrison: None,
            kind: None,
            size: None,
            formation: None,
            stance: None,
        });
        state
            .classify(
                Actor::Player,
                ClassifiedKind::Report,
                "0",
                Classification::Secret,
                [hoshi_core::COMPARTMENT_INTEL.into()],
            )
            .unwrap();
        state.events.push(hoshi_core::GameEvent {
            at: 10,
            text: format!(
                "outpost {}: spy. score 1 fleet. detected no. lost 0/1",
                Coord::HOME
            ),
        });
        save_state(&mut conn, &state).unwrap();
        drop(conn);
        let (player, exit) = run_on(&save, &["state"]);
        assert_eq!(exit, EXIT_OK, "{player}");
        assert_eq!(player["spy_reports"].as_array().unwrap().len(), 1);
        assert!(
            player["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["text"].as_str().unwrap_or("").contains("spy"))
        );
        let (hidden, exit) = run_on(&save, &["state", "--as", "leader:1"]);
        assert_eq!(exit, EXIT_OK, "{hidden}");
        assert!(hidden["spy_reports"].as_array().unwrap().is_empty());
        assert!(
            hidden["events"]
                .as_array()
                .unwrap()
                .iter()
                .all(|row| !row["text"].as_str().unwrap_or("").contains("spy"))
        );
        let (classified, exit) = run_on(
            &save,
            &[
                "classify",
                "fact",
                "note",
                "--class",
                "internal",
                "--compartment",
                "fleet",
                "--as",
                "leader:1",
            ],
        );
        assert_eq!(exit, EXIT_OK, "{classified}");
        assert!(classified["spy_reports"].as_array().unwrap().is_empty());
        let _ = std::fs::remove_file(&save);
    }

    #[test]
    fn parse_classify_and_share() {
        let opts = parse_cli(&args(&[
            "classify",
            "report",
            "0",
            "--class",
            "secret",
            "--compartment",
            "intel",
        ]))
        .unwrap();
        assert_eq!(
            opts.command,
            Command::Classify {
                kind: ClassifiedKind::Report,
                id: "0".into(),
                class: Classification::Secret,
                compartments: vec!["intel".into()],
                actor: Actor::Player,
            }
        );
        let opts = parse_cli(&args(&[
            "share",
            "traders",
            "--class",
            "restricted",
            "--compartment",
            "intel",
        ]))
        .unwrap();
        assert_eq!(
            opts.command,
            Command::Share {
                faction: "traders".into(),
                class: Classification::Restricted,
                compartments: vec!["intel".into()],
            }
        );
    }

    #[test]
    fn demo_governance_is_off_until_opt_in() {
        let out = run_cli(&args(&["demo", "governance", "--seed", "7"]));
        assert_eq!(out.exit, EXIT_REFUSED, "{}", out.json);
        let value: Value = serde_json::from_str(&out.json).unwrap();
        assert!(
            value["error"]["message"]
                .as_str()
                .unwrap()
                .contains("demo is off"),
            "{}",
            out.json
        );
        let opts = parse_cli(&args(&[
            "--demo",
            "demo",
            "governance",
            "--seed",
            "9",
            "--report",
            "out.md",
        ]))
        .unwrap();
        assert!(opts.demo);
        assert_eq!(opts.seed, Some(9));
        match opts.command {
            Command::DemoGovernance { report, opt_in, .. } => {
                assert_eq!(report, Some(PathBuf::from("out.md")));
                assert!(!opt_in);
            }
            other => panic!("{other:?}"),
        }
    }

    #[cfg(feature = "chisei")]
    #[test]
    fn demo_governance_runs_when_opted_in() {
        let out = run_cli(&args(&["--demo", "demo", "governance", "--seed", "7"]));
        assert_eq!(out.exit, EXIT_OK, "{}", out.json);
        let value: Value = serde_json::from_str(&out.json).unwrap();
        assert_eq!(value["message"], "governance demo");
        assert_eq!(value["scenarios"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn blueprint_capture_export_import_and_apply_round_trip() {
        let save = unique_save("blueprint");
        let (captured, exit) = run_on(&save, &["--seed=1", "blueprint", "capture", "home", "core"]);
        assert_eq!(exit, EXIT_OK, "{captured}");
        assert_eq!(captured["name"], "core");
        assert_eq!(captured["planet_type"], "rocky");
        let (listed, exit) = run_on(&save, &["blueprint", "list"]);
        assert_eq!(exit, EXIT_OK, "{listed}");
        assert_eq!(listed["blueprints"][0], "core");
        let (exported, exit) = run_on(&save, &["blueprint", "export", "core"]);
        assert_eq!(exit, EXIT_OK, "{exported}");
        assert!(
            exported["text"]
                .as_str()
                .unwrap()
                .contains("name = \"core\""),
            "{exported}"
        );
        let (applied, exit) = run_on(&save, &["blueprint", "apply", "core", "home"]);
        assert_eq!(exit, EXIT_OK, "{applied}");
        assert_eq!(applied["message"], "blueprint applied");
        let dir = std::env::temp_dir().join(format!("hoshi-bp-import-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("copy.toml");
        std::fs::write(&path, exported["text"].as_str().unwrap()).unwrap();
        let (imported, exit) = run_on(&save, &["blueprint", "import", &path.to_string_lossy()]);
        assert_eq!(exit, EXIT_OK, "{imported}");
        assert_eq!(imported["name"], "core");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&save);
    }
}
