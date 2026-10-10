//! hoshi: an idle space-strategy game for the terminal (placeholder screen).

use hoshi_core::{Clock, GameState, SystemClock};
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
        let text = format!(
            "hoshi (pre-alpha)\n\nMetal: {:.0}  (+{:.0}/h)\n\nPress q to quit.",
            state.metal, state.metal_per_hour
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
