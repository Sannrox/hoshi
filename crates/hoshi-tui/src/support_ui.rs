use crate::support::{self, Report};
use ratatui::{
    Frame,
    crossterm::event::KeyCode,
    layout::Rect,
    style::{Color, Style},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use std::path::{Path, PathBuf};

pub struct SupportUi {
    save: PathBuf,
    balance: Option<PathBuf>,
    report: Report,
    archive: bool,
    scroll: u16,
    status: String,
    written: bool,
}

impl SupportUi {
    pub fn open(save: &Path) -> Result<Self, String> {
        let args: Vec<String> = std::env::args().collect();
        let balance = crate::balance_override(crate::parse_path_flag(args.iter(), "--balance"));
        Ok(Self {
            save: save.to_path_buf(),
            report: support::build(save, false, 100, balance.as_deref(), true, None)?,
            balance,
            archive: false,
            scroll: 0,
            status: String::new(),
            written: false,
        })
    }

    /// Return true when the overlay should close.
    pub fn handle_key(&mut self, key: KeyCode) -> bool {
        match key {
            KeyCode::Esc => return true,
            KeyCode::Enter if self.written => return true,
            KeyCode::Enter => {
                let out = support::default_out(&self.save, self.archive);
                match support::write(&self.report, &out, self.archive) {
                    Ok(()) => {
                        self.status = format!("Report saved: {}", out.display());
                        self.written = true;
                    }
                    Err(error) => self.status = format!("Could not write report: {error}"),
                }
            }
            KeyCode::Char(' ') if !self.written => {
                match support::build(
                    &self.save,
                    !self.report.includes_save,
                    100,
                    self.balance.as_deref(),
                    true,
                    None,
                ) {
                    Ok(report) => {
                        self.archive |= report.includes_save;
                        self.report = report;
                        self.scroll = 0;
                        self.status.clear();
                    }
                    Err(error) => self.status = error,
                }
            }
            KeyCode::Char('f') if !self.written => {
                if self.report.includes_save {
                    self.status = "Exclude the save to select Markdown.".into();
                } else {
                    self.archive = !self.archive;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => self.scroll = self.scroll.saturating_add(1),
            KeyCode::Up | KeyCode::Char('k') => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::PageDown => self.scroll = self.scroll.saturating_add(10),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::Home => self.scroll = 0,
            _ => {}
        }
        false
    }

    pub fn draw(&self, frame: &mut Frame<'_>) {
        let area = frame.area();
        let width = area.width.saturating_sub(4).min(100);
        let height = area.height.saturating_sub(2);
        if width < 10 || height < 8 {
            return;
        }
        let rect = Rect::new(
            (area.width - width) / 2,
            (area.height - height) / 2,
            width,
            height,
        );
        let style = Style::default().fg(Color::White).bg(Color::Black);
        frame.render_widget(Clear, rect);
        frame.render_widget(
            Block::default()
                .style(style)
                .borders(Borders::ALL)
                .title("Support report   Esc close"),
            rect,
        );
        let body = Rect::new(rect.x + 2, rect.y + 1, rect.width - 4, rect.height - 6);
        frame.render_widget(
            Paragraph::new(self.report.markdown.as_str())
                .style(style)
                .wrap(Wrap { trim: false })
                .scroll((self.scroll, 0)),
            body,
        );
        let content = if self.written {
            format!("{}\nEnter close", self.status)
        } else {
            format!(
                "[{}] Include save   Format: {}\n{}\nEnter write report   Space include save   f format   j/k scroll\n{}",
                if self.report.includes_save { "x" } else { " " },
                if self.archive { "Archive" } else { "Markdown" },
                if self.report.includes_save {
                    "Includes galaxy seed and world state."
                } else {
                    "Local files only. Nothing is uploaded."
                },
                self.status
            )
        };
        frame.render_widget(
            Paragraph::new(content)
                .style(style)
                .wrap(Wrap { trim: false }),
            Rect::new(rect.x + 2, rect.y + rect.height - 5, rect.width - 4, 4),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn preview_at_small_terminal_scrolls_and_closes_without_writing() {
        let mut report = support::build(
            Path::new("unused-support-preview/save.sqlite"),
            false,
            0,
            None,
            false,
            None,
        )
        .unwrap();
        report.markdown = (0..80).map(|n| format!("Diagnostic line {n}\n")).collect();
        let mut ui = SupportUi {
            save: PathBuf::from("unused.sqlite"),
            balance: None,
            report,
            archive: false,
            scroll: 0,
            status: String::new(),
            written: false,
        };
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| ui.draw(frame)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Support report") && text.contains("Diagnostic line 0"));
        assert!(text.contains("Enter write report") && text.contains("Nothing is uploaded"));
        assert!(!ui.handle_key(KeyCode::PageDown));
        terminal.draw(|frame| ui.draw(frame)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Diagnostic line 10"));
        assert!(ui.handle_key(KeyCode::Esc));
        assert!(!ui.written);
    }
}
