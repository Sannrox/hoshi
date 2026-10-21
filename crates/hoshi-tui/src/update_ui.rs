//! Interactive release notices. Network work remains in the background session.
use crate::update::{self, CheckCache, CheckSession};
use ratatui::crossterm::event::KeyCode;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use std::path::PathBuf;

pub struct UpdateUi {
    dir: PathBuf,
    session: Option<CheckSession>,
    dismissed: Option<String>,
    disclosure: bool,
    disclosure_recorded: bool,
    warning: Option<String>,
    initial_installed: Option<String>,
    document: Option<(String, String)>,
    scroll: u16,
}

impl UpdateUi {
    pub fn start(
        dir: PathBuf,
        config: &crate::config::UserConfig,
        args: &[String],
        now: u64,
    ) -> Self {
        let settings = update::settings(config, args);
        let mut document = None;
        let mut disclosure = false;
        let mut config_warning = None;
        let cache = CheckCache::modify(&dir, |cache| {
            disclosure = !cache.disclosure_seen && settings.enabled && !settings.offline;
            use sha2::{Digest, Sha256};
            let fingerprint = format!(
                "{:x}",
                Sha256::digest(config.unknown_lines.join("\n").as_bytes())
            );
            if !config.unknown_lines.is_empty()
                && cache.warned_unknown_config.as_deref() != Some(fingerprint.as_str())
            {
                config_warning = Some(
                    "Config contains keys from another build. They have been preserved.".to_owned(),
                );
                cache.warned_unknown_config = Some(fingerprint);
            }

            if let Some(previous) = &cache.last_seen_version
                && update::newer(update::running_version(), previous)
            {
                let changes =
                    update::changelog_since(update::CHANGELOG, previous, update::running_version());
                let changes = if config.collapse_changelog || changes.is_empty() {
                    format!(
                        "Updated to v{}. Press ? then c for Changelog.",
                        update::running_version()
                    )
                } else {
                    changes
                };
                if !changes.is_empty() {
                    document = Some(("What's new".into(), changes));
                }
            }
            if cache
                .last_seen_version
                .as_ref()
                .is_none_or(|previous| update::newer(update::running_version(), previous))
            {
                cache.last_seen_version = Some(update::running_version().into());
            }
        });
        let (dismissed, warning) = match cache {
            Ok(cache) => (cache.dismissed_version, config_warning),
            Err(error) => (
                None,
                Some(format!("Update state could not be saved: {error}")),
            ),
        };
        let session = update::public_key()
            .ok()
            .map(|key| CheckSession::start(dir.clone(), settings, key, now));
        Self {
            dir,
            session,
            dismissed,
            disclosure,
            disclosure_recorded: false,
            warning,
            initial_installed: crate::managed_update::installed_version(),
            document,
            scroll: 0,
        }
    }

    pub fn did_draw(&mut self) {
        if self.disclosure && !self.disclosure_recorded {
            match CheckCache::modify(&self.dir, |cache| cache.disclosure_seen = true) {
                Ok(_) => self.disclosure_recorded = true,
                Err(error) => {
                    self.warning = Some(format!("Could not record update disclosure: {error}"))
                }
            }
        }
    }

    pub fn overlay_open(&self) -> bool {
        self.document.is_some()
    }

    pub fn open_changelog(&mut self) {
        self.document = Some(("Changelog".into(), update::CHANGELOG.into()));
        self.scroll = 0;
    }

    pub fn handle_key(&mut self, key: KeyCode) -> bool {
        if self.document.is_some() {
            match key {
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('?') => self.document = None,
                KeyCode::Down | KeyCode::Char('j') => self.scroll = self.scroll.saturating_add(1),
                KeyCode::Up | KeyCode::Char('k') => self.scroll = self.scroll.saturating_sub(1),
                KeyCode::PageDown => self.scroll = self.scroll.saturating_add(10),
                KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(10),
                KeyCode::Home => self.scroll = 0,
                _ => {}
            }
            return true;
        }
        false
    }

    pub fn dismiss_available(&mut self) {
        if let Some(release) = self.available() {
            match CheckCache::modify(&self.dir, |cache| {
                cache.dismissed_version = Some(release.version.clone())
            }) {
                Ok(_) => self.dismissed = Some(release.version),
                Err(error) => self.warning = Some(format!("Could not dismiss update: {error}")),
            }
        }
    }

    fn available(&self) -> Option<update::Release> {
        self.session.as_ref()?.release().filter(|release| {
            release.channel == update::channel()
                && update::newer(&release.version, update::running_version())
                && self.dismissed.as_deref() != Some(&release.version)
        })
    }

    pub fn draw(&self, frame: &mut Frame<'_>) {
        let area = frame.area();
        let style = Style::default().fg(Color::White).bg(Color::Black);
        let mut messages = Vec::new();
        if let Some(installed) = crate::managed_update::installed_version()
            && self
                .initial_installed
                .as_deref()
                .is_some_and(|initial| initial != installed)
        {
            messages.push(format!("hoshi was updated to {installed} while this session was running ({}). Restart to use it; your save is safe.", update::running_version()));
        }
        if self.disclosure {
            messages.push("Update check on: sends version, OS and architecture; cached 24h. Disable: --offline, HOSHI_OFFLINE=1, HOSHI_SKIP_VERSION_CHECK=1 or update_check=false.".to_owned());
        }
        if let Some(warning) = &self.warning {
            messages.push(warning.clone());
        }
        if let Some(release) = self.available() {
            messages.push(format!(
                "hoshi {} is available. Run hoshi update. Help: u dismiss",
                release.version
            ));
            if let Some(note) = release.note {
                messages.push(render_markdown(&note));
            }
            messages.push(format!(
                "https://github.com/Sannrox/hoshi/releases/tag/v{}",
                release.version.trim_start_matches('v')
            ));
        }
        if !messages.is_empty() && area.width > 6 && area.height > 6 {
            let text = messages.join("\n");
            let width = area.width.saturating_sub(4).min(110);
            let inner_width = usize::from(width.saturating_sub(2)).max(1);
            let lines: usize = text
                .lines()
                .map(|line| line.chars().count().div_ceil(inner_width).max(1))
                .sum();
            let height = (lines as u16 + 2).min(area.height / 2);
            let rect = Rect::new(
                area.x + 2,
                area.bottom().saturating_sub(height + 2),
                width,
                height,
            );
            frame.render_widget(Clear, rect);
            frame.render_widget(
                Paragraph::new(text)
                    .style(style)
                    .wrap(Wrap { trim: false })
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title(if self.available().is_some() {
                                "Update available"
                            } else {
                                "Updates"
                            })
                            .style(style),
                    ),
                rect,
            );
        }
        if let Some((title, text)) = &self.document {
            let rect = Rect::new(
                area.x + 2,
                area.y + 1,
                area.width.saturating_sub(4),
                area.height.saturating_sub(2),
            );
            frame.render_widget(Clear, rect);
            frame.render_widget(
                Paragraph::new(render_markdown(text))
                    .style(style)
                    .wrap(Wrap { trim: false })
                    .scroll((self.scroll, 0))
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title(title.as_str())
                            .title_bottom("↑↓ scroll  esc close")
                            .style(style),
                    ),
                rect,
            );
        }
    }
}

fn render_markdown(source: &str) -> String {
    use pulldown_cmark::{Event, Tag, TagEnd};
    let mut text = String::new();
    for event in pulldown_cmark::Parser::new(source) {
        match event {
            Event::Text(value) | Event::Code(value) => text.push_str(&value),
            Event::Start(Tag::Item) => text.push_str("• "),
            Event::End(
                TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::Item | TagEnd::CodeBlock,
            )
            | Event::HardBreak => text.push('\n'),
            Event::SoftBreak => text.push(' '),
            Event::Rule => text.push('\n'),
            _ => {}
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn text(ui: &UpdateUi) -> String {
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
    fn unknown_config_warning_and_upgrade_log_are_once_only_and_changelog_stays_accessible() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("hoshi-update-ui-{}-{nonce}", std::process::id()));
        let config = crate::config::UserConfig {
            update_check: false,
            unknown_lines: vec![r#"future_secret = "preserve""#.into()],
            ..Default::default()
        };
        let mut first = UpdateUi::start(dir.clone(), &config, &[], 100);
        assert!(!first.overlay_open());
        assert!(text(&first).contains("keys from another build"));
        first.open_changelog();
        assert!(text(&first).contains("Changelog"));
        assert!(first.handle_key(KeyCode::Esc));
        assert!(!first.overlay_open());
        assert!(
            !std::fs::read_to_string(dir.join("update-state.json"))
                .unwrap()
                .contains("preserve")
        );
        let restart = UpdateUi::start(dir.clone(), &config, &[], 101);
        assert!(!restart.overlay_open());
        assert!(restart.warning.is_none());
        CheckCache::modify(&dir, |cache| cache.last_seen_version = Some("0.0.1".into())).unwrap();
        let upgraded = UpdateUi::start(dir.clone(), &config, &[], 102);
        assert!(upgraded.overlay_open());
        assert!(text(&upgraded).contains("What's new"));
        assert!(!UpdateUi::start(dir.clone(), &config, &[], 103).overlay_open());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
