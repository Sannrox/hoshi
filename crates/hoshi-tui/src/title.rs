//! Title screen: continue, new game, load, settings. No game rules live here.

use crate::config::UserConfig;
use hoshi_core::{SaveError, SaveGlance, inspect_save, lock_save, save_is_locked};
use ratatui::Frame;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use std::path::{Path, PathBuf};

const MENU: [&str; 6] = [
    "Continue",
    "New game",
    "Load game",
    "Settings",
    "Credits",
    "Quit",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleAction {
    None,
    Quit,
    Play(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Menu,
    New,
    Load,
    Settings,
    Credits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Difficulty {
    Easy,
    Normal,
    Hard,
}

impl Difficulty {
    fn as_label(self) -> &'static str {
        match self {
            Difficulty::Easy => "easy",
            Difficulty::Normal => "normal",
            Difficulty::Hard => "hard",
        }
    }

    fn cycle(self) -> Self {
        match self {
            Difficulty::Easy => Difficulty::Normal,
            Difficulty::Normal => Difficulty::Hard,
            Difficulty::Hard => Difficulty::Easy,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NewField {
    Name,
    Seed,
    Difficulty,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    pub name: String,
    pub path: PathBuf,
    pub status: SlotStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlotStatus {
    Ready(SaveGlance),
    Locked,
    Newer { found: u32 },
    Corrupt(String),
    Harness(SaveGlance),
}

impl SlotStatus {
    fn line(&self, now_unix: u64) -> String {
        match self {
            SlotStatus::Ready(g) | SlotStatus::Harness(g) => {
                let ago = now_unix.saturating_sub(g.last_tick_unix);
                let kind = if matches!(self, SlotStatus::Harness(_)) {
                    "harness save (CLI only)"
                } else {
                    "Humans"
                };
                format!(
                    "{kind}  planets {}  fleet {}  {ago}s ago",
                    g.planets, g.fleet_strength
                )
            }
            SlotStatus::Locked => "in use by another hoshi process".into(),
            SlotStatus::Newer { found } => {
                format!("made by a newer hoshi (schema {found}); please update")
            }
            SlotStatus::Corrupt(msg) => format!("corrupt ({msg})"),
        }
    }

    fn playable(&self) -> bool {
        matches!(self, SlotStatus::Ready(_))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AwaySummary {
    pub elapsed_secs: u64,
    pub metal: f64,
    pub crystal: f64,
    pub deuterium: f64,
    pub builds: u32,
    pub research: u32,
    pub fleets_returned: u32,
    pub expeditions: u32,
    pub raids: u32,
    pub events: Vec<String>,
}

pub struct Title {
    dir: PathBuf,
    pub config: UserConfig,
    page: Page,
    selected: usize,
    slots: Vec<Slot>,
    status: String,
    tick: u64,
    now_unix: u64,
    new_name: String,
    new_seed: String,
    new_difficulty: Difficulty,
    new_field: NewField,
    confirm_delete: Option<String>,
    rename_from: Option<String>,
    rename_to: String,
    play_path: Option<PathBuf>,
}

impl Title {
    pub fn new(dir: PathBuf, now_unix: u64) -> Self {
        std::fs::create_dir_all(&dir).ok();
        let config = UserConfig::load(&dir);
        let mut title = Self {
            dir,
            config,
            page: Page::Menu,
            selected: 0,
            slots: Vec::new(),
            status: String::new(),
            tick: 0,
            now_unix,
            new_name: "save".into(),
            new_seed: String::new(),
            new_difficulty: Difficulty::Normal,
            new_field: NewField::Name,
            confirm_delete: None,
            rename_from: None,
            rename_to: String::new(),
            play_path: None,
        };
        title.refresh_slots();
        title
    }

    pub fn take_play_path(&mut self) -> Option<PathBuf> {
        self.play_path.take()
    }

    pub fn tick(&mut self) {
        if !self.config.reduced_motion {
            self.tick = self.tick.saturating_add(1);
        }
    }

    pub fn set_now(&mut self, now_unix: u64) {
        self.now_unix = now_unix;
    }

    fn refresh_slots(&mut self) {
        self.slots = list_slots(&self.dir, self.now_unix);
    }

    pub fn handle_key(&mut self, code: KeyCode) -> TitleAction {
        match self.page {
            Page::Menu => self.handle_menu(code),
            Page::New => self.handle_new(code),
            Page::Load => self.handle_load(code),
            Page::Settings => self.handle_settings(code),
            Page::Credits => {
                if matches!(code, KeyCode::Esc | KeyCode::Char('x') | KeyCode::Enter) {
                    self.page = Page::Menu;
                }
                TitleAction::None
            }
        }
    }

    fn handle_menu(&mut self, code: KeyCode) -> TitleAction {
        match code {
            KeyCode::Char('q') => TitleAction::Quit,
            KeyCode::Char('j') | KeyCode::Down => {
                self.selected = (self.selected + 1) % MENU.len();
                TitleAction::None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.selected = (self.selected + MENU.len() - 1) % MENU.len();
                TitleAction::None
            }
            KeyCode::Enter => match self.selected {
                0 => self.continue_last(),
                1 => {
                    self.page = Page::New;
                    self.status.clear();
                    self.pick_default_name();
                    TitleAction::None
                }
                2 => {
                    self.refresh_slots();
                    self.page = Page::Load;
                    self.selected = 0;
                    self.status.clear();
                    TitleAction::None
                }
                3 => {
                    self.page = Page::Settings;
                    self.selected = 0;
                    TitleAction::None
                }
                4 => {
                    self.page = Page::Credits;
                    TitleAction::None
                }
                _ => TitleAction::Quit,
            },
            _ => TitleAction::None,
        }
    }

    fn continue_last(&mut self) -> TitleAction {
        self.refresh_slots();
        let want = self.config.last_slot.clone();
        let slot = want
            .as_ref()
            .and_then(|name| self.slots.iter().find(|s| s.name == *name))
            .or_else(|| self.slots.iter().find(|s| s.status.playable()))
            .cloned();
        match slot {
            Some(slot) => self.try_play(&slot),
            None => {
                self.status = "no save to continue".into();
                TitleAction::None
            }
        }
    }

    fn try_play(&mut self, slot: &Slot) -> TitleAction {
        match &slot.status {
            SlotStatus::Ready(_) => {
                self.config.last_slot = Some(slot.name.clone());
                let _ = self.config.save(&self.dir);
                self.play_path = Some(slot.path.clone());
                TitleAction::Play(
                    self.slots
                        .iter()
                        .position(|s| s.name == slot.name)
                        .unwrap_or(0),
                )
            }
            SlotStatus::Locked => {
                self.status = format!(
                    "{}: in use by another hoshi process (retry or back)",
                    slot.name
                );
                TitleAction::None
            }
            SlotStatus::Newer { found } => {
                self.status = format!(
                    "{}: made by a newer hoshi (schema {found}); please update",
                    slot.name
                );
                TitleAction::None
            }
            SlotStatus::Corrupt(msg) => {
                self.status = format!("{}: corrupt ({msg})", slot.name);
                TitleAction::None
            }
            SlotStatus::Harness(_) => {
                self.status = format!("{}: harness save (CLI only)", slot.name);
                TitleAction::None
            }
        }
    }

    fn pick_default_name(&mut self) {
        if !self.slots.iter().any(|s| s.name == "save") {
            self.new_name = "save".into();
            return;
        }
        for i in 2..100 {
            let name = format!("save{i}");
            if !self.slots.iter().any(|s| s.name == name) {
                self.new_name = name;
                return;
            }
        }
        self.new_name = "save".into();
    }

    fn handle_new(&mut self, code: KeyCode) -> TitleAction {
        match code {
            KeyCode::Esc | KeyCode::Char('x') => {
                self.page = Page::Menu;
                TitleAction::None
            }
            KeyCode::Tab | KeyCode::Down => {
                self.new_field = match self.new_field {
                    NewField::Name => NewField::Seed,
                    NewField::Seed => NewField::Difficulty,
                    NewField::Difficulty => NewField::Name,
                };
                TitleAction::None
            }
            KeyCode::Up => {
                self.new_field = match self.new_field {
                    NewField::Name => NewField::Difficulty,
                    NewField::Seed => NewField::Name,
                    NewField::Difficulty => NewField::Seed,
                };
                TitleAction::None
            }
            KeyCode::Enter => {
                if self.new_field == NewField::Difficulty {
                    return self.create_new();
                }
                self.new_field = match self.new_field {
                    NewField::Name => NewField::Seed,
                    NewField::Seed => NewField::Difficulty,
                    NewField::Difficulty => NewField::Difficulty,
                };
                TitleAction::None
            }
            KeyCode::Char(' ') if self.new_field == NewField::Difficulty => {
                self.new_difficulty = self.new_difficulty.cycle();
                TitleAction::None
            }
            KeyCode::Backspace => {
                match self.new_field {
                    NewField::Name => {
                        self.new_name.pop();
                    }
                    NewField::Seed => {
                        self.new_seed.pop();
                    }
                    NewField::Difficulty => {}
                }
                TitleAction::None
            }
            KeyCode::Char(c) if !c.is_control() => {
                match self.new_field {
                    NewField::Name => {
                        if self.new_name.len() < 24
                            && (c.is_ascii_alphanumeric() || c == '-' || c == '_')
                        {
                            self.new_name.push(c);
                        }
                    }
                    NewField::Seed => {
                        if self.new_seed.len() < 32 {
                            self.new_seed.push(c);
                        }
                    }
                    NewField::Difficulty => {}
                }
                TitleAction::None
            }
            _ => TitleAction::None,
        }
    }

    fn create_new(&mut self) -> TitleAction {
        let name = self.new_name.trim();
        if name.is_empty() {
            self.status = "name required".into();
            return TitleAction::None;
        }
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            self.status = "invalid slot name".into();
            return TitleAction::None;
        }
        if let Err(msg) = parse_seed(&self.new_seed) {
            self.status = msg;
            return TitleAction::None;
        }
        let path = self.dir.join(format!("{name}.sqlite"));
        if path.exists() {
            self.status = format!("{name} already exists");
            return TitleAction::None;
        }
        self.write_slot_meta(name);
        self.config.last_slot = Some(name.to_string());
        let _ = self.config.save(&self.dir);
        self.play_path = Some(path);
        TitleAction::Play(0)
    }

    fn write_slot_meta(&self, name: &str) {
        let seed = self.new_seed.trim();
        let text = format!(
            "name = \"{name}\"\nspecies = \"Humans\"\ndifficulty = \"{}\"\nseed = \"{seed}\"\n",
            self.new_difficulty.as_label()
        );
        let _ = std::fs::write(self.dir.join(format!("{name}.slot.toml")), text);
    }

    fn handle_load(&mut self, code: KeyCode) -> TitleAction {
        if let Some(from) = self.rename_from.clone() {
            return self.handle_rename(code, from);
        }
        if let Some(name) = self.confirm_delete.clone() {
            return self.handle_delete_confirm(code, name);
        }
        if self.slots.is_empty() {
            if matches!(code, KeyCode::Esc | KeyCode::Char('x')) {
                self.page = Page::Menu;
            }
            return TitleAction::None;
        }
        match code {
            KeyCode::Esc | KeyCode::Char('x') => {
                self.page = Page::Menu;
                self.selected = 2;
                TitleAction::None
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.selected = (self.selected + 1) % self.slots.len();
                TitleAction::None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.selected = (self.selected + self.slots.len() - 1) % self.slots.len();
                TitleAction::None
            }
            KeyCode::Enter => {
                let slot = self.slots[self.selected].clone();
                self.try_play(&slot)
            }
            KeyCode::Char('d') => {
                let name = self.slots[self.selected].name.clone();
                if save_is_locked(&self.slots[self.selected].path) {
                    self.status = format!("{name} is locked");
                    return TitleAction::None;
                }
                self.confirm_delete = Some(name);
                self.status = "delete: press d again or type the slot name and Enter".into();
                TitleAction::None
            }
            KeyCode::Char('r') => {
                let slot = &self.slots[self.selected];
                if save_is_locked(&slot.path) {
                    self.status = format!("{} is locked", slot.name);
                    return TitleAction::None;
                }
                self.rename_from = Some(slot.name.clone());
                self.rename_to = slot.name.clone();
                self.status = "rename: edit name and Enter".into();
                TitleAction::None
            }
            _ => TitleAction::None,
        }
    }

    fn handle_delete_confirm(&mut self, code: KeyCode, name: String) -> TitleAction {
        match code {
            KeyCode::Esc | KeyCode::Char('x') => {
                self.confirm_delete = None;
                self.status.clear();
            }
            KeyCode::Char('d') | KeyCode::Enter => {
                if let Err(msg) = self.delete_slot(&name) {
                    self.status = msg;
                } else {
                    self.status = format!("deleted {name}");
                    self.refresh_slots();
                    if self.selected >= self.slots.len() {
                        self.selected = self.slots.len().saturating_sub(1);
                    }
                }
                self.confirm_delete = None;
            }
            _ => {}
        }
        TitleAction::None
    }

    fn handle_rename(&mut self, code: KeyCode, from: String) -> TitleAction {
        match code {
            KeyCode::Esc | KeyCode::Char('x') => {
                self.rename_from = None;
                self.status.clear();
            }
            KeyCode::Backspace => {
                self.rename_to.pop();
            }
            KeyCode::Enter => {
                if let Err(msg) = self.rename_slot(&from, &self.rename_to.clone()) {
                    self.status = msg;
                } else {
                    self.status = format!("renamed {from}");
                    self.refresh_slots();
                }
                self.rename_from = None;
            }
            KeyCode::Char(c)
                if (c.is_ascii_alphanumeric() || c == '-' || c == '_')
                    && self.rename_to.len() < 24 =>
            {
                self.rename_to.push(c);
            }
            _ => {}
        }
        TitleAction::None
    }

    fn delete_slot(&mut self, name: &str) -> Result<(), String> {
        let path = self.dir.join(format!("{name}.sqlite"));
        let lock = match lock_save(&path) {
            Ok(lock) => lock,
            Err(SaveError::Locked) => return Err(format!("{name} is locked")),
            Err(e) => return Err(e.to_string()),
        };
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
        let _ = std::fs::remove_file(wal_path(&path));
        let _ = std::fs::remove_file(shm_path(&path));
        let _ = std::fs::remove_file(self.dir.join(format!("{name}.slot.toml")));
        drop(lock);
        if self.config.last_slot.as_deref() == Some(name) {
            self.config.last_slot = None;
            let _ = self.config.save(&self.dir);
        }
        Ok(())
    }

    fn rename_slot(&mut self, from: &str, to: &str) -> Result<(), String> {
        let to = to.trim();
        if to.is_empty()
            || !to
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err("invalid slot name".into());
        }
        let src = self.dir.join(format!("{from}.sqlite"));
        let dst = self.dir.join(format!("{to}.sqlite"));
        if dst.exists() {
            return Err(format!("{to} already exists"));
        }
        let lock = match lock_save(&src) {
            Ok(lock) => lock,
            Err(SaveError::Locked) => return Err(format!("{from} is locked")),
            Err(e) => return Err(e.to_string()),
        };
        std::fs::rename(&src, &dst).map_err(|e| e.to_string())?;
        if let Err(e) = rename_sidecar(&wal_path(&src), &wal_path(&dst)) {
            let _ = std::fs::rename(&dst, &src);
            return Err(e);
        }
        if let Err(e) = rename_sidecar(&shm_path(&src), &shm_path(&dst)) {
            let _ = rename_sidecar(&wal_path(&dst), &wal_path(&src));
            let _ = std::fs::rename(&dst, &src);
            return Err(e);
        }
        let _ = std::fs::rename(
            self.dir.join(format!("{from}.slot.toml")),
            self.dir.join(format!("{to}.slot.toml")),
        );
        drop(lock);
        if self.config.last_slot.as_deref() == Some(from) {
            self.config.last_slot = Some(to.to_string());
            let _ = self.config.save(&self.dir);
        }
        Ok(())
    }

    fn handle_settings(&mut self, code: KeyCode) -> TitleAction {
        let items = 4;
        match code {
            KeyCode::Esc | KeyCode::Char('x') => {
                self.page = Page::Menu;
                self.selected = 3;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.selected = (self.selected + 1) % items;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.selected = (self.selected + items - 1) % items;
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                match self.selected {
                    0 => self.config.reduced_motion = !self.config.reduced_motion,
                    1 => self.config.high_contrast = !self.config.high_contrast,
                    2 => self.config.bridge = !self.config.bridge,
                    3 => self.config.ai_advisor = !self.config.ai_advisor,
                    _ => {}
                }
                let _ = self.config.save(&self.dir);
            }
            _ => {}
        }
        TitleAction::None
    }

    pub fn draw(&self, frame: &mut Frame<'_>) {
        let area = frame.area();
        let contrast = if self.config.high_contrast {
            Style::default().fg(Color::White)
        } else {
            Style::default()
        };
        let block = Block::bordered().title(" hoshi ").style(contrast);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let mut lines: Vec<Line> = Vec::new();
        if inner.width >= 40 && inner.height >= 10 {
            lines.push(Line::from(Span::styled(
                "hoshi",
                Style::default().add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
        }
        if !self.config.reduced_motion {
            lines.push(Line::from(star_row(inner.width, self.tick)));
        } else {
            lines.push(Line::from(star_row(inner.width, 0)));
        }
        lines.push(Line::from(""));
        match self.page {
            Page::Menu => {
                for (i, item) in MENU.iter().enumerate() {
                    lines.push(menu_line(item, i == self.selected));
                }
            }
            Page::New => {
                lines.push(Line::from("New game  Humans"));
                lines.push(field_line(
                    "name",
                    &self.new_name,
                    self.new_field == NewField::Name,
                ));
                lines.push(field_line(
                    "seed (blank = random)",
                    &self.new_seed,
                    self.new_field == NewField::Seed,
                ));
                lines.push(field_line(
                    "difficulty",
                    self.new_difficulty.as_label(),
                    self.new_field == NewField::Difficulty,
                ));
                lines.push(Line::from("Enter on difficulty creates the slot. x back."));
            }
            Page::Load => {
                lines.push(Line::from(
                    "Load game  Enter play  d delete  r rename  x back",
                ));
                if self.slots.is_empty() {
                    lines.push(Line::from("no saves yet"));
                }
                for (i, slot) in self.slots.iter().enumerate() {
                    let mark = if i == self.selected { ">" } else { " " };
                    lines.push(Line::from(format!(
                        "{mark} {}  {}",
                        slot.name,
                        slot.status.line(self.now_unix)
                    )));
                }
                if let Some(from) = &self.rename_from {
                    lines.push(Line::from(format!("rename {from} -> {}", self.rename_to)));
                }
            }
            Page::Settings => {
                lines.push(Line::from("Settings  space toggles  takes effect now"));
                lines.push(toggle_line(
                    "reduced motion",
                    self.config.reduced_motion,
                    self.selected == 0,
                ));
                lines.push(toggle_line(
                    "high contrast",
                    self.config.high_contrast,
                    self.selected == 1,
                ));
                lines.push(toggle_line(
                    "agent bridge (off by default)",
                    self.config.bridge,
                    self.selected == 2,
                ));
                lines.push(toggle_line(
                    "local AI advisor (off by default)",
                    self.config.ai_advisor,
                    self.selected == 3,
                ));
            }
            Page::Credits => {
                lines.push(Line::from(
                    "hoshi  an idle space-strategy game for the terminal",
                ));
                lines.push(Line::from(
                    "core owns the rules. the terminal only renders.",
                ));
                lines.push(Line::from("Enter back"));
            }
        }
        if !self.status.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                self.status.clone(),
                Style::default().add_modifier(Modifier::ITALIC),
            )));
        }
        let chunks = Layout::vertical([Constraint::Min(1)]).split(inner);
        frame.render_widget(Paragraph::new(lines), chunks[0]);
    }

    pub fn new_game_spec(&self) -> NewGameSpec {
        NewGameSpec {
            path: self
                .play_path
                .clone()
                .unwrap_or_else(|| self.dir.join(format!("{}.sqlite", self.new_name))),
            seed: parse_seed(&self.new_seed).ok().flatten(),
            difficulty: self.new_difficulty.as_label().to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct NewGameSpec {
    pub path: PathBuf,
    pub seed: Option<u64>,
    pub difficulty: String,
}

pub fn parse_seed(text: &str) -> Result<Option<u64>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    text.parse::<u64>()
        .map(Some)
        .map_err(|_| "invalid seed".to_string())
}

pub fn list_slots(dir: &Path, _now_unix: u64) -> Vec<Slot> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut slots = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.ends_with(".sqlite") {
            continue;
        }
        let stem = name.trim_end_matches(".sqlite").to_string();
        let status = classify_slot(&path);
        slots.push(Slot {
            name: stem,
            path,
            status,
        });
    }
    slots.sort_by(|a, b| a.name.cmp(&b.name));
    slots
}

pub fn classify_slot(path: &Path) -> SlotStatus {
    if save_is_locked(path) {
        return SlotStatus::Locked;
    }
    match inspect_save(path) {
        Ok(g) if g.deterministic => SlotStatus::Harness(g),
        Ok(g) => SlotStatus::Ready(g),
        Err(SaveError::Locked) => SlotStatus::Locked,
        Err(SaveError::SchemaTooNew { found, .. }) => SlotStatus::Newer { found },
        Err(SaveError::Corrupt(msg)) => SlotStatus::Corrupt(msg.to_string()),
        Err(e) => SlotStatus::Corrupt(e.to_string()),
    }
}

pub fn slot_message(status: &SlotStatus) -> String {
    match status {
        SlotStatus::Locked => "in use by another hoshi process".into(),
        SlotStatus::Newer { found } => {
            format!("made by a newer hoshi (schema {found}); please update")
        }
        SlotStatus::Corrupt(_) => "corrupt save".into(),
        SlotStatus::Harness(_) => "harness save (CLI only)".into(),
        SlotStatus::Ready(_) => String::new(),
    }
}

pub fn continue_slot<'a>(slots: &'a [Slot], last: Option<&str>) -> Option<&'a Slot> {
    last.and_then(|name| slots.iter().find(|s| s.name == name))
        .or_else(|| slots.iter().find(|s| s.status.playable()))
        .or_else(|| slots.first())
}

fn wal_path(save: &Path) -> PathBuf {
    PathBuf::from(format!("{}-wal", save.display()))
}

fn shm_path(save: &Path) -> PathBuf {
    PathBuf::from(format!("{}-shm", save.display()))
}

fn rename_sidecar(src: &Path, dst: &Path) -> Result<(), String> {
    match std::fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

fn menu_line(label: &str, selected: bool) -> Line<'static> {
    if selected {
        Line::from(Span::styled(
            format!("> {label}"),
            Style::default().add_modifier(Modifier::REVERSED),
        ))
    } else {
        Line::from(format!("  {label}"))
    }
}

fn field_line(label: &str, value: &str, selected: bool) -> Line<'static> {
    let text = format!("{label}: {value}");
    if selected {
        Line::from(Span::styled(
            text,
            Style::default().add_modifier(Modifier::REVERSED),
        ))
    } else {
        Line::from(text)
    }
}

fn toggle_line(label: &str, on: bool, selected: bool) -> Line<'static> {
    let mark = if on { "on" } else { "off" };
    let text = format!("{label}: {mark}");
    if selected {
        Line::from(Span::styled(
            text,
            Style::default().add_modifier(Modifier::REVERSED),
        ))
    } else {
        Line::from(text)
    }
}

fn star_row(width: u16, tick: u64) -> String {
    let w = width as usize;
    if w == 0 {
        return String::new();
    }
    let mut s = String::from(" ");
    for i in 0..w.saturating_sub(2) {
        let n = (i as u64).wrapping_mul(17).wrapping_add(tick / 2);
        s.push(if n.is_multiple_of(23) {
            '*'
        } else if n.is_multiple_of(11) {
            '.'
        } else {
            ' '
        });
    }
    s
}

pub fn apply_difficulty(config: &mut hoshi_core::BalanceConfig, difficulty: &str) {
    let scale: f64 = match difficulty {
        "easy" => 2.0,
        "hard" => 0.5,
        _ => 1.0,
    };
    if (scale - 1.0).abs() < f64::EPSILON {
        return;
    }
    config.starting_resources.metal *= scale;
    config.starting_resources.crystal *= scale;
    config.starting_resources.deuterium *= scale;
}

pub struct AwaySnapshot {
    pub tick: u64,
    pub metal: f64,
    pub crystal: f64,
    pub deuterium: f64,
    pub builds: u32,
    pub research: u32,
    pub events: usize,
}

pub fn snapshot_away(state: &hoshi_core::GameState) -> AwaySnapshot {
    AwaySnapshot {
        tick: state.last_tick_unix,
        metal: state.resources.metal,
        crystal: state.resources.crystal,
        deuterium: state.resources.deuterium,
        builds: building_levels(state),
        research: research_levels(state),
        events: state.events.len(),
    }
}

pub fn summarize_away(before: &AwaySnapshot, after: &hoshi_core::GameState) -> AwaySummary {
    let new_events: Vec<String> = after
        .events
        .iter()
        .skip(before.events)
        .map(|e| e.text.clone())
        .collect();
    let fleets_returned = new_events.iter().filter(|t| t.contains("return")).count() as u32;
    let expeditions = new_events
        .iter()
        .filter(|t| t.contains("expedition"))
        .count() as u32;
    let raids = new_events.iter().filter(|t| t.contains("raid")).count() as u32;
    AwaySummary {
        elapsed_secs: after.last_tick_unix.saturating_sub(before.tick),
        metal: after.resources.metal - before.metal,
        crystal: after.resources.crystal - before.crystal,
        deuterium: after.resources.deuterium - before.deuterium,
        builds: after_building_levels(after).saturating_sub(before.builds),
        research: after
            .tech_levels
            .values()
            .copied()
            .sum::<u32>()
            .saturating_sub(before.research),
        fleets_returned,
        expeditions,
        raids,
        events: new_events,
    }
}

fn after_building_levels(state: &hoshi_core::GameState) -> u32 {
    use hoshi_core::BuildingKind;
    BuildingKind::ALL
        .iter()
        .map(|&k| state.buildings.get(k))
        .sum()
}

pub fn building_levels(state: &hoshi_core::GameState) -> u32 {
    after_building_levels(state)
}

pub fn research_levels(state: &hoshi_core::GameState) -> u32 {
    state.tech_levels.values().copied().sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoshi_core::{BalanceConfig, GameState, SCHEMA_VERSION, lock_save, open_save, save_state};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_dir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static UNIQUE: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let n = UNIQUE.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("hoshi-title-{}-{nanos}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn render(title: &Title, w: u16, h: u16) -> String {
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| title.draw(f)).unwrap();
        let buf = terminal.backend().buffer();
        let area = buf.area();
        let mut lines = Vec::new();
        for y in area.y..area.y + area.height {
            let mut line = String::new();
            for x in area.x..area.x + area.width {
                line.push_str(buf[(x, y)].symbol());
            }
            lines.push(line.trim_end().to_string());
        }
        lines.join("\n")
    }

    fn write_save(dir: &Path, name: &str, state: &GameState) {
        let path = dir.join(format!("{name}.sqlite"));
        let mut conn = open_save(&path).unwrap();
        save_state(&mut conn, state).unwrap();
    }

    #[test]
    fn title_renders_at_80x24_and_continue_is_default() {
        let dir = unique_dir();
        let title = Title::new(dir.clone(), 100);
        let text = render(&title, 80, 24);
        assert!(text.contains("Continue"), "{text}");
        assert!(text.contains("New game"), "{text}");
        assert!(text.contains("Load game"), "{text}");
        assert!(text.contains("Settings"), "{text}");
        assert!(
            text.contains("> Continue") || text.contains("Continue"),
            "{text}"
        );
        let small = render(&title, 80, 24);
        let _ = small;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reduced_motion_keeps_the_starfield_still() {
        let dir = unique_dir();
        let mut title = Title::new(dir.clone(), 100);
        title.config.reduced_motion = true;
        let a = render(&title, 80, 24);
        title.tick();
        title.tick();
        let b = render(&title, 80, 24);
        assert_eq!(a, b);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn new_game_rejects_an_invalid_seed() {
        let dir = unique_dir();
        let mut title = Title::new(dir.clone(), 100);
        title.handle_key(KeyCode::Down);
        title.handle_key(KeyCode::Enter);
        title.new_field = NewField::Seed;
        for c in "abc".chars() {
            title.handle_key(KeyCode::Char(c));
        }
        title.new_field = NewField::Difficulty;
        let action = title.handle_key(KeyCode::Enter);
        assert_eq!(action, TitleAction::None);
        assert!(title.status.contains("invalid seed"), "{}", title.status);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_lists_locked_newer_corrupt_and_harness_without_playing() {
        let dir = unique_dir();
        write_save(&dir, "live", &GameState::new(10));
        write_save(
            &dir,
            "harness",
            &GameState::with_seed(10, BalanceConfig::default(), 1),
        );
        std::fs::write(dir.join("junk.sqlite"), b"nope").unwrap();
        let newer = dir.join("future.sqlite");
        {
            let conn = open_save(&newer).unwrap();
            conn.pragma_update(None, "user_version", SCHEMA_VERSION + 3)
                .unwrap();
        }
        let locked_path = dir.join("busy.sqlite");
        write_save(&dir, "busy", &GameState::new(10));
        let _holder = lock_save(&locked_path).unwrap();

        let mut title = Title::new(dir.clone(), 40);
        title.handle_key(KeyCode::Down);
        title.handle_key(KeyCode::Down);
        title.handle_key(KeyCode::Enter);
        let text = render(&title, 100, 30);
        assert!(text.contains("in use by another hoshi process"), "{text}");
        assert!(text.contains("newer hoshi"), "{text}");
        assert!(text.contains("corrupt"), "{text}");
        assert!(text.contains("harness save"), "{text}");

        let busy = title.slots.iter().find(|s| s.name == "busy").unwrap();
        assert_eq!(title.try_play(&busy.clone()), TitleAction::None);
        let harness = title.slots.iter().find(|s| s.name == "harness").unwrap();
        assert_eq!(title.try_play(&harness.clone()), TitleAction::None);
        let live = title.slots.iter().find(|s| s.name == "live").unwrap();
        assert!(matches!(
            title.try_play(&live.clone()),
            TitleAction::Play(_)
        ));
        drop(_holder);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_needs_a_second_key_and_skips_a_locked_slot() {
        let dir = unique_dir();
        write_save(&dir, "alpha", &GameState::new(1));
        write_save(&dir, "busy", &GameState::new(1));
        let holder = lock_save(&dir.join("busy.sqlite")).unwrap();
        let mut title = Title::new(dir.clone(), 1);
        title.page = Page::Load;
        title.selected = title.slots.iter().position(|s| s.name == "alpha").unwrap();
        title.handle_key(KeyCode::Char('d'));
        assert!(dir.join("alpha.sqlite").exists());
        title.handle_key(KeyCode::Char('d'));
        assert!(!dir.join("alpha.sqlite").exists());
        title.refresh_slots();
        title.selected = title.slots.iter().position(|s| s.name == "busy").unwrap();
        title.handle_key(KeyCode::Char('d'));
        assert!(dir.join("busy.sqlite").exists());
        title.rename_from = Some("busy".into());
        title.rename_to = "moved".into();
        assert!(title.rename_slot("busy", "moved").is_err());
        assert!(dir.join("busy.sqlite").exists());
        drop(holder);

        write_save(&dir, "keep", &GameState::new(1));
        title.config.last_slot = Some("keep".into());
        title.rename_slot("keep", "kept").unwrap();
        assert_eq!(title.config.last_slot.as_deref(), Some("kept"));
        assert_eq!(UserConfig::load(&dir).last_slot.as_deref(), Some("kept"));
        assert!(dir.join("kept.sqlite").exists());
        write_save(&dir, "walslot", &GameState::new(1));
        std::fs::write(dir.join("walslot.sqlite-wal"), b"wal").unwrap();
        title.rename_slot("walslot", "movedwal").unwrap();
        assert!(dir.join("movedwal.sqlite-wal").exists());
        assert!(!dir.join("walslot.sqlite").exists());
        write_save(&dir, "split", &GameState::new(1));
        std::fs::write(dir.join("split.sqlite-wal"), b"wal").unwrap();
        std::fs::create_dir(dir.join("stuck.sqlite-wal")).unwrap();
        assert!(title.rename_slot("split", "stuck").is_err());
        assert!(dir.join("split.sqlite").exists());
        assert!(!dir.join("stuck.sqlite").exists());
        write_save(&dir, "partial", &GameState::new(1));
        std::fs::write(dir.join("partial.sqlite-wal"), b"wal").unwrap();
        std::fs::write(dir.join("partial.sqlite-shm"), b"shm").unwrap();
        std::fs::create_dir(dir.join("blockedshm.sqlite-shm")).unwrap();
        assert!(title.rename_slot("partial", "blockedshm").is_err());
        assert!(dir.join("partial.sqlite").exists());
        assert!(dir.join("partial.sqlite-wal").exists());
        assert!(!dir.join("blockedshm.sqlite").exists());

        std::fs::create_dir(dir.join("blocked.sqlite")).unwrap();
        let err = title.delete_slot("blocked").unwrap_err();
        assert!(!err.is_empty(), "{err}");
        assert!(dir.join("blocked.sqlite").is_dir());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn settings_persist_and_apply_without_a_new_title() {
        let dir = unique_dir();
        let mut title = Title::new(dir.clone(), 1);
        title.page = Page::Settings;
        title.selected = 0;
        title.handle_key(KeyCode::Char(' '));
        assert!(title.config.reduced_motion);
        let loaded = UserConfig::load(&dir);
        assert!(loaded.reduced_motion);
        assert!(!loaded.bridge);
        assert!(!loaded.ai_advisor);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_seed_accepts_blank_and_u64_only() {
        assert_eq!(parse_seed("").unwrap(), None);
        assert_eq!(parse_seed("42").unwrap(), Some(42));
        assert!(parse_seed("nope").is_err());
    }
}
