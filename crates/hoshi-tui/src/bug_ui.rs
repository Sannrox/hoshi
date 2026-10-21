use crate::bug::{self, Fields, Prepared};
use ratatui::{
    Frame,
    crossterm::event::KeyCode,
    layout::Rect,
    style::{Color, Style},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use std::path::{Path, PathBuf};

pub struct BugUi {
    save: PathBuf,
    config_dir: PathBuf,
    target: String,
    fields: Fields,
    selected: usize,
    preview: Option<Prepared>,
    scroll: u16,
    status: String,
    saved: bool,
}

impl BugUi {
    pub fn open(save: &Path, config_dir: &Path, crash: bool) -> Self {
        let target = crate::config::UserConfig::load(config_dir).bug_report_target;
        Self {
            save: save.into(),
            config_dir: config_dir.into(),
            target,
            fields: Fields {
                title: if crash {
                    "Crash while playing".into()
                } else {
                    String::new()
                },
                ..Default::default()
            },
            selected: 0,
            preview: None,
            scroll: 0,
            status: String::new(),
            saved: false,
        }
    }

    pub fn handle_key(&mut self, key: KeyCode) -> bool {
        if self.saved {
            return matches!(key, KeyCode::Esc | KeyCode::Enter);
        }
        if let Some(prepared) = &self.preview {
            match key {
                KeyCode::Esc => {
                    self.preview = None;
                    self.scroll = 0;
                    self.status.clear();
                }
                KeyCode::Enter | KeyCode::Char('s') => {
                    self.scroll = 0;
                    match bug::save(prepared, &self.save) {
                        Ok(path) => {
                            self.status = format!("Report and URL saved: {}", path.display());
                            self.saved = true;
                            if key == KeyCode::Enter
                                && !bug::offline()
                                && let Err(error) = bug::open_browser(&prepared.url)
                            {
                                self.status.push_str(&format!("\n{error}"));
                            }
                        }
                        Err(error) => self.status = error,
                    }
                }
                KeyCode::Down => self.scroll = self.scroll.saturating_add(1),
                KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
                KeyCode::PageDown => self.scroll = self.scroll.saturating_add(10),
                KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(10),
                _ => {}
            }
            return false;
        }
        match key {
            KeyCode::Esc => return true,
            KeyCode::Tab | KeyCode::Down => self.selected = (self.selected + 1) % 4,
            KeyCode::BackTab | KeyCode::Up => self.selected = (self.selected + 3) % 4,
            KeyCode::Enter => {
                match bug::prepare(&self.save, &self.config_dir, &self.fields, &self.target) {
                    Ok(prepared) => {
                        self.preview = Some(prepared);
                        self.scroll = 0;
                        self.status.clear();
                    }
                    Err(error) => self.status = error,
                }
            }
            KeyCode::Backspace => {
                self.field().pop();
            }
            KeyCode::Char(ch) if self.field().len() < 8_000 => self.field().push(ch),
            _ => {}
        }
        false
    }

    fn field(&mut self) -> &mut String {
        match self.selected {
            0 => &mut self.fields.title,
            1 => &mut self.fields.happened,
            2 => &mut self.fields.expected,
            _ => &mut self.fields.steps,
        }
    }

    pub fn draw(&self, frame: &mut Frame<'_>) {
        let area = frame.area();
        let width = area.width.saturating_sub(4).min(100);
        let height = area.height.saturating_sub(2);
        if width < 10 || height < 8 {
            return;
        }
        let rect = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        let text = if let Some(prepared) = &self.preview {
            format!(
                "{}\n{}\n\n{}\n\n{}",
                if self.saved {
                    "Saved locally"
                } else {
                    "Review bug report"
                },
                self.status,
                prepared
                    .search_url
                    .as_ref()
                    .map(|url| format!("Search existing issues: {url}"))
                    .unwrap_or_default(),
                prepared.report.markdown
            )
        } else {
            let mut text = String::new();
            for (index, (label, value)) in [
                ("Title", &self.fields.title),
                ("What happened", &self.fields.happened),
                ("Expected", &self.fields.expected),
                ("Steps (optional)", &self.fields.steps),
            ]
            .iter()
            .enumerate()
            {
                text.push_str(&format!(
                    "{} {label}\n{value}\n\n",
                    if index == self.selected { ">" } else { " " }
                ));
            }
            text.push_str(&self.status);
            text
        };
        let footer = if self.saved {
            "Enter / Esc close"
        } else if self.preview.is_some() {
            "Enter save + open page · s save for later · Esc edit"
        } else {
            "Tab next · Enter preview · Esc cancel"
        };
        frame.render_widget(Clear, rect);
        frame.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: false })
                .scroll((self.scroll, 0))
                .style(Style::default().fg(Color::White).bg(Color::Black))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("Report a bug")
                        .title_bottom(footer),
                ),
            rect,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn text(ui: &BugUi) -> String {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| ui.draw(frame)).unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn form_preview_and_cancel_never_publish() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("hoshi-bug-ui-{}-{nonce}", std::process::id()));
        let path = dir.join("save.sqlite");
        std::fs::create_dir(&dir).unwrap();
        let mut ui = BugUi::open(&path, &dir, false);
        assert!(text(&ui).contains("What happened"));
        for word in ["Failure", "Stopped", "Continue"] {
            for ch in word.chars() {
                ui.handle_key(KeyCode::Char(ch));
            }
            ui.handle_key(KeyCode::Tab);
        }
        ui.handle_key(KeyCode::Enter);
        assert!(ui.preview.is_some());
        assert!(text(&ui).contains("Review bug report"));
        assert!(!dir.join("reports").exists());
        ui.handle_key(KeyCode::PageDown);
        ui.handle_key(KeyCode::Esc);
        assert!(ui.preview.is_none());
        assert!(text(&ui).contains("What happened"));
        ui.handle_key(KeyCode::Enter);
        ui.handle_key(KeyCode::PageDown);
        ui.handle_key(KeyCode::Char('s'));
        assert!(text(&ui).contains("Report and URL saved"));
        assert!(ui.handle_key(KeyCode::Esc));
        let crash = BugUi::open(&path, &dir, true);
        assert!(text(&crash).contains("Crash while playing"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
