//! hoshi: an idle space-strategy game for the terminal (placeholder screen).

use hoshi_core::{Clock, GameState, ResourceKind, SystemClock};
use ratatui::{
    crossterm::event::{self, Event, KeyCode, KeyEventKind},
    widgets::{Block, Paragraph},
};
use std::time::Duration;

fn main() -> std::io::Result<()> {
    let clock = SystemClock;
    let mut state = GameState::new(clock.now_unix());
    let mut terminal = ratatui::init();
    let result = loop {
        state.catch_up(clock.now_unix());
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
        let text = format!(
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
        );
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
    result
}
