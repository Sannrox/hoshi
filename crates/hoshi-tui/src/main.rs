//! hoshi: an idle space-strategy game for the terminal.

mod app;

use app::{Action, App};
use hoshi_core::{
    BalanceConfig, Clock, SystemClock, load_or_new, open_save, resolve_save_path, save_state,
};
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use std::path::PathBuf;
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

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let clock = SystemClock;
    let flag = parse_save_flag(std::env::args());
    let env = std::env::var_os("HOSHI_SAVE");
    let path = resolve_save_path(flag.as_deref(), env.as_deref());
    let mut conn = open_save(&path)?;
    let state = load_or_new(&mut conn, clock.now_unix(), BalanceConfig::default())?;
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
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let arg = arg.as_ref();
        if let Some(rest) = arg.strip_prefix("--save=") {
            return Some(PathBuf::from(rest));
        }
        if arg == "--save" {
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
