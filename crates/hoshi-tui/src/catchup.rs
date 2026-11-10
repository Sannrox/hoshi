//! The frame shown while a save catches up on offline time, so a long gap
//! reads as work in progress rather than a frozen terminal.

use crate::ui::chrome::{Navbar, fill, put};
use crate::ui::spans::duration;
use crate::ui::{Surface, Theme};
use hoshi_core::{CatchUpLedger, GameState};
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;
use std::time::{Duration, Instant};

/// Fastest redraw while catching up. Progress, not animation.
pub const REDRAW_EVERY: Duration = Duration::from_millis(250);

const BAR_MAX: u16 = 66;

/// Navbar, a progress line with a bar, and a footer naming what follows.
pub struct CatchUpFrame<'a> {
    pub theme: &'a Theme,
    pub slot: &'a str,
    pub done_secs: u64,
    pub total_secs: u64,
    /// The away summary opens afterwards.
    pub summary: bool,
}

impl Widget for CatchUpFrame<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let t = self.theme;
        fill(buf, area, t.surface(Surface::Base));
        if area.height == 0 {
            return;
        }
        let row = |y: u16| Rect {
            y: area.y + y,
            height: 1,
            ..area
        };
        Navbar {
            theme: t,
            mark: "HOSHI",
            crumbs: vec!["Loading".into(), self.slot.into()],
            right: Vec::new(),
        }
        .render(row(0), buf);
        let rule = Line::styled("─".repeat(usize::from(area.width)), t.line());
        if area.height > 1 {
            put(buf, row(1), &rule);
        }
        let done = self.done_secs.min(self.total_secs);
        let mid = area.height / 2;
        let text = Line::from(vec![
            Span::raw("    "),
            Span::styled("Catching up", t.strong()),
            Span::raw(format!(
                "  {} of {}",
                duration(done),
                duration(self.total_secs)
            )),
        ]);
        put(buf, row(mid.saturating_sub(1)), &text);
        let bar = area.width.saturating_sub(8).min(BAR_MAX);
        let filled = (u64::from(bar) * done)
            .checked_div(self.total_secs)
            .unwrap_or(0) as usize;
        let bar = Line::from(vec![
            Span::raw("    "),
            Span::styled("█".repeat(filled), t.strong()),
            Span::styled("░".repeat(usize::from(bar) - filled), t.muted()),
        ]);
        put(buf, row(mid), &bar);
        if self.summary && area.height > 4 {
            put(buf, row(area.height - 2), &rule);
            put(
                buf,
                row(area.height - 1),
                &Line::styled("  the summary opens when done", t.muted()),
            );
        }
    }
}

/// Catches `state` up to `now_unix`, drawing a [`CatchUpFrame`] in `theme`
/// first and then at most once per `every` as time is reached. Draw errors
/// are left to the game loop that follows; catch-up always finishes.
pub fn run<B: Backend>(
    terminal: &mut Terminal<B>,
    state: &mut GameState,
    now_unix: u64,
    slot: &str,
    summary: bool,
    theme: &Theme,
    every: Duration,
) -> CatchUpLedger {
    let start = state.last_tick_unix;
    let total_secs = now_unix.saturating_sub(start);
    let mut draw = |done_secs: u64| {
        let _ = terminal.draw(|f| {
            f.render_widget(
                CatchUpFrame {
                    theme,
                    slot,
                    done_secs,
                    total_secs,
                    summary,
                },
                f.area(),
            );
        });
    };
    if total_secs == 0 {
        return state.catch_up_ledger_with(now_unix, &mut |_| {});
    }
    draw(0);
    let mut last = Instant::now();
    state.catch_up_ledger_with(now_unix, &mut |at| {
        if last.elapsed() >= every {
            draw(at.saturating_sub(start));
            last = Instant::now();
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    fn text(terminal: &Terminal<TestBackend>) -> Vec<String> {
        let buf = terminal.backend().buffer();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect()
    }

    fn away_state() -> (GameState, u64) {
        let mut cfg = hoshi_core::BalanceConfig::default();
        cfg.raid.enabled = true;
        let state = GameState::with_config(0, cfg);
        (state, 3 * 86_400 + 4 * 3_600)
    }

    #[test]
    fn the_frame_names_the_slot_and_the_gap_and_fills_its_bar() {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let theme = Theme::default();
        terminal
            .draw(|f| {
                f.render_widget(
                    CatchUpFrame {
                        theme: &theme,
                        slot: "main",
                        done_secs: 86_400 + 22 * 3_600,
                        total_secs: 3 * 86_400 + 4 * 3_600,
                        summary: true,
                    },
                    f.area(),
                )
            })
            .unwrap();
        let rows = text(&terminal);
        assert!(rows[0].contains("Loading / main"), "{}", rows[0]);
        let all = rows.join("\n");
        assert!(all.contains("Catching up  1d 22h of 3d 04h"), "{all}");
        let bar = rows.iter().find(|r| r.contains('░')).unwrap();
        let filled = bar.matches('█').count();
        assert!((38..=40).contains(&filled), "{filled}: {bar}");
        assert!(all.contains("the summary opens when done"), "{all}");
    }

    #[test]
    fn a_narrow_terminal_keeps_the_frame_inside_it() {
        let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
        let theme = Theme::default();
        terminal
            .draw(|f| {
                f.render_widget(
                    CatchUpFrame {
                        theme: &theme,
                        slot: "main",
                        done_secs: 10,
                        total_secs: 20,
                        summary: false,
                    },
                    f.area(),
                )
            })
            .unwrap();
        let all = text(&terminal).join("\n");
        assert!(all.contains("Catching up"), "{all}");
        assert!(!all.contains("summary"), "{all}");
    }

    #[test]
    fn catch_up_draws_its_frame_before_the_game_and_gives_the_same_result() {
        let (start, now) = away_state();
        let mut plain = start.clone();
        let plain_ledger = plain.catch_up_ledger(now);

        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut state = start.clone();
        let ledger = run(
            &mut terminal,
            &mut state,
            now,
            "main",
            true,
            &Theme::default(),
            REDRAW_EVERY,
        );
        assert_eq!(ledger, plain_ledger);
        assert_eq!(state, plain);
        // Nothing else drew yet, so the screen still holds the catch-up frame.
        let all = text(&terminal).join("\n");
        assert!(all.contains("Loading / main"), "{all}");
        assert!(all.contains("Catching up"), "{all}");
        assert!(all.contains("of 3d 04h"), "{all}");
    }

    #[test]
    fn progress_redraws_as_time_is_reached() {
        let (mut state, now) = away_state();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        run(
            &mut terminal,
            &mut state,
            now,
            "main",
            true,
            &Theme::default(),
            Duration::ZERO,
        );
        let all = text(&terminal).join("\n");
        assert!(!all.contains("Catching up  0s of"), "{all}");
    }

    #[test]
    fn no_gap_draws_nothing() {
        let (mut state, _) = away_state();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let now = state.last_tick_unix;
        run(
            &mut terminal,
            &mut state,
            now,
            "main",
            true,
            &Theme::default(),
            Duration::ZERO,
        );
        assert!(!text(&terminal).join("\n").contains("Catching up"));
    }
}
