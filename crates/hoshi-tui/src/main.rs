//! hoshi: an idle space-strategy game for the terminal.

use hoshi_core::{
    BalanceConfig, Clock, GameState, ResourceKind, SystemClock, load_or_new, open_save,
    resolve_save_path, save_state,
};
use ratatui::{
    crossterm::event::{self, Event, KeyCode, KeyEventKind},
    widgets::{Block, Paragraph},
};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const SAVE_INTERVAL: Duration = Duration::from_secs(5);

fn main() {
    if let Err(e) = run() {
        eprintln!("hoshi: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let clock = SystemClock;
    let flag = parse_save_flag(std::env::args());
    let env = std::env::var_os("HOSHI_SAVE");
    let path = resolve_save_path(flag.as_deref(), env.as_deref());
    let mut conn = open_save(&path)?;
    let mut state = load_or_new(&mut conn, clock.now_unix(), BalanceConfig::default())?;

    let mut terminal = ratatui::init();
    let mut last_save = Instant::now();
    let result = loop {
        state.catch_up(clock.now_unix());
        if last_save.elapsed() >= SAVE_INTERVAL {
            if let Err(e) = save_state(&mut conn, &state) {
                break Err(std::io::Error::other(e.to_string()));
            }
            last_save = Instant::now();
        }
        let text = render(&state);
        if let Err(e) = terminal.draw(|f| {
            f.render_widget(
                Paragraph::new(text).block(Block::bordered().title(" hoshi ")),
                f.area(),
            )
        }) {
            break Err(e);
        }
        match event::poll(Duration::from_millis(500)) {
            Ok(true) => match event::read() {
                Ok(Event::Key(k))
                    if k.kind == KeyEventKind::Press && k.code == KeyCode::Char('q') =>
                {
                    break Ok(());
                }
                Ok(_) => {}
                Err(e) => break Err(e),
            },
            Ok(false) => {}
            Err(e) => break Err(e),
        }
    };
    ratatui::restore();
    save_state(&mut conn, &state)?;
    result?;
    Ok(())
}

fn render(state: &GameState) -> String {
    let build_line = match state.build_queue.first() {
        Some(job) => {
            let left = job
                .completes_at
                .unwrap_or(state.last_tick_unix)
                .saturating_sub(state.last_tick_unix);
            format!(
                "Build: {} -> lv{}  {}s  ({}/{} queued)\n\n",
                job.building,
                job.from_level + 1,
                left,
                state.build_queue.len(),
                state.config.max_build_queue
            )
        }
        None => "Build: idle\n\n".to_string(),
    };
    let research_line = match &state.research {
        Some(job) => {
            let left = job.completes_at.saturating_sub(state.last_tick_unix);
            format!(
                "Research: {} -> lv{}  {}s\n\n",
                job.tech,
                job.from_level + 1,
                left
            )
        }
        None => "Research: idle\n\n".to_string(),
    };
    format!(
        "hoshi (pre-alpha)\n\n\
         Metal:      {:>10.0}  (+{:.0}/h)  cap {:.0}\n\
         Crystal:    {:>10.0}  (+{:.0}/h)  cap {:.0}\n\
         Deuterium:  {:>10.0}  (+{:.0}/h)  cap {:.0}\n\
         Energy:     {:>10.0} / {:.0}\n\
         {build_line}\
         {research_line}\
         Press q to quit.",
        state.resources.metal,
        state.production_per_hour(ResourceKind::Metal),
        state.storage_cap(ResourceKind::Metal),
        state.resources.crystal,
        state.production_per_hour(ResourceKind::Crystal),
        state.storage_cap(ResourceKind::Crystal),
        state.resources.deuterium,
        state.production_per_hour(ResourceKind::Deuterium),
        state.storage_cap(ResourceKind::Deuterium),
        state.energy_produced(),
        state.energy_consumed(),
    )
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
}
