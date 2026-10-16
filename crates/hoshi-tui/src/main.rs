//! hoshi: an idle space-strategy game for the terminal.

mod app;
mod arena;
mod cli;
mod clip;
mod cockpit;
mod config;
mod demo;
mod detail_panel;
mod keymap;
mod notify;
mod portraits;
mod title;

use app::{Action, App};
use hoshi_core::{
    BackendKind, BalanceConfig, Clock, GameState, Prestige, RivalCatalog, RivalSettings, SaveError,
    SeasonCatalog, SpeciesCatalog, SystemClock, default_save_dir, encode_galaxy_seed, inspect_save,
    lock_save, open_storage, parse_galaxy_seed, resolve_save_path, simulate_greedy,
};
use portraits::{PortraitView, Protocol, detect_protocol};
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use title::{
    NewGameSpec, Title, TitleAction, apply_difficulty, continue_slot, list_slots, slot_message,
    snapshot_away, summarize_away,
};

const SAVE_INTERVAL: Duration = Duration::from_secs(5);

fn write_notify(bytes: Vec<u8>) {
    if bytes.is_empty() {
        return;
    }
    let mut out = std::io::stdout();
    if !out.is_terminal() {
        return;
    }
    let _ = out.write_all(&bytes);
    let _ = out.flush();
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        match option_env!("HOSHI_BUILD") {
            Some(build) => println!("hoshi {} ({build})", env!("CARGO_PKG_VERSION")),
            None => println!("hoshi {}", env!("CARGO_PKG_VERSION")),
        }
        return;
    }
    if args.get(1).map(String::as_str) == Some("post") {
        match run_post(&args[2..]) {
            Ok(()) => return,
            Err(e) => {
                eprintln!("hoshi: {e}");
                std::process::exit(2);
            }
        }
    }
    if args.get(1).map(String::as_str) == Some("simulate") {
        match run_simulate(&args[2..]) {
            Ok(()) => return,
            Err(e) => {
                eprintln!("hoshi: {e}");
                std::process::exit(2);
            }
        }
    }
    if args.get(1).map(String::as_str) == Some("season") {
        match run_season(&args[2..]) {
            Ok(()) => return,
            Err(e) => {
                eprintln!("hoshi: {e}");
                std::process::exit(e.exit);
            }
        }
    }
    if args.get(1).map(String::as_str) == Some("new") {
        match run_new_game(&args) {
            Ok(()) => return,
            Err(e) => {
                eprintln!("hoshi: {e}");
                std::process::exit(e.exit);
            }
        }
    }
    if cli::is_cli_invocation(&args) {
        let out = cli::run_cli(&args);
        let mut stdout = std::io::stdout();
        if stdout
            .write_all(out.json.as_bytes())
            .and_then(|_| stdout.flush())
            .is_err()
        {
            std::process::exit(cli::EXIT_INTERNAL);
        }
        std::process::exit(out.exit);
    }
    match run() {
        Ok(()) => {}
        Err(e) => {
            eprintln!("hoshi: {e}");
            std::process::exit(e.exit);
        }
    }
}

#[derive(Debug)]
struct LaunchError {
    message: String,
    exit: i32,
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for LaunchError {}

impl From<Box<dyn std::error::Error>> for LaunchError {
    fn from(value: Box<dyn std::error::Error>) -> Self {
        LaunchError {
            message: value.to_string(),
            exit: 1,
        }
    }
}

impl From<SaveError> for LaunchError {
    fn from(value: SaveError) -> Self {
        let exit = match value {
            SaveError::Locked => cli::EXIT_LOCKED,
            _ => cli::EXIT_INTERNAL,
        };
        LaunchError {
            message: value.to_string(),
            exit,
        }
    }
}

impl From<std::io::Error> for LaunchError {
    fn from(value: std::io::Error) -> Self {
        LaunchError {
            message: value.to_string(),
            exit: cli::EXIT_INTERNAL,
        }
    }
}

fn run_post(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "bridge")]
    {
        hoshi_bridge::run_post(args)?;
        Ok(())
    }
    #[cfg(not(feature = "bridge"))]
    {
        let _ = args;
        Err("post requires a build with --features bridge".into())
    }
}

fn run_season(args: &[String]) -> Result<(), LaunchError> {
    let catalog = SeasonCatalog::default();
    let now = SystemClock.now_unix();
    let window = catalog.window(now);
    if args.is_empty() {
        println!(
            "window {}–{}s UTC  ({}s)",
            window.start, window.end, catalog.duration_secs
        );
        for ruleset in catalog.iter() {
            let prestige = if ruleset.prestige {
                "shares prestige"
            } else {
                "prestige isolated"
            };
            println!(
                "{}  {}  {prestige}  {}",
                ruleset.id, ruleset.name, ruleset.blurb
            );
        }
        return Ok(());
    }
    if args[0] != "start" {
        return Err(LaunchError {
            message: "season start <id>".into(),
            exit: cli::EXIT_INVALID,
        });
    }
    let id = args.get(1).ok_or_else(|| LaunchError {
        message: "season start <id>".into(),
        exit: cli::EXIT_INVALID,
    })?;
    let mut save = None;
    let mut i = 2;
    while i < args.len() {
        let a = args[i].as_str();
        if a == "--save" {
            let v = args.get(i + 1).ok_or_else(|| LaunchError {
                message: "--save needs a value".into(),
                exit: cli::EXIT_INVALID,
            })?;
            save = Some(PathBuf::from(v));
            i += 2;
            continue;
        }
        if let Some(v) = a.strip_prefix("--save=") {
            save = Some(PathBuf::from(v));
            i += 1;
            continue;
        }
        return Err(LaunchError {
            message: format!("unknown season argument {a}"),
            exit: cli::EXIT_INVALID,
        });
    }
    let name = hoshi_core::season_slot_name(id, window.start);
    let path = save.unwrap_or_else(|| default_save_dir().join(format!("{name}.sqlite")));
    if path.exists() {
        return Err(LaunchError {
            message: "save already exists".into(),
            exit: cli::EXIT_INVALID,
        });
    }
    let state =
        GameState::new_season(now, &catalog, id, BalanceConfig::default()).map_err(|e| {
            LaunchError {
                message: e.to_string(),
                exit: cli::EXIT_INVALID,
            }
        })?;
    let _lock = lock_save(&path)?;
    let mut store = open_storage(&path)?;
    store.save(&state)?;
    println!("{}", encode_galaxy_seed(state.galaxy_seed));
    Ok(())
}

fn run_new_game(args: &[String]) -> Result<(), LaunchError> {
    let mut seed = None;
    let mut save = None;
    let mut balance_path = None;
    let mut species_id = "humans".to_string();
    let mut rivals = false;
    let mut rival_aggression: Option<f64> = None;
    let mut rival_efficiency: Option<f64> = None;
    let mut rival_bonus: Option<f64> = None;
    let mut rival_personalities: Vec<String> = Vec::new();
    let mut i = 2;
    while i < args.len() {
        let a = args[i].as_str();
        if let Some(v) = a.strip_prefix("--seed=") {
            seed = Some(parse_new_seed(v)?);
            i += 1;
            continue;
        }
        if a == "--seed" {
            let v = args.get(i + 1).ok_or_else(|| LaunchError {
                message: "--seed needs a value".into(),
                exit: cli::EXIT_INVALID,
            })?;
            seed = Some(parse_new_seed(v)?);
            i += 2;
            continue;
        }
        if let Some(v) = a.strip_prefix("--species=") {
            species_id = v.to_string();
            i += 1;
            continue;
        }
        if a == "--species" {
            let v = args.get(i + 1).ok_or_else(|| LaunchError {
                message: "--species needs a value".into(),
                exit: cli::EXIT_INVALID,
            })?;
            species_id = v.clone();
            i += 2;
            continue;
        }
        if a == "--rivals" {
            rivals = true;
            i += 1;
            continue;
        }
        if let Some(v) = a.strip_prefix("--rival-aggression=") {
            rival_aggression = Some(parse_rival_f64(v, "--rival-aggression")?);
            i += 1;
            continue;
        }
        if a == "--rival-aggression" {
            let v = args.get(i + 1).ok_or_else(|| LaunchError {
                message: "--rival-aggression needs a value".into(),
                exit: cli::EXIT_INVALID,
            })?;
            rival_aggression = Some(parse_rival_f64(v, "--rival-aggression")?);
            i += 2;
            continue;
        }
        if let Some(v) = a.strip_prefix("--rival-efficiency=") {
            rival_efficiency = Some(parse_rival_f64(v, "--rival-efficiency")?);
            i += 1;
            continue;
        }
        if a == "--rival-efficiency" {
            let v = args.get(i + 1).ok_or_else(|| LaunchError {
                message: "--rival-efficiency needs a value".into(),
                exit: cli::EXIT_INVALID,
            })?;
            rival_efficiency = Some(parse_rival_f64(v, "--rival-efficiency")?);
            i += 2;
            continue;
        }
        if let Some(v) = a.strip_prefix("--rival-bonus=") {
            rival_bonus = Some(parse_rival_f64(v, "--rival-bonus")?);
            i += 1;
            continue;
        }
        if a == "--rival-bonus" {
            let v = args.get(i + 1).ok_or_else(|| LaunchError {
                message: "--rival-bonus needs a value".into(),
                exit: cli::EXIT_INVALID,
            })?;
            rival_bonus = Some(parse_rival_f64(v, "--rival-bonus")?);
            i += 2;
            continue;
        }
        if let Some(v) = a.strip_prefix("--rival-personality=") {
            rival_personalities.push(v.to_string());
            i += 1;
            continue;
        }
        if a == "--rival-personality" {
            let v = args.get(i + 1).ok_or_else(|| LaunchError {
                message: "--rival-personality needs a value".into(),
                exit: cli::EXIT_INVALID,
            })?;
            rival_personalities.push(v.clone());
            i += 2;
            continue;
        }
        if let Some(v) = a.strip_prefix("--save=") {
            save = Some(PathBuf::from(v));
            i += 1;
            continue;
        }
        if a == "--save" {
            let v = args.get(i + 1).ok_or_else(|| LaunchError {
                message: "--save needs a value".into(),
                exit: cli::EXIT_INVALID,
            })?;
            save = Some(PathBuf::from(v));
            i += 2;
            continue;
        }
        if let Some(v) = a.strip_prefix("--balance=") {
            balance_path = Some(PathBuf::from(v));
            i += 1;
            continue;
        }
        if a == "--balance" {
            let v = args.get(i + 1).ok_or_else(|| LaunchError {
                message: "--balance needs a value".into(),
                exit: cli::EXIT_INVALID,
            })?;
            balance_path = Some(PathBuf::from(v));
            i += 2;
            continue;
        }
        return Err(LaunchError {
            message: format!("unknown new argument {a}"),
            exit: cli::EXIT_INVALID,
        });
    }
    let seed = seed.ok_or_else(|| LaunchError {
        message: "new needs --seed".into(),
        exit: cli::EXIT_INVALID,
    })?;
    let path = resolve_save_path(save.as_deref(), std::env::var_os("HOSHI_SAVE").as_deref());
    if path.exists() {
        return Err(LaunchError {
            message: "save already exists".into(),
            exit: cli::EXIT_INVALID,
        });
    }
    let balance =
        load_balance(balance_override(balance_path).as_deref()).map_err(|e| LaunchError {
            message: e.to_string(),
            exit: cli::EXIT_INTERNAL,
        })?;
    let now = SystemClock.now_unix();
    let catalog = SpeciesCatalog::default();
    let mut state = GameState::new_with_species(
        now,
        &catalog,
        &species_id,
        balance,
        seed,
        Prestige::new(now),
    )
    .map_err(|e| LaunchError {
        message: e.to_string(),
        exit: cli::EXIT_INVALID,
    })?;
    if rivals {
        let mut settings = RivalSettings::on();
        if let Some(v) = rival_aggression {
            settings.aggression = v;
        }
        if let Some(v) = rival_efficiency {
            settings.efficiency = v;
        }
        if let Some(v) = rival_bonus {
            settings.starting_bonus = v;
        }
        settings.personalities = rival_personalities;
        state
            .spawn_rivals(&RivalCatalog::default(), settings)
            .map_err(|e| LaunchError {
                message: e.to_string(),
                exit: cli::EXIT_INVALID,
            })?;
    }
    let _lock = lock_save(&path)?;
    let mut store = open_storage(&path)?;
    store.save(&state)?;
    println!("{}", encode_galaxy_seed(seed));
    Ok(())
}

fn parse_rival_f64(v: &str, flag: &str) -> Result<f64, LaunchError> {
    let n: f64 = v.parse().map_err(|_| LaunchError {
        message: format!("{flag} needs a number"),
        exit: cli::EXIT_INVALID,
    })?;
    if !n.is_finite() || !(0.0..=10.0).contains(&n) {
        return Err(LaunchError {
            message: format!("{flag} must be a finite number in 0..=10"),
            exit: cli::EXIT_INVALID,
        });
    }
    Ok(n)
}

fn parse_new_seed(v: &str) -> Result<u64, LaunchError> {
    parse_galaxy_seed(v)
        .map_err(|e| LaunchError {
            message: e,
            exit: cli::EXIT_INVALID,
        })?
        .ok_or_else(|| LaunchError {
            message: "new needs --seed".into(),
            exit: cli::EXIT_INVALID,
        })
}

fn run_simulate(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let opts = parse_simulate_args(args)?;
    let config = load_balance(balance_override(opts.balance).as_deref())?;
    let report = simulate_greedy(config, opts.days);
    let markdown = report.to_markdown();
    if let Some(path) = opts.report {
        std::fs::write(&path, &markdown)?;
    }
    print!("{markdown}");
    if !report.passed() {
        return Err("pacing targets failed".into());
    }
    Ok(())
}

fn load_balance(path: Option<&Path>) -> Result<BalanceConfig, Box<dyn std::error::Error>> {
    match path {
        Some(path) => Ok(BalanceConfig::load_file(path)?),
        None => Ok(BalanceConfig::default()),
    }
}

fn balance_override(flag: Option<PathBuf>) -> Option<PathBuf> {
    flag.or_else(|| std::env::var_os("HOSHI_BALANCE").map(PathBuf::from))
}

#[derive(Debug)]
struct SimulateOpts {
    days: u32,
    balance: Option<PathBuf>,
    report: Option<PathBuf>,
}

fn parse_simulate_args(args: &[String]) -> Result<SimulateOpts, Box<dyn std::error::Error>> {
    let mut days = 3_u32;
    let mut balance = None;
    let mut report = None;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if let Some(rest) = arg.strip_prefix("--days=") {
            days = parse_days(rest)?;
            i += 1;
            continue;
        }
        if arg == "--days" {
            let value = args.get(i + 1).ok_or("simulate --days needs a number")?;
            days = parse_days(value)?;
            i += 2;
            continue;
        }
        if let Some(path) = named_path(arg, "--balance") {
            balance = Some(path);
            i += 1;
            continue;
        }
        if arg == "--balance" {
            let value = args.get(i + 1).ok_or("simulate --balance needs a path")?;
            balance = Some(PathBuf::from(value));
            i += 2;
            continue;
        }
        if let Some(path) = named_path(arg, "--report") {
            report = Some(path);
            i += 1;
            continue;
        }
        if arg == "--report" {
            let value = args.get(i + 1).ok_or("simulate --report needs a path")?;
            report = Some(PathBuf::from(value));
            i += 2;
            continue;
        }
        return Err(format!("unknown simulate flag {arg}").into());
    }
    Ok(SimulateOpts {
        days,
        balance,
        report,
    })
}

fn parse_days(value: &str) -> Result<u32, Box<dyn std::error::Error>> {
    let days: u32 = value
        .parse()
        .map_err(|_| format!("invalid --days {value}"))?;
    if days == 0 || days > 30 {
        return Err("--days must be between 1 and 30".into());
    }
    Ok(days)
}

fn named_path(arg: &str, flag: &str) -> Option<PathBuf> {
    arg.strip_prefix(&format!("{flag}=")).map(PathBuf::from)
}

fn refuse_deterministic_tui(
    state: &hoshi_core::GameState,
) -> Result<(), Box<dyn std::error::Error>> {
    if state.deterministic {
        Err("this save is deterministic; use the JSON CLI instead of the TUI".into())
    } else {
        Ok(())
    }
}

fn run() -> Result<(), LaunchError> {
    let clock = SystemClock;
    let args: Vec<String> = std::env::args().collect();
    let flag = parse_save_flag(args.iter());
    let env = std::env::var_os("HOSHI_SAVE");
    let continue_flag = args.iter().any(|a| a == "--continue");
    let no_portraits = args.iter().any(|a| a == "--no-portraits");
    let commentary_flag = args.iter().any(|a| a == "--commentary");
    let advisor_flag = args.iter().any(|a| a == "--advisor");
    let balance =
        load_balance(balance_override(parse_path_flag(args.iter(), "--balance")).as_deref())
            .map_err(|e| LaunchError {
                message: e.to_string(),
                exit: cli::EXIT_INTERNAL,
            })?;
    if flag.is_some() || env.is_some() {
        let path = resolve_save_path(flag.as_deref(), env.as_deref());
        let mut cfg = config::UserConfig::load(&config_dir_for_save(&path));
        if commentary_flag {
            cfg.commentary = true;
        }
        if advisor_flag {
            cfg.ai_advisor = true;
        }
        return play_path(
            &path,
            balance,
            &clock,
            None,
            portrait_view(&cfg, no_portraits),
            &cfg,
        );
    }
    let dir = default_save_dir();
    if continue_flag {
        let path = resolve_continue(&dir, clock.now_unix())?;
        let mut cfg = config::UserConfig::load(&dir);
        if commentary_flag {
            cfg.commentary = true;
        }
        if advisor_flag {
            cfg.ai_advisor = true;
        }
        return play_path(
            &path,
            balance,
            &clock,
            None,
            portrait_view(&cfg, no_portraits),
            &cfg,
        );
    }
    title_then_play(
        dir,
        balance,
        &clock,
        no_portraits,
        commentary_flag,
        advisor_flag,
    )
}

fn config_dir_for_save(path: &Path) -> PathBuf {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

fn resolve_continue(dir: &Path, now_unix: u64) -> Result<PathBuf, LaunchError> {
    let cfg = config::UserConfig::load(dir);
    let slots = list_slots(dir, now_unix);
    let Some(slot) = continue_slot(&slots, cfg.last_slot.as_deref()) else {
        return Err(LaunchError {
            message: "no save to continue".into(),
            exit: cli::EXIT_INTERNAL,
        });
    };
    match &slot.status {
        title::SlotStatus::Ready(_) => Ok(slot.path.clone()),
        other => Err(LaunchError {
            message: slot_message(other),
            exit: match other {
                title::SlotStatus::Locked => cli::EXIT_LOCKED,
                _ => cli::EXIT_INTERNAL,
            },
        }),
    }
}

fn title_then_play(
    dir: PathBuf,
    balance: BalanceConfig,
    clock: &SystemClock,
    no_portraits: bool,
    commentary_flag: bool,
    advisor_flag: bool,
) -> Result<(), LaunchError> {
    let mut title = Title::new(dir, clock.now_unix());
    if commentary_flag {
        title.config.commentary = true;
    }
    if advisor_flag {
        title.config.ai_advisor = true;
    }
    title.set_balance(balance.clone());
    let mut terminal = ratatui::init();
    let result = loop {
        title.set_now(clock.now_unix());
        title.tick();
        if let Err(e) = terminal.draw(|f| title.draw(f)) {
            break Err(LaunchError {
                message: e.to_string(),
                exit: 1,
            });
        }
        match event::poll(Duration::from_secs(1)) {
            Ok(true) => match event::read() {
                Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => {
                    match title.handle_key(k.code) {
                        TitleAction::Quit => break Ok(None),
                        TitleAction::Play(_) => {
                            let path = title
                                .take_play_path()
                                .unwrap_or_else(|| title.new_game_spec().path);
                            let new_spec = if path.exists() {
                                None
                            } else {
                                Some(title.new_game_spec())
                            };
                            break Ok(Some((path, new_spec)));
                        }
                        TitleAction::None => {}
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    break Err(LaunchError {
                        message: e.to_string(),
                        exit: 1,
                    });
                }
            },
            Ok(false) => {}
            Err(e) => {
                break Err(LaunchError {
                    message: e.to_string(),
                    exit: 1,
                });
            }
        }
    };
    ratatui::restore();
    match result? {
        None => Ok(()),
        Some((path, new_spec)) => play_path(
            &path,
            balance,
            clock,
            new_spec,
            portrait_view(&title.config, no_portraits),
            &title.config,
        ),
    }
}

fn should_open_bridge(user_enabled: bool) -> bool {
    cfg!(feature = "bridge") && user_enabled
}

fn portrait_view(cfg: &config::UserConfig, no_portraits: bool) -> PortraitView {
    if no_portraits || !cfg.portraits {
        return PortraitView {
            enabled: false,
            protocol: Protocol::None,
        };
    }
    PortraitView {
        enabled: true,
        protocol: detect_protocol(cfg.portrait_protocol, |key| std::env::var(key).ok()),
    }
}

fn play_path(
    path: &Path,
    mut config: BalanceConfig,
    clock: &SystemClock,
    new_spec: Option<NewGameSpec>,
    portraits: PortraitView,
    user: &config::UserConfig,
) -> Result<(), LaunchError> {
    let now = clock.now_unix();
    if new_spec.is_none() && path.exists() && BackendKind::from_path(path) == BackendKind::Sqlite {
        match inspect_save(path) {
            Err(SaveError::Locked) => return Err(SaveError::Locked.into()),
            Err(e) => return Err(e.into()),
            Ok(g) if g.deterministic => {
                return Err(LaunchError {
                    message: "this save is deterministic; use the JSON CLI instead of the TUI"
                        .into(),
                    exit: cli::EXIT_INTERNAL,
                });
            }
            Ok(_) => {}
        }
    }
    let _lock = lock_save(path)?;
    let mut store = open_storage(path)?;
    let mut state = if let Some(spec) = new_spec {
        apply_difficulty(&mut config, &spec.difficulty);
        let mut state = if let Some(id) = spec.season_id.as_deref() {
            let catalog = SeasonCatalog::default();
            GameState::new_season(now, &catalog, id, config).map_err(|e| LaunchError {
                message: e.to_string(),
                exit: cli::EXIT_INVALID,
            })?
        } else {
            let seed = spec.seed.unwrap_or(now.wrapping_mul(0x9E37_79B9_7F4A_7C15));
            let catalog = SpeciesCatalog::default();
            GameState::new_with_species(
                now,
                &catalog,
                &spec.species_id,
                config,
                seed,
                spec.prestige.clone(),
            )
            .map_err(|e| LaunchError {
                message: e.to_string(),
                exit: cli::EXIT_INVALID,
            })?
        };
        if spec.rivals.enabled {
            state
                .spawn_rivals(&RivalCatalog::default(), spec.rivals.clone())
                .map_err(|e| LaunchError {
                    message: e.to_string(),
                    exit: cli::EXIT_INVALID,
                })?;
        }
        let share = state.season.as_ref().is_none_or(|run| run.prestige);
        if spec.season_id.is_some()
            && share
            && (spec.prestige.lifetime > 0
                || !spec.prestige.owned.is_empty()
                || !spec.prestige.runs.is_empty()
                || !spec.prestige.unlocked_species.is_empty())
        {
            state.inherit_prestige(spec.prestige);
        }
        store.save(&state)?;
        state
    } else {
        store.load_or_new(now, config)?
    };
    refuse_deterministic_tui(&state).map_err(|e| LaunchError {
        message: e.to_string(),
        exit: cli::EXIT_INTERNAL,
    })?;
    let before = snapshot_away(&state);
    state.catch_up(now);
    if user.reset_tutorial {
        state.reset_hints();
        store.save(&state)?;
        let mut cfg = user.clone();
        cfg.reset_tutorial = false;
        let dir = path.parent().unwrap_or_else(|| Path::new("."));
        let _ = cfg.save(dir);
    }
    let mut user = user.clone();
    let mut app = App::with_keymap(state, user.keys.clone());
    app.apply_cockpit(&user);
    app.portraits = portraits;
    app.reduced_motion = user.reduced_motion;
    app.no_animation = user.no_animation;
    app.show_tutorial = user.tutorial;
    app.commentary = user.commentary;
    app.commentary_model_url = user.commentary_model_url.clone();
    app.ai_advisor = user.ai_advisor;
    app.advisor_model_url = user.advisor_model_url.clone();
    app.advisor_model_name = user.advisor_model_name.clone();
    app.advisor_allow_lan = user.advisor_allow_lan;
    app.governance = user.governance;
    app.set_notify(user.notify_prefs());
    #[cfg(feature = "bridge")]
    app.set_bridge_ui(user.bridge);
    if now.saturating_sub(before.tick) >= user.away_threshold_secs {
        let away = summarize_away(&before, &app.state);
        write_notify(app.notify_away(&away));
        app.set_away(away);
    }
    let open_bridge = should_open_bridge(user.bridge);
    #[cfg(feature = "bridge")]
    let mut bridge = if open_bridge {
        let dir = path.parent().unwrap_or_else(|| std::path::Path::new("."));
        Some(
            hoshi_bridge::Bridge::open(hoshi_bridge::config_for_state_dir(dir)).map_err(|e| {
                LaunchError {
                    message: e.to_string(),
                    exit: 1,
                }
            })?,
        )
    } else {
        None
    };
    #[cfg(not(feature = "bridge"))]
    let _ = open_bridge;

    let mut terminal = ratatui::init();
    let mut last_save = Instant::now();
    let result = loop {
        #[cfg(feature = "bridge")]
        {
            if let Some(bridge) = bridge.as_mut()
                && let Ok(batch) = bridge.poll()
            {
                apply_bridge_events(&mut app, batch, clock.now_unix());
            }
        }
        app.catch_up_now(clock.now_unix());
        write_notify(app.poll_notify());
        if last_save.elapsed() >= SAVE_INTERVAL {
            app.state.finish_pending_tactical();
            if let Err(e) = store.save(&app.state) {
                break Err(std::io::Error::other(e.to_string()));
            }
            last_save = Instant::now();
        }
        if let Err(e) = terminal.draw(|f| app.draw(f)) {
            break Err(e);
        }
        let size = match terminal.size() {
            Ok(size) => size,
            Err(e) => break Err(e),
        };
        let area = ratatui::layout::Rect::new(0, 0, size.width, size.height);
        if let Err(e) = app.paint_inline_graphics(&mut std::io::stdout(), area) {
            break Err(e);
        }
        let wait = if app.replay_active() {
            Duration::from_millis(100)
        } else {
            Duration::from_secs(1)
        };
        match event::poll(wait) {
            Ok(true) => match event::read() {
                Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => {
                    if app.handle_key(k.code, clock.now_unix()) == Action::Quit {
                        break Ok(());
                    }
                    if let Some(layout) = app.take_cockpit_layout() {
                        user.cockpit_layout = layout;
                        let dir = path.parent().unwrap_or_else(|| Path::new("."));
                        let _ = user.save(dir);
                    }
                    if let Some(on) = app.take_detail_panel() {
                        user.detail_panel = on;
                        let dir = path.parent().unwrap_or_else(|| Path::new("."));
                        let _ = user.save(dir);
                    }
                    write_notify(app.poll_notify());
                }
                Ok(_) => {}
                Err(e) => break Err(e),
            },
            Ok(false) => app.tick_replay(),
            Err(e) => break Err(e),
        }
    };
    ratatui::restore();
    app.state.finish_pending_tactical();
    store.save(&app.state)?;
    result?;
    Ok(())
}

#[cfg(feature = "bridge")]
fn apply_bridge_events(app: &mut App, batch: hoshi_bridge::ReadBatch, now_unix: u64) {
    let mut events = batch.events;
    events.sort_by_key(|e| e.ts);
    for event in events {
        let mut applied = false;
        if let Some(pulse) = pulse_from_event(&event)
            && app.state.apply_activity(pulse, now_unix) == hoshi_core::ActivityApply::Applied
        {
            applied = true;
        }
        if let Some((signal, id)) = frontier_from_event(&event)
            && app
                .state
                .apply_frontier_bridge(signal, &id, event.ts, now_unix)
                == hoshi_core::ActivityApply::Applied
        {
            applied = true;
        }
        if applied {
            let effect = app
                .state
                .events
                .last()
                .map(|e| e.text.clone())
                .unwrap_or_else(|| event.kind.as_key().to_string());
            let alert = effect.contains("raid") || effect.contains("lost");
            app.record_bridge(&event.source, &effect, alert);
        }
    }
}

#[cfg(feature = "bridge")]
fn pulse_from_event(event: &hoshi_bridge::ActivityEvent) -> Option<hoshi_core::ActivityPulse> {
    use hoshi_bridge::ActivityKind;
    use hoshi_core::ActivityPulse;
    match event.kind {
        ActivityKind::PrMerged => Some(ActivityPulse::PrMerged { ts: event.ts }),
        ActivityKind::WorkflowFinished => {
            let success =
                event.payload.get("conclusion").and_then(|v| v.as_str()) == Some("success");
            Some(ActivityPulse::WorkflowFinished {
                ts: event.ts,
                success,
            })
        }
        ActivityKind::AgentFinished => Some(ActivityPulse::AgentFinished {
            ts: event.ts,
            duration_secs: payload_secs(event, &["duration_secs", "elapsed"]),
        }),
        ActivityKind::AgentWaitingForInput => Some(ActivityPulse::AgentWaiting {
            ts: event.ts,
            waiting_secs: payload_secs(event, &["waiting_secs", "duration_secs", "elapsed"]),
        }),
        ActivityKind::AgentStarted
        | ActivityKind::AgentFailed
        | ActivityKind::ReleasePublished
        | ActivityKind::EpicClosed => None,
    }
}

#[cfg(feature = "bridge")]
fn frontier_from_event(
    event: &hoshi_bridge::ActivityEvent,
) -> Option<(hoshi_core::FrontierSignal, String)> {
    use hoshi_bridge::ActivityKind;
    let id = match event.payload.get("id") {
        Some(serde_json::Value::String(s)) if !s.is_empty() => s.clone(),
        Some(serde_json::Value::Number(n)) => n.to_string(),
        _ => return None,
    };
    let id = match event
        .payload
        .get("repo")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        Some(repo) => format!("{repo}:{id}"),
        None => id,
    };
    match event.kind {
        ActivityKind::ReleasePublished => Some((hoshi_core::FrontierSignal::ReleasePublished, id)),
        ActivityKind::EpicClosed => Some((hoshi_core::FrontierSignal::EpicClosed, id)),
        _ => None,
    }
}

#[cfg(feature = "bridge")]
fn payload_secs(event: &hoshi_bridge::ActivityEvent, keys: &[&str]) -> u64 {
    for key in keys {
        if let Some(n) = event.payload.get(*key).and_then(|v| v.as_u64()) {
            return n;
        }
    }
    0
}

fn parse_save_flag(args: impl IntoIterator<Item = impl AsRef<str>>) -> Option<PathBuf> {
    parse_path_flag(args, "--save")
}

fn parse_path_flag(args: impl IntoIterator<Item = impl AsRef<str>>, flag: &str) -> Option<PathBuf> {
    let mut args = args.into_iter();
    let eq = format!("{flag}=");
    while let Some(arg) = args.next() {
        let arg = arg.as_ref();
        if let Some(rest) = arg.strip_prefix(&eq) {
            return Some(PathBuf::from(rest));
        }
        if arg == flag {
            return args.next().map(|path| PathBuf::from(path.as_ref()));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoshi_core::resolve_save_path;
    use std::path::Path;

    #[test]
    fn parse_save_flag_reads_space_and_equals_forms() {
        assert_eq!(
            parse_save_flag(["hoshi", "--save", "/tmp/flag.sqlite"]),
            Some(PathBuf::from("/tmp/flag.sqlite"))
        );
        assert_eq!(
            parse_save_flag(["hoshi", "--save=/tmp/eq.sqlite"]),
            Some(PathBuf::from("/tmp/eq.sqlite"))
        );
        assert_eq!(parse_save_flag(["hoshi"]), None);
    }

    #[test]
    fn parse_balance_flag_reads_space_and_equals_forms() {
        assert_eq!(
            parse_path_flag(["hoshi", "--balance", "/tmp/b.toml"], "--balance"),
            Some(PathBuf::from("/tmp/b.toml"))
        );
        assert_eq!(
            parse_path_flag(["hoshi", "--balance=/tmp/eq.toml"], "--balance"),
            Some(PathBuf::from("/tmp/eq.toml"))
        );
    }

    #[test]
    fn parse_simulate_args_reads_days_balance_and_report() {
        let opts = parse_simulate_args(&[
            "--days".into(),
            "7".into(),
            "--balance=crates/hoshi-core/data/balance.toml".into(),
            "--report".into(),
            "out.md".into(),
        ])
        .unwrap();
        assert_eq!(opts.days, 7);
        assert_eq!(
            opts.balance.as_deref(),
            Some(Path::new("crates/hoshi-core/data/balance.toml"))
        );
        assert_eq!(opts.report.as_deref(), Some(Path::new("out.md")));
    }

    #[test]
    fn parse_simulate_args_rejects_zero_days() {
        let err = parse_simulate_args(&["--days".into(), "0".into()]).unwrap_err();
        assert!(err.to_string().contains("days"), "{err}");
    }

    #[test]
    fn balance_override_prefers_the_flag() {
        assert_eq!(
            balance_override(Some(PathBuf::from("/tmp/flag.toml"))),
            Some(PathBuf::from("/tmp/flag.toml"))
        );
    }

    #[test]
    fn launch_keeps_bridge_off_until_the_user_enables_it() {
        assert!(!should_open_bridge(false));
        assert_eq!(should_open_bridge(true), cfg!(feature = "bridge"));
    }

    #[test]
    fn save_flag_reads_config_beside_the_file() {
        let path = PathBuf::from("/tmp/custom-slot/game.sqlite");
        assert_eq!(config_dir_for_save(&path), Path::new("/tmp/custom-slot"));
        assert_eq!(
            config_dir_for_save(Path::new("game.sqlite")),
            Path::new(".")
        );
    }

    #[test]
    fn tui_refuses_a_deterministic_save() {
        let live = hoshi_core::GameState::new(0);
        assert!(refuse_deterministic_tui(&live).is_ok());
        let seeded = hoshi_core::GameState::with_seed(0, hoshi_core::BalanceConfig::default(), 1);
        let err = refuse_deterministic_tui(&seeded).unwrap_err();
        assert!(err.to_string().contains("deterministic"), "{err}");
    }

    #[test]
    fn tui_save_path_uses_flag_then_env() {
        let flag = parse_save_flag(["hoshi", "--save", "/tmp/flag.sqlite"]);
        let path = resolve_save_path(
            flag.as_deref(),
            Some(std::ffi::OsStr::new("/tmp/env.sqlite")),
        );
        assert_eq!(path, Path::new("/tmp/flag.sqlite"));
        let path = resolve_save_path(None, Some(std::ffi::OsStr::new("/tmp/env.sqlite")));
        assert_eq!(path, Path::new("/tmp/env.sqlite"));
    }

    #[test]
    fn post_without_bridge_feature_explains() {
        #[cfg(not(feature = "bridge"))]
        {
            let err = run_post(&[]).unwrap_err();
            assert!(err.to_string().contains("bridge"));
        }
    }

    #[test]
    #[cfg(feature = "bridge")]
    fn pulse_maps_merged_pr_and_green_ci() {
        let pr = hoshi_bridge::parse_line(
            r#"{"type":"pr_merged","source":"github","ts":9,"payload":{"id":1}}"#,
            8192,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            pulse_from_event(&pr),
            Some(hoshi_core::ActivityPulse::PrMerged { ts: 9 })
        );
        let ci = hoshi_bridge::parse_line(
            r#"{"type":"workflow_finished","source":"github","ts":4,"payload":{"conclusion":"success"}}"#,
            8192,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            pulse_from_event(&ci),
            Some(hoshi_core::ActivityPulse::WorkflowFinished {
                ts: 4,
                success: true,
            })
        );
    }

    #[test]
    #[cfg(feature = "bridge")]
    fn frontier_events_keep_repo_in_the_seen_id() {
        let with_repo = hoshi_bridge::parse_line(
            r#"{"type":"release_published","source":"github","ts":9,"payload":{"id":"v1.0.0","repo":"acme/one"}}"#,
            8192,
        )
        .unwrap()
        .unwrap();
        let other_repo = hoshi_bridge::parse_line(
            r#"{"type":"release_published","source":"github","ts":9,"payload":{"id":"v1.0.0","repo":"acme/two"}}"#,
            8192,
        )
        .unwrap()
        .unwrap();
        let local = hoshi_bridge::parse_line(
            r#"{"type":"release_published","source":"github","ts":9,"payload":{"id":"v1.0.0"}}"#,
            8192,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            frontier_from_event(&with_repo).unwrap().1,
            "acme/one:v1.0.0"
        );
        assert_eq!(
            frontier_from_event(&other_repo).unwrap().1,
            "acme/two:v1.0.0"
        );
        assert_eq!(frontier_from_event(&local).unwrap().1, "v1.0.0");
        let empty = hoshi_bridge::parse_line(
            r#"{"type":"release_published","source":"github","ts":9,"payload":{"id":"","repo":"acme/one"}}"#,
            8192,
        )
        .unwrap()
        .unwrap();
        assert!(frontier_from_event(&empty).is_none());
    }
}
