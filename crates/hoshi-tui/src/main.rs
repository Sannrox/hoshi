//! hoshi: an idle space-strategy game for the terminal.

mod app;
mod cli;

use app::{Action, App};
use hoshi_core::{
    BalanceConfig, Clock, SystemClock, load_or_new, lock_save, open_save, resolve_save_path,
    save_state, simulate_greedy,
};
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const SAVE_INTERVAL: Duration = Duration::from_secs(5);

fn main() {
    let args: Vec<String> = std::env::args().collect();
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
    if let Err(e) = run() {
        eprintln!("hoshi: {e}");
        std::process::exit(1);
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

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let clock = SystemClock;
    let flag = parse_save_flag(std::env::args());
    let env = std::env::var_os("HOSHI_SAVE");
    let path = resolve_save_path(flag.as_deref(), env.as_deref());
    let config =
        load_balance(balance_override(parse_path_flag(std::env::args(), "--balance")).as_deref())?;
    let _lock = lock_save(&path)?;
    let mut conn = open_save(&path)?;
    let state = load_or_new(&mut conn, clock.now_unix(), config)?;
    refuse_deterministic_tui(&state)?;
    let mut app = App::new(state);
    #[cfg(feature = "bridge")]
    let mut bridge = {
        let dir = path.parent().unwrap_or_else(|| std::path::Path::new("."));
        hoshi_bridge::Bridge::open(hoshi_bridge::config_for_state_dir(dir))?
    };

    let mut terminal = ratatui::init();
    let mut last_save = Instant::now();
    let result = loop {
        #[cfg(feature = "bridge")]
        {
            if let Ok(batch) = bridge.poll() {
                apply_bridge_events(&mut app.state, batch, clock.now_unix());
            }
        }
        app.state.catch_up(clock.now_unix());
        if last_save.elapsed() >= SAVE_INTERVAL {
            if let Err(e) = save_state(&mut conn, &app.state) {
                break Err(std::io::Error::other(e.to_string()));
            }
            last_save = Instant::now();
        }
        if let Err(e) = terminal.draw(|f| app.draw(f)) {
            break Err(e);
        }
        match event::poll(Duration::from_secs(1)) {
            Ok(true) => match event::read() {
                Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => {
                    if app.handle_key(k.code, clock.now_unix()) == Action::Quit {
                        break Ok(());
                    }
                }
                Ok(_) => {}
                Err(e) => break Err(e),
            },
            Ok(false) => {}
            Err(e) => break Err(e),
        }
    };
    ratatui::restore();
    save_state(&mut conn, &app.state)?;
    result?;
    Ok(())
}

#[cfg(feature = "bridge")]
fn apply_bridge_events(
    state: &mut hoshi_core::GameState,
    batch: hoshi_bridge::ReadBatch,
    now_unix: u64,
) {
    let mut events = batch.events;
    events.sort_by_key(|e| e.ts);
    for event in events {
        if let Some(pulse) = pulse_from_event(&event) {
            let _ = state.apply_activity(pulse, now_unix);
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
        ActivityKind::AgentStarted | ActivityKind::AgentFailed => None,
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
}
