//! Title screen: continue, new game, load, settings. No game rules live here.

use crate::clip::copy_to_clipboard;
use crate::config::UserConfig;
use crate::keymap::{Binding, HelpContext, Keymap};
use crate::portraits::{self, Mood, PortraitView, Slot as PortraitSlot};
use crate::ui::chrome::{Dialog, Navbar, StatusBar};
use crate::ui::table::{Column, DataTable};
use crate::ui::{Depth, Intent, Surface, Theme};
use hoshi_core::{
    AchievementCondition, AchievementConfig, BalanceConfig, CareerGlance, Prestige, PrestigeConfig,
    PrestigeError, RivalCatalog, RivalSettings, SaveError, SaveGlance, SeasonCatalog,
    SpeciesCatalog, SpeciesSpec, advisor_endpoint_allowed, encode_galaxy_seed, inspect_career,
    inspect_save, lock_save, open_storage, parse_galaxy_seed, preview_galaxy, probe_model_endpoint,
    save_is_locked, season_slot_name,
};
use ratatui::Frame;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

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
    Season,
    Load,
    Settings,
    Advisor,
    Keys,
    Achievements,
    Prestige,
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
    Species,
    Rivals,
    RivalRoster,
    RivalDifficulty,
}

impl NewField {
    fn next(self, rivals_on: bool) -> Self {
        match self {
            NewField::Name => NewField::Seed,
            NewField::Seed => NewField::Difficulty,
            NewField::Difficulty => NewField::Species,
            NewField::Species => NewField::Rivals,
            NewField::Rivals if rivals_on => NewField::RivalRoster,
            NewField::Rivals => NewField::Name,
            NewField::RivalRoster => NewField::RivalDifficulty,
            NewField::RivalDifficulty => NewField::Name,
        }
    }

    fn prev(self, rivals_on: bool) -> Self {
        match self {
            NewField::Name if rivals_on => NewField::RivalDifficulty,
            NewField::Name => NewField::Rivals,
            NewField::Seed => NewField::Name,
            NewField::Difficulty => NewField::Seed,
            NewField::Species => NewField::Difficulty,
            NewField::Rivals => NewField::Species,
            NewField::RivalRoster => NewField::Rivals,
            NewField::RivalDifficulty => NewField::RivalRoster,
        }
    }
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
    Newer { found: u32, kind: &'static str },
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
                match g.galaxy_seed {
                    Some(seed) => format!(
                        "{kind}  planets {}  fleet {}  {}  {ago}s ago",
                        g.planets,
                        g.fleet_strength,
                        encode_galaxy_seed(seed)
                    ),
                    None => format!(
                        "{kind}  planets {}  fleet {}  {ago}s ago",
                        g.planets, g.fleet_strength
                    ),
                }
            }
            SlotStatus::Locked => "in use by another hoshi process".into(),
            SlotStatus::Newer { found, kind } => newer_message(kind, *found),
            SlotStatus::Corrupt(msg) => format!("corrupt ({msg})"),
        }
    }

    fn playable(&self) -> bool {
        matches!(self, SlotStatus::Ready(_))
    }
}

/// What happened over one gap: the core ledger plus the session lists that
/// grew while it ran.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AwaySummary {
    pub elapsed_secs: u64,
    pub ledger: hoshi_core::CatchUpLedger,
    pub builds: u32,
    pub research: u32,
    pub deliveries: Vec<String>,
    pub stalls: Vec<String>,
    pub completed: Vec<String>,
    pub events: Vec<String>,
}

pub struct Title {
    pub updates: Option<crate::update_ui::UpdateUi>,
    support: Option<crate::support_ui::SupportUi>,
    bug: Option<crate::bug_ui::BugUi>,
    crash_notice: Option<String>,
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
    new_species: usize,
    new_rivals: bool,
    new_rival_row: usize,
    new_personalities: Vec<String>,
    new_aggression: f64,
    new_efficiency: f64,
    new_starting_bonus: f64,
    species_catalog: SpeciesCatalog,
    rival_catalog: RivalCatalog,
    confirm_delete: Option<String>,
    rename_from: Option<String>,
    rename_to: String,
    play_path: Option<PathBuf>,
    help_open: bool,
    help_commands: bool,
    keys_selected: usize,
    listening: bool,
    reset_confirm: Option<ResetConfirm>,
    career: CareerGlance,
    stats_selected: usize,
    achievements: AchievementConfig,
    balance: BalanceConfig,
    prestige_cfg: PrestigeConfig,
    prestige_selected: usize,
    prestige_confirm: bool,
    prestige_source: Option<PathBuf>,
    prestige_menu: bool,
    season_id: Option<String>,
    advisor_selected: usize,
    advisor_editing: bool,
    advisor_draft: String,
    advisor_probe: Option<mpsc::Receiver<Result<(), String>>>,
    advisor_probe_status: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResetConfirm {
    Action,
    All,
}

impl Title {
    pub fn new(dir: PathBuf, now_unix: u64) -> Self {
        std::fs::create_dir_all(&dir).ok();
        let config = UserConfig::load(&dir);
        let mut title = Self {
            updates: None,
            support: None,
            bug: None,
            crash_notice: crate::diagnostics::unseen_crash(&dir),
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
            new_species: 0,
            new_rivals: false,
            new_rival_row: 0,
            new_personalities: Vec::new(),
            new_aggression: 1.0,
            new_efficiency: 1.0,
            new_starting_bonus: 0.0,
            species_catalog: SpeciesCatalog::default(),
            rival_catalog: RivalCatalog::default(),
            confirm_delete: None,
            rename_from: None,
            rename_to: String::new(),
            play_path: None,
            help_open: false,
            help_commands: false,
            keys_selected: 0,
            listening: false,
            reset_confirm: None,
            career: CareerGlance::default(),
            stats_selected: 0,
            achievements: AchievementConfig::default(),
            balance: BalanceConfig::default(),
            prestige_cfg: PrestigeConfig::default(),
            prestige_selected: 0,
            prestige_confirm: false,
            prestige_source: None,
            prestige_menu: false,
            season_id: None,
            advisor_selected: 0,
            advisor_editing: false,
            advisor_draft: String::new(),
            advisor_probe: None,
            advisor_probe_status: String::new(),
        };
        title.new_rivals = title.config.rivals;
        title.new_personalities = title
            .rival_catalog
            .iter()
            .map(|row| row.id.clone())
            .collect();
        title.refresh_slots();
        // Without a playable save, Continue can only say "no save"; start on New game.
        if !title.slots.iter().any(|slot| slot.status.playable()) {
            title.selected = 1;
        }
        title
    }

    pub fn update_dir(&self) -> PathBuf {
        self.dir.clone()
    }

    pub fn take_play_path(&mut self) -> Option<PathBuf> {
        self.play_path.take()
    }

    pub fn tick(&mut self) {
        if !self.config.reduced_motion {
            self.tick = self.tick.saturating_add(1);
        }
        if let Some(rx) = &self.advisor_probe
            && let Ok(result) = rx.try_recv()
        {
            self.advisor_probe = None;
            self.advisor_probe_status = match result {
                Ok(()) => "connection ok".into(),
                Err(err) => format!("connection failed: {err}"),
            };
            self.status = self.advisor_probe_status.clone();
        }
    }

    pub fn set_now(&mut self, now_unix: u64) {
        self.now_unix = now_unix;
    }

    pub fn set_balance(&mut self, balance: BalanceConfig) {
        self.prestige_cfg = balance.prestige.clone();
        self.balance = balance;
    }

    fn season_visible(&self) -> bool {
        self.config.season && SeasonCatalog::default().current(self.now_unix).is_some()
    }

    fn menu_items(&self) -> Vec<&'static str> {
        let mut items = vec!["Continue", "New game"];
        if self.season_visible() {
            items.push("Season");
        }
        items.extend(["Load game", "Settings", "Achievements"]);
        if self.prestige_visible() {
            items.push("Prestige");
        }
        if self.config.bug_report_target != "off" {
            items.push("Report a bug");
        }
        items.push("Support report");
        items.push("Credits");
        items.push("Quit");
        items
    }

    fn select_menu_item(&mut self, name: &str) {
        if let Some(idx) = self.menu_items().iter().position(|item| *item == name) {
            self.selected = idx;
        }
    }

    fn prestige_visible(&self) -> bool {
        self.prestige_menu
    }

    fn prestige_summary(&self) -> String {
        if !self.prestige_visible() {
            return String::new();
        }
        let owned: Vec<String> = self
            .prestige_cfg
            .bonuses
            .iter()
            .filter_map(|spec| {
                let n = self.career.prestige.owned_count(&spec.id);
                (n > 0).then(|| format!("{}×{}", n, spec.name.to_ascii_lowercase()))
            })
            .collect();
        if owned.is_empty() {
            format!(
                "prestige run {}  {} unspent  no bonuses yet",
                self.career.prestige.current.index, self.career.prestige.currency
            )
        } else {
            format!(
                "prestige run {}  {} unspent  {}",
                self.career.prestige.current.index,
                self.career.prestige.currency,
                owned.join(", ")
            )
        }
    }

    fn seed_preview_line(&self) -> String {
        match parse_seed(&self.new_seed) {
            Ok(None) => format!(
                "random seed  generator {}",
                hoshi_core::GALAXY_GENERATOR_VERSION
            ),
            Ok(Some(seed)) => {
                let preview = preview_galaxy(seed, &self.balance.outposts);
                format!(
                    "{}  {} size {}  outposts {}  generator {}",
                    preview.code,
                    preview.home_kind.as_key(),
                    preview.home_size,
                    preview.outposts,
                    preview.generator
                )
            }
            Err(msg) => msg,
        }
    }

    fn refresh_slots(&mut self) {
        self.slots = list_slots(&self.dir, self.now_unix);
        self.slots.sort_by(|a, b| {
            slot_is_seasonal(a)
                .cmp(&slot_is_seasonal(b))
                .then_with(|| a.name.cmp(&b.name))
        });
        self.career = CareerGlance::default();
        self.prestige_source = None;
        self.prestige_menu = false;
        let mut best_lifetime = 0u32;
        let mut best_currency = 0u32;
        let mut source_prestige: Option<Prestige> = None;
        for slot in &self.slots {
            if inspect_save(&slot.path).is_ok_and(|glance| glance.modified) {
                continue;
            }
            if let Ok(glance) = inspect_career(&slot.path) {
                let ready = glance.reset_available(&self.prestige_cfg)
                    && (!glance.seasonal || glance.shares_prestige);
                if ready {
                    self.prestige_menu = true;
                    let take = source_prestige.is_none()
                        || glance.prestige.lifetime > best_lifetime
                        || (glance.prestige.lifetime == best_lifetime
                            && glance.prestige.currency > best_currency);
                    if take {
                        best_lifetime = glance.prestige.lifetime;
                        best_currency = glance.prestige.currency;
                        self.prestige_source = Some(slot.path.clone());
                        source_prestige = Some(glance.prestige.clone());
                    }
                }
                self.career.merge(glance);
            }
        }
        if let Some(prestige) = source_prestige {
            self.career.prestige = prestige;
        }
    }

    pub fn handle_key(&mut self, code: KeyCode) -> TitleAction {
        if let Some(bug) = &mut self.bug {
            if bug.handle_key(code) {
                self.bug = None;
            }
            return TitleAction::None;
        }
        if let Some(report) = &mut self.support {
            if report.handle_key(code) {
                self.support = None;
            }
            return TitleAction::None;
        }
        if let Some(updates) = &mut self.updates {
            if self.help_open && code == KeyCode::Char('u') {
                updates.dismiss_available();
                return TitleAction::None;
            }
            if self.help_open && code == KeyCode::Char('v') {
                updates.open_changelog();
                return TitleAction::None;
            }
            if updates.handle_key(code) {
                return TitleAction::None;
            }
        }

        if self.crash_notice.is_some() && matches!(code, KeyCode::Char('r') | KeyCode::Esc) {
            if let Some(crash) = self.crash_notice.take()
                && let Err(error) = crate::diagnostics::acknowledge_crash(&self.dir, &crash)
            {
                self.status = error.to_string();
            }
            if code == KeyCode::Char('r') {
                self.open_bug(true);
            }
            return TitleAction::None;
        }
        if self.help_open && code == KeyCode::Char('b') {
            self.open_bug(false);
            return TitleAction::None;
        }
        if self.help_open && code == KeyCode::Char('p') {
            self.open_support();
            return TitleAction::None;
        }

        if self.help_open {
            if matches!(code, KeyCode::Tab | KeyCode::BackTab) {
                self.help_commands = !self.help_commands;
                return TitleAction::None;
            }
            if self.config.keys.matches(Binding::Quit, code) && self.page == Page::Menu {
                return TitleAction::Quit;
            }
            if self.config.keys.matches(Binding::Help, code)
                || self.config.keys.matches(Binding::Cancel, code)
                || self.config.keys.matches(Binding::Confirm, code)
            {
                self.help_open = false;
            }
            return TitleAction::None;
        }
        if !self.listening
            && !self.advisor_editing
            && self.config.keys.lookup(self.help_context(), code) == Some(Binding::Help)
        {
            self.help_open = true;
            self.help_commands = false;
            return TitleAction::None;
        }
        match self.page {
            Page::Menu => self.handle_menu(code),
            Page::New => self.handle_new(code),
            Page::Season => self.handle_season(code),
            Page::Load => self.handle_load(code),
            Page::Settings => self.handle_settings(code),
            Page::Advisor => self.handle_advisor(code),
            Page::Keys => self.handle_keys(code),
            Page::Achievements => self.handle_achievements(code),
            Page::Prestige => self.handle_prestige(code),
            Page::Credits => {
                if let Some(Binding::Confirm | Binding::Cancel) =
                    self.config.keys.lookup(HelpContext::TitleCredits, code)
                {
                    self.page = Page::Menu;
                }
                TitleAction::None
            }
        }
    }

    fn help_context(&self) -> HelpContext {
        match self.page {
            Page::Menu => HelpContext::TitleMenu,
            Page::New => HelpContext::TitleNew,
            Page::Season => HelpContext::TitleSeason,
            Page::Load if self.rename_from.is_some() => HelpContext::TitleRename,
            Page::Load if self.confirm_delete.is_some() => HelpContext::TitleDelete,
            Page::Load => HelpContext::TitleLoad,
            Page::Settings => HelpContext::TitleSettings,
            Page::Advisor => HelpContext::TitleAdvisor,
            Page::Keys => HelpContext::TitleKeys,
            Page::Achievements => HelpContext::TitleAchievements,
            Page::Prestige => HelpContext::TitlePrestige,
            Page::Credits => HelpContext::TitleCredits,
        }
    }

    fn handle_menu(&mut self, code: KeyCode) -> TitleAction {
        match self.config.keys.lookup(HelpContext::TitleMenu, code) {
            Some(Binding::Quit) => TitleAction::Quit,
            Some(Binding::CopySeed) => {
                self.copy_continue_seed();
                TitleAction::None
            }
            Some(Binding::SelectNext) => {
                let n = self.menu_items().len();
                self.selected = (self.selected + 1) % n;
                TitleAction::None
            }
            Some(Binding::SelectPrev) => {
                let n = self.menu_items().len();
                self.selected = (self.selected + n - 1) % n;
                TitleAction::None
            }
            Some(Binding::Confirm) => {
                let item = self
                    .menu_items()
                    .get(self.selected)
                    .copied()
                    .unwrap_or("Quit");
                match item {
                    "Continue" => self.continue_last(),
                    "New game" => {
                        self.page = Page::New;
                        self.status.clear();
                        self.pick_default_name();
                        TitleAction::None
                    }
                    "Season" => {
                        self.page = Page::Season;
                        self.selected = 0;
                        self.season_id = None;
                        self.status.clear();
                        TitleAction::None
                    }
                    "Load game" => {
                        self.refresh_slots();
                        self.page = Page::Load;
                        self.selected = 0;
                        self.status.clear();
                        TitleAction::None
                    }
                    "Settings" => {
                        self.page = Page::Settings;
                        self.selected = 0;
                        TitleAction::None
                    }
                    "Achievements" => {
                        self.open_achievements();
                        TitleAction::None
                    }
                    "Prestige" => {
                        self.open_prestige();
                        TitleAction::None
                    }
                    "Report a bug" => {
                        self.open_bug(false);
                        TitleAction::None
                    }
                    "Support report" => {
                        self.open_support();
                        TitleAction::None
                    }
                    "Credits" => {
                        self.page = Page::Credits;
                        TitleAction::None
                    }
                    _ => TitleAction::Quit,
                }
            }
            _ => TitleAction::None,
        }
    }

    fn open_bug(&mut self, crash: bool) {
        if self.config.bug_report_target != "off" {
            let path = continue_slot(&self.slots, self.config.last_slot.as_deref())
                .map(|slot| slot.path.clone())
                .unwrap_or_else(|| self.dir.join("save.sqlite"));
            self.bug = Some(crate::bug_ui::BugUi::open(&path, &self.dir, crash));
        }
    }

    fn open_support(&mut self) {
        let path = continue_slot(&self.slots, self.config.last_slot.as_deref())
            .map(|slot| slot.path.clone())
            .unwrap_or_else(|| self.dir.join("save.sqlite"));
        match crate::support_ui::SupportUi::open(&path) {
            Ok(report) => self.support = Some(report),
            Err(error) => self.status = error,
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
            SlotStatus::Newer { found, kind } => {
                self.status = format!("{}: {}", slot.name, newer_message(kind, *found));
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

    fn copy_continue_seed(&mut self) {
        let want = self.config.last_slot.clone();
        let slot = want
            .as_ref()
            .and_then(|name| self.slots.iter().find(|s| s.name == *name))
            .or_else(|| self.slots.iter().find(|s| s.status.playable()));
        match slot.cloned() {
            Some(slot) => self.copy_slot_seed(&slot),
            None => self.status = "no seed to copy".into(),
        }
    }

    fn copy_slot_seed(&mut self, slot: &Slot) {
        match &slot.status {
            SlotStatus::Ready(g) | SlotStatus::Harness(g) => match g.galaxy_seed {
                Some(seed) => {
                    let code = encode_galaxy_seed(seed);
                    copy_to_clipboard(&code);
                    self.status = format!("copied {code}");
                }
                None => self.status = "no seed to copy".into(),
            },
            _ => self.status = "no seed to copy".into(),
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
        match self.config.keys.lookup(HelpContext::TitleNew, code) {
            Some(Binding::Cancel) => {
                self.page = Page::Menu;
                return TitleAction::None;
            }
            Some(Binding::NextScreen) => {
                self.new_field = self.new_field.next(self.new_rivals);
                return TitleAction::None;
            }
            Some(Binding::Confirm) => {
                if self.new_field == NewField::Difficulty
                    || self.new_field == NewField::Species
                    || self.new_field == NewField::Rivals
                    || self.new_field == NewField::RivalRoster
                    || self.new_field == NewField::RivalDifficulty
                {
                    return self.create_new();
                }
                self.new_field = self.new_field.next(self.new_rivals);
                return TitleAction::None;
            }
            Some(Binding::Toggle) if self.new_field == NewField::Difficulty => {
                self.new_difficulty = self.new_difficulty.cycle();
                return TitleAction::None;
            }
            Some(Binding::Toggle) if self.new_field == NewField::Species => {
                self.cycle_species();
                return TitleAction::None;
            }
            Some(Binding::Toggle) if self.new_field == NewField::Rivals => {
                self.new_rivals = !self.new_rivals;
                if self.new_rivals && self.new_personalities.is_empty() {
                    self.new_personalities = self
                        .rival_catalog
                        .iter()
                        .map(|row| row.id.clone())
                        .collect();
                }
                if !self.new_rivals
                    && matches!(
                        self.new_field,
                        NewField::RivalRoster | NewField::RivalDifficulty
                    )
                {
                    self.new_field = NewField::Rivals;
                }
                return TitleAction::None;
            }
            Some(Binding::Toggle) if self.new_field == NewField::RivalRoster => {
                self.toggle_rival_personality();
                return TitleAction::None;
            }
            Some(Binding::Toggle) if self.new_field == NewField::RivalDifficulty => {
                self.cycle_rival_difficulty();
                return TitleAction::None;
            }
            _ => {}
        }
        match code {
            KeyCode::Down => {
                self.new_field = self.new_field.next(self.new_rivals);
                TitleAction::None
            }
            KeyCode::Up => {
                self.new_field = self.new_field.prev(self.new_rivals);
                TitleAction::None
            }
            KeyCode::Left if self.new_field == NewField::RivalRoster => {
                self.cycle_rival_row(false);
                TitleAction::None
            }
            KeyCode::Right if self.new_field == NewField::RivalRoster => {
                self.cycle_rival_row(true);
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
                    NewField::Difficulty
                    | NewField::Species
                    | NewField::Rivals
                    | NewField::RivalRoster
                    | NewField::RivalDifficulty => {}
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
                    NewField::Difficulty
                    | NewField::Species
                    | NewField::Rivals
                    | NewField::RivalRoster
                    | NewField::RivalDifficulty => {}
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
        let spec = self.selected_species();
        if !self
            .species_catalog
            .is_unlocked(&spec.id, &self.career.prestige)
        {
            self.status = format!("{} locked ({} prestige)", spec.name, spec.unlock_cost);
            return TitleAction::None;
        }
        if self.new_rivals && self.new_personalities.is_empty() {
            self.status = "pick at least one rival personality".into();
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
        self.season_id = None;
        TitleAction::Play(0)
    }

    fn catalog_specs(&self) -> Vec<&SpeciesSpec> {
        self.species_catalog.iter().collect()
    }

    fn selected_species(&self) -> &SpeciesSpec {
        let specs = self.catalog_specs();
        specs
            .get(self.new_species)
            .copied()
            .or_else(|| specs.first().copied())
            .unwrap_or_else(|| {
                self.species_catalog
                    .get("humans")
                    .expect("humans are bundled")
            })
    }

    fn cycle_species(&mut self) {
        let n = self.catalog_specs().len().max(1);
        self.new_species = (self.new_species + 1) % n;
    }

    fn cycle_rival_row(&mut self, forward: bool) {
        let n = self.rival_catalog.iter().count().max(1);
        if forward {
            self.new_rival_row = (self.new_rival_row + 1) % n;
        } else {
            self.new_rival_row = (self.new_rival_row + n - 1) % n;
        }
    }

    fn toggle_rival_personality(&mut self) {
        let Some(spec) = self.rival_catalog.iter().nth(self.new_rival_row) else {
            return;
        };
        let id = spec.id.clone();
        if let Some(pos) = self.new_personalities.iter().position(|row| row == &id) {
            self.new_personalities.remove(pos);
        } else {
            self.new_personalities.push(id);
        }
    }

    fn cycle_rival_difficulty(&mut self) {
        let presets = [(1.0, 1.0, 0.0), (0.5, 1.0, 0.0), (1.5, 1.2, 0.25)];
        let cur = (
            self.new_aggression,
            self.new_efficiency,
            self.new_starting_bonus,
        );
        let idx = presets
            .iter()
            .position(|p| *p == cur)
            .map(|i| (i + 1) % presets.len())
            .unwrap_or(0);
        let next = presets[idx];
        self.new_aggression = next.0;
        self.new_efficiency = next.1;
        self.new_starting_bonus = next.2;
    }

    fn rival_difficulty_label(&self) -> &'static str {
        match (
            self.new_aggression,
            self.new_efficiency,
            self.new_starting_bonus,
        ) {
            (a, e, b)
                if (a - 1.0).abs() < f64::EPSILON
                    && (e - 1.0).abs() < f64::EPSILON
                    && b.abs() < f64::EPSILON =>
            {
                "standard"
            }
            (a, e, b)
                if (a - 0.5).abs() < f64::EPSILON
                    && (e - 1.0).abs() < f64::EPSILON
                    && b.abs() < f64::EPSILON =>
            {
                "calm"
            }
            (a, e, b)
                if (a - 1.5).abs() < f64::EPSILON
                    && (e - 1.2).abs() < f64::EPSILON
                    && (b - 0.25).abs() < f64::EPSILON =>
            {
                "fierce"
            }
            _ => "custom",
        }
    }

    fn rival_settings(&self) -> RivalSettings {
        if !self.new_rivals {
            return RivalSettings::default();
        }
        RivalSettings {
            enabled: true,
            tick_secs: self.rival_catalog.tick_secs,
            aggression: self.new_aggression,
            efficiency: self.new_efficiency,
            starting_bonus: self.new_starting_bonus,
            personalities: self.new_personalities.clone(),
        }
    }

    fn species_unlocked(&self, spec: &SpeciesSpec) -> bool {
        self.species_catalog
            .is_unlocked(&spec.id, &self.career.prestige)
    }

    fn species_card_lines(&self, spec: &SpeciesSpec, wide: bool) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        if wide && self.config.portraits {
            lines.extend(portraits::portrait_lines_for(
                PortraitView {
                    enabled: true,
                    protocol: portraits::Protocol::None,
                },
                PortraitSlot::Commander,
                Mood::Neutral,
                Some(spec.portrait.as_str()),
            ));
        }
        let lock = if self.species_unlocked(spec) {
            "unlocked".to_string()
        } else {
            format!("locked, {} prestige", spec.unlock_cost)
        };
        lines.push(Line::from(format!(
            "{}  {}  lean {}  {lock}",
            spec.name,
            spec.blurb,
            spec.victory_lean.as_key()
        )));
        lines.push(Line::from(format!(
            "+{}  -{}",
            spec.strengths, spec.weaknesses
        )));
        if !spec.standing.is_empty() {
            let standing = spec
                .standing
                .iter()
                .map(|(faction, delta)| format!("{faction} {delta:+.0}"))
                .collect::<Vec<_>>()
                .join("  ");
            lines.push(Line::from(format!("start standing  {standing}")));
        }
        lines
    }

    fn slot_status_line(&self, status: &SlotStatus, now_unix: u64) -> String {
        match status {
            SlotStatus::Ready(g) | SlotStatus::Harness(g) => {
                let ago = now_unix.saturating_sub(g.last_tick_unix);
                let species = g
                    .species_id
                    .as_deref()
                    .and_then(|id| self.species_catalog.get(id).map(|s| s.name.as_str()))
                    .unwrap_or("Humans");
                let kind = if matches!(status, SlotStatus::Harness(_)) {
                    "harness save (CLI only)"
                } else {
                    species
                };
                let rivals = if g.rivals_enabled {
                    let names: Vec<_> = g
                        .rival_personalities
                        .iter()
                        .map(|id| {
                            self.rival_catalog
                                .get(id)
                                .map(|row| row.name.as_str())
                                .unwrap_or(id.as_str())
                        })
                        .collect();
                    if names.is_empty() {
                        "rivals on".to_string()
                    } else {
                        format!("rivals {}", names.join(", "))
                    }
                } else {
                    "rivals off".to_string()
                };
                match g.galaxy_seed {
                    Some(seed) => format!(
                        "{kind}  planets {}  fleet {}  {}  {rivals}  {ago}s ago",
                        g.planets,
                        g.fleet_strength,
                        encode_galaxy_seed(seed)
                    ),
                    None => format!(
                        "{kind}  planets {}  fleet {}  {rivals}  {ago}s ago",
                        g.planets, g.fleet_strength
                    ),
                }
            }
            _ => status.line(now_unix),
        }
    }

    fn locked_species(&self) -> Vec<&SpeciesSpec> {
        self.species_catalog
            .iter()
            .filter(|spec| {
                !spec.available && !self.career.prestige.unlocked_species.contains(&spec.id)
            })
            .collect()
    }

    fn handle_season(&mut self, code: KeyCode) -> TitleAction {
        match self.config.keys.lookup(HelpContext::TitleSeason, code) {
            Some(Binding::Cancel) => {
                self.page = Page::Menu;
                self.select_menu_item("Season");
                TitleAction::None
            }
            Some(Binding::Confirm) => self.start_season(),
            _ => TitleAction::None,
        }
    }

    fn start_season(&mut self) -> TitleAction {
        let cat = SeasonCatalog::default();
        let Some(ruleset) = cat.current(self.now_unix) else {
            self.status = "no season is active".into();
            return TitleAction::None;
        };
        let window = cat.window(self.now_unix);
        let name = season_slot_name(&ruleset.id, window.start);
        let path = self.dir.join(format!("{name}.sqlite"));
        if path.exists() {
            self.season_id = None;
            self.play_path = Some(path);
            self.config.last_slot = Some(name);
            let _ = self.config.save(&self.dir);
            return TitleAction::Play(0);
        }
        self.season_id = Some(ruleset.id.clone());
        self.play_path = Some(path);
        self.config.last_slot = Some(name);
        let _ = self.config.save(&self.dir);
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
            if self.config.keys.lookup(HelpContext::TitleLoad, code) == Some(Binding::Cancel) {
                self.page = Page::Menu;
                self.select_menu_item("Load game");
            }
            return TitleAction::None;
        }
        match self.config.keys.lookup(HelpContext::TitleLoad, code) {
            Some(Binding::Cancel) => {
                self.page = Page::Menu;
                self.select_menu_item("Load game");
                TitleAction::None
            }
            Some(Binding::SelectNext) => {
                self.selected = (self.selected + 1) % self.slots.len();
                TitleAction::None
            }
            Some(Binding::SelectPrev) => {
                self.selected = (self.selected + self.slots.len() - 1) % self.slots.len();
                TitleAction::None
            }
            Some(Binding::CopySeed) => {
                if let Some(slot) = self.slots.get(self.selected).cloned() {
                    self.copy_slot_seed(&slot);
                }
                TitleAction::None
            }
            Some(Binding::Confirm) => {
                let slot = self.slots[self.selected].clone();
                self.try_play(&slot)
            }
            Some(Binding::DeleteSlot) => {
                let name = self.slots[self.selected].name.clone();
                if save_is_locked(&self.slots[self.selected].path) {
                    self.status = format!("{name} is locked");
                    return TitleAction::None;
                }
                self.confirm_delete = Some(name);
                self.status = format!(
                    "delete: press {} again or Enter",
                    self.config.keys.primary(Binding::DeleteSlot)
                );
                TitleAction::None
            }
            Some(Binding::RenameSlot) => {
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
        match self.config.keys.lookup(HelpContext::TitleDelete, code) {
            Some(Binding::Cancel) => {
                self.confirm_delete = None;
                self.status.clear();
            }
            Some(Binding::DeleteSlot | Binding::Confirm) => {
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
        match self.config.keys.lookup(HelpContext::TitleRename, code) {
            Some(Binding::Cancel) => {
                self.rename_from = None;
                self.status.clear();
                return TitleAction::None;
            }
            Some(Binding::Confirm) => {
                if let Err(msg) = self.rename_slot(&from, &self.rename_to.clone()) {
                    self.status = msg;
                } else {
                    self.status = format!("renamed {from}");
                    self.refresh_slots();
                }
                self.rename_from = None;
                return TitleAction::None;
            }
            _ => {}
        }
        match code {
            KeyCode::Backspace => {
                self.rename_to.pop();
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

    fn open_achievements(&mut self) {
        self.refresh_career();
        self.page = Page::Achievements;
        self.stats_selected = 0;
        self.status.clear();
    }

    fn refresh_career(&mut self) {
        self.refresh_slots();
    }

    fn open_prestige(&mut self) {
        self.refresh_slots();
        if !self.prestige_visible() {
            self.status = "prestige is locked until a reset is available".into();
            return;
        }
        self.page = Page::Prestige;
        self.prestige_selected = 0;
        self.prestige_confirm = false;
        self.status.clear();
    }

    fn handle_prestige(&mut self, code: KeyCode) -> TitleAction {
        let items = self
            .prestige_cfg
            .bonuses
            .len()
            .saturating_add(self.locked_species().len())
            .max(1);
        match self.config.keys.lookup(HelpContext::TitlePrestige, code) {
            Some(Binding::Cancel) => {
                if self.prestige_confirm {
                    self.prestige_confirm = false;
                    self.status.clear();
                } else {
                    self.page = Page::Menu;
                    if let Some(idx) = self
                        .menu_items()
                        .iter()
                        .position(|item| *item == "Prestige")
                    {
                        self.selected = idx;
                    }
                    self.status.clear();
                }
            }
            Some(Binding::SelectNext) => {
                self.prestige_confirm = false;
                self.prestige_selected = (self.prestige_selected + 1) % items;
            }
            Some(Binding::SelectPrev) => {
                self.prestige_confirm = false;
                self.prestige_selected = (self.prestige_selected + items - 1) % items;
            }
            Some(Binding::Confirm) => {
                if self.prestige_confirm {
                    self.buy_selected_prestige();
                    self.prestige_confirm = false;
                } else {
                    self.prestige_confirm = true;
                    let n_bonus = self.prestige_cfg.bonuses.len();
                    if let Some(spec) = self.prestige_cfg.bonuses.get(self.prestige_selected) {
                        self.status = format!(
                            "buy {} for {}? enter again to confirm",
                            spec.name, spec.cost
                        );
                    } else if let Some(spec) = self
                        .locked_species()
                        .get(self.prestige_selected.saturating_sub(n_bonus))
                    {
                        self.status = format!(
                            "unlock {} for {}? enter again to confirm",
                            spec.name, spec.unlock_cost
                        );
                    }
                }
            }
            _ => {}
        }
        TitleAction::None
    }

    fn buy_selected_prestige(&mut self) {
        let n_bonus = self.prestige_cfg.bonuses.len();
        if self.prestige_selected < n_bonus {
            self.buy_selected_bonus();
            return;
        }
        let Some(spec) = self
            .locked_species()
            .get(self.prestige_selected - n_bonus)
            .cloned()
            .cloned()
        else {
            return;
        };
        self.unlock_selected_species(&spec);
    }

    fn buy_selected_bonus(&mut self) {
        let Some(spec) = self
            .prestige_cfg
            .bonuses
            .get(self.prestige_selected)
            .cloned()
        else {
            return;
        };
        let Some(path) = self.prestige_source.clone() else {
            self.status = "no save to store prestige".into();
            return;
        };
        if save_is_locked(&path) {
            self.status = "save is locked".into();
            return;
        }
        let result = (|| -> Result<(), String> {
            let _lock = lock_save(&path).map_err(|e| e.to_string())?;
            let mut store = open_storage(&path).map_err(|e| e.to_string())?;
            let mut state = store
                .load(self.balance.clone())
                .map_err(|e| e.to_string())?;
            state
                .buy_prestige_bonus(&spec.id, self.now_unix)
                .map_err(|e: PrestigeError| e.to_string())?;
            store.save(&state).map_err(|e| e.to_string())?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.refresh_slots();
                self.status = format!("bought {}", spec.name);
            }
            Err(msg) => self.status = msg,
        }
    }

    fn unlock_selected_species(&mut self, spec: &SpeciesSpec) {
        let Some(path) = self.prestige_source.clone() else {
            self.status = "no save to store prestige".into();
            return;
        };
        if save_is_locked(&path) {
            self.status = "save is locked".into();
            return;
        }
        let id = spec.id.clone();
        let name = spec.name.clone();
        let result = (|| -> Result<(), String> {
            let _lock = lock_save(&path).map_err(|e| e.to_string())?;
            let mut store = open_storage(&path).map_err(|e| e.to_string())?;
            let mut state = store
                .load(self.balance.clone())
                .map_err(|e| e.to_string())?;
            state
                .unlock_species(&self.species_catalog, &id)
                .map_err(|e| e.to_string())?;
            store.save(&state).map_err(|e| e.to_string())?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.refresh_slots();
                self.status = format!("unlocked {name}");
            }
            Err(msg) => self.status = msg,
        }
    }

    fn handle_achievements(&mut self, code: KeyCode) -> TitleAction {
        let items = self.achievements.len().max(1);
        match self
            .config
            .keys
            .lookup(HelpContext::TitleAchievements, code)
        {
            Some(Binding::Cancel) => {
                self.page = Page::Menu;
                self.select_menu_item("Achievements");
                self.status.clear();
            }
            Some(Binding::SelectNext) => {
                self.stats_selected = (self.stats_selected + 1) % items;
            }
            Some(Binding::SelectPrev) => {
                self.stats_selected = (self.stats_selected + items - 1) % items;
            }
            _ => {}
        }
        TitleAction::None
    }

    fn handle_settings(&mut self, code: KeyCode) -> TitleAction {
        let items = 12;
        match self.config.keys.lookup(HelpContext::TitleSettings, code) {
            Some(Binding::Cancel) => {
                self.page = Page::Menu;
                self.select_menu_item("Settings");
            }
            Some(Binding::SelectNext) => {
                self.selected = (self.selected + 1) % items;
            }
            Some(Binding::SelectPrev) => {
                self.selected = (self.selected + items - 1) % items;
            }
            Some(Binding::Confirm | Binding::Toggle) => {
                match self.selected {
                    0 => self.config.reduced_motion = !self.config.reduced_motion,
                    1 => self.config.high_contrast = !self.config.high_contrast,
                    2 => self.config.bridge = !self.config.bridge,
                    3 => self.config.ai_advisor = !self.config.ai_advisor,
                    4 => self.config.portraits = !self.config.portraits,
                    5 => self.config.notify = !self.config.notify,
                    6 => self.config.tutorial = !self.config.tutorial,
                    7 => {
                        self.config.reset_tutorial = true;
                        self.status = "tutorial resets the next time a save is opened".into();
                    }
                    8 => {
                        self.page = Page::Keys;
                        self.keys_selected = 0;
                        self.listening = false;
                        self.reset_confirm = None;
                        self.status.clear();
                        return TitleAction::None;
                    }
                    9 => {
                        self.page = Page::Advisor;
                        self.advisor_selected = 0;
                        self.advisor_editing = false;
                        self.advisor_draft.clear();
                        self.advisor_probe = None;
                        self.advisor_probe_status.clear();
                        self.status.clear();
                        return TitleAction::None;
                    }
                    11 => self.config.cheats = !self.config.cheats,
                    10 => {
                        self.help_open = true;
                        self.help_commands = false;
                        return TitleAction::None;
                    }
                    _ => {}
                }
                let _ = self.config.save(&self.dir);
            }
            _ => {}
        }
        TitleAction::None
    }

    fn handle_advisor(&mut self, code: KeyCode) -> TitleAction {
        if self.advisor_editing {
            match self.config.keys.lookup(HelpContext::TitleAdvisor, code) {
                Some(Binding::Cancel) => {
                    self.advisor_editing = false;
                    self.advisor_draft.clear();
                    self.status.clear();
                    return TitleAction::None;
                }
                Some(Binding::Confirm) => {
                    self.commit_advisor_draft();
                    return TitleAction::None;
                }
                _ => {}
            }
            match code {
                KeyCode::Backspace => {
                    self.advisor_draft.pop();
                }
                KeyCode::Char(c) if !c.is_control() && self.advisor_draft.len() < 80 => {
                    self.advisor_draft.push(c);
                }
                _ => {}
            }
            return TitleAction::None;
        }
        const ITEMS: usize = 5;
        match self.config.keys.lookup(HelpContext::TitleAdvisor, code) {
            Some(Binding::Cancel) => {
                self.page = Page::Settings;
                self.selected = 9;
                self.status.clear();
            }
            Some(Binding::SelectNext) => {
                self.advisor_selected = (self.advisor_selected + 1) % ITEMS;
            }
            Some(Binding::SelectPrev) => {
                self.advisor_selected = (self.advisor_selected + ITEMS - 1) % ITEMS;
            }
            Some(Binding::Confirm | Binding::Toggle) => match self.advisor_selected {
                0 => {
                    self.config.ai_advisor = !self.config.ai_advisor;
                    let _ = self.config.save(&self.dir);
                }
                1 => {
                    self.advisor_draft = self.config.advisor_model_url.clone();
                    self.advisor_editing = true;
                    self.status = "edit endpoint, Enter to save".into();
                }
                2 => {
                    self.advisor_draft = self.config.advisor_model_name.clone();
                    self.advisor_editing = true;
                    self.status = "edit model name, Enter to save".into();
                }
                3 => {
                    self.config.advisor_allow_lan = !self.config.advisor_allow_lan;
                    let _ = self.config.save(&self.dir);
                }
                4 => self.start_advisor_probe(),
                _ => {}
            },
            _ => {}
        }
        TitleAction::None
    }

    fn commit_advisor_draft(&mut self) {
        let draft = self.advisor_draft.trim().to_string();
        match self.advisor_selected {
            1 => {
                if !draft.is_empty()
                    && !advisor_endpoint_allowed(&draft, self.config.advisor_allow_lan)
                {
                    self.status = "only loopback, or enable LAN for a private IPv4 host".into();
                    return;
                }
                self.config.advisor_model_url = draft;
            }
            2 => {
                if !draft
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
                {
                    self.status = "model name: letters, digits, dash, underscore, dot".into();
                    return;
                }
                self.config.advisor_model_name = draft;
            }
            _ => {}
        }
        self.advisor_editing = false;
        self.advisor_draft.clear();
        let _ = self.config.save(&self.dir);
        self.status = "saved".into();
    }

    fn start_advisor_probe(&mut self) {
        if self.advisor_probe.is_some() {
            self.status = "connection test already running".into();
            return;
        }
        let url = self.config.advisor_model_url.clone();
        let allow_lan = self.config.advisor_allow_lan;
        let (tx, rx) = mpsc::channel();
        self.advisor_probe = Some(rx);
        self.advisor_probe_status = "testing…".into();
        self.status = "testing connection…".into();
        std::thread::spawn(move || {
            let _ = tx.send(probe_model_endpoint(&url, allow_lan));
        });
    }

    fn commit_keys(&mut self, keys: Keymap, ok: String) {
        let previous = self.config.keys.clone();
        self.config.keys = keys;
        if let Err(err) = self.config.save(&self.dir) {
            self.config.keys = previous;
            self.status = format!("could not save: {err}");
            return;
        }
        self.status = ok;
    }

    fn handle_keys(&mut self, code: KeyCode) -> TitleAction {
        let actions = Binding::REMAPPABLE;
        let items = actions.len() + 1;
        if self.listening {
            if self.config.keys.matches(Binding::Cancel, code) {
                self.listening = false;
                self.status = "rebinding cancelled".into();
                return TitleAction::None;
            }
            match self.config.keys.rebind(actions[self.keys_selected], code) {
                Ok(keys) => {
                    let msg = format!(
                        "saved {} -> {}",
                        HelpContext::label(actions[self.keys_selected]),
                        keys.display(actions[self.keys_selected])
                    );
                    self.commit_keys(keys, msg);
                }
                Err(err) => {
                    self.status = err;
                }
            }
            self.listening = false;
            return TitleAction::None;
        }
        if let Some(confirm) = self.reset_confirm {
            match self.config.keys.lookup(HelpContext::TitleKeys, code) {
                Some(Binding::Confirm) => {
                    match confirm {
                        ResetConfirm::Action => {
                            match self.config.keys.reset_binding(actions[self.keys_selected]) {
                                Ok(keys) => {
                                    let msg = format!(
                                        "reset {}",
                                        HelpContext::label(actions[self.keys_selected])
                                    );
                                    self.commit_keys(keys, msg);
                                }
                                Err(err) => self.status = err,
                            }
                        }
                        ResetConfirm::All => {
                            self.commit_keys(Keymap::defaults(), "reset all keys".into());
                        }
                    }
                    self.reset_confirm = None;
                }
                Some(Binding::Cancel) => {
                    self.reset_confirm = None;
                    self.status.clear();
                }
                _ => {}
            }
            return TitleAction::None;
        }
        match self.config.keys.lookup(HelpContext::TitleKeys, code) {
            Some(Binding::Cancel) => {
                self.page = Page::Settings;
                self.selected = 8;
                self.status.clear();
            }
            Some(Binding::SelectNext) => {
                self.keys_selected = (self.keys_selected + 1) % items;
            }
            Some(Binding::SelectPrev) => {
                self.keys_selected = (self.keys_selected + items - 1) % items;
            }
            Some(Binding::Confirm) if self.keys_selected == actions.len() => {
                self.reset_confirm = Some(ResetConfirm::All);
                self.status = "press enter again to reset all keys".into();
            }
            Some(Binding::Confirm) => {
                self.listening = true;
                self.status = format!(
                    "press a key for {}",
                    HelpContext::label(actions[self.keys_selected])
                );
            }
            Some(Binding::DeleteSlot) if self.keys_selected < actions.len() => {
                self.reset_confirm = Some(ResetConfirm::Action);
                self.status = format!(
                    "press enter again to reset {}",
                    HelpContext::label(actions[self.keys_selected])
                );
            }
            _ => {}
        }
        TitleAction::None
    }

    fn theme(&self) -> Theme {
        Theme::resolve(
            Depth::detect(|key| std::env::var(key).ok()),
            self.config.high_contrast,
        )
    }

    fn page_label(&self) -> &'static str {
        match self.page {
            Page::Menu => "menu",
            Page::New => "new game",
            Page::Season => "season",
            Page::Load => "load game",
            Page::Settings => "settings",
            Page::Advisor => "advisor",
            Page::Keys => "keys",
            Page::Achievements => "achievements",
            Page::Prestige => "prestige",
            Page::Credits => "credits",
        }
    }

    /// Saves as a table: name, species, planets, fleet, last played and state.
    fn save_table<'a>(&self, t: &'a Theme) -> DataTable<'a> {
        let continue_name = self
            .config
            .last_slot
            .as_deref()
            .filter(|name| {
                self.slots
                    .iter()
                    .any(|s| s.name == *name && s.status.playable())
            })
            .or_else(|| {
                self.slots
                    .iter()
                    .find(|s| s.status.playable())
                    .map(|s| s.name.as_str())
            });
        let rows = self
            .slots
            .iter()
            .map(|slot| {
                let mut name = vec![Span::styled(slot_label(slot), t.text())];
                if continue_name == Some(slot.name.as_str()) {
                    name.insert(0, Span::styled("▸ ", t.intent(Intent::Primary)));
                }
                let chip = |text: &str, intent: Intent| {
                    Line::from(crate::ui::spans::tag(t, text, intent, true))
                };
                match &slot.status {
                    SlotStatus::Ready(g) | SlotStatus::Harness(g) => {
                        let species = g
                            .species_id
                            .as_deref()
                            .and_then(|id| self.species_catalog.get(id).map(|s| s.name.clone()))
                            .unwrap_or_else(|| "Humans".into());
                        let ago = self.now_unix.saturating_sub(g.last_tick_unix);
                        vec![
                            Line::from(name),
                            Line::styled(species, t.muted()),
                            Line::styled(g.planets.to_string(), t.text()),
                            Line::styled(g.fleet_strength.to_string(), t.text()),
                            Line::styled(
                                format!("{} ago", crate::ui::spans::duration(ago)),
                                t.muted(),
                            ),
                            if g.modified {
                                chip("MODIFIED", Intent::Warning)
                            } else if matches!(slot.status, SlotStatus::Harness(_)) {
                                chip("CLI ONLY", Intent::Neutral)
                            } else if g.seasonal {
                                chip("SEASON", Intent::Primary)
                            } else {
                                chip("READY", Intent::Success)
                            },
                        ]
                    }
                    other => {
                        let (text, intent) = match other {
                            SlotStatus::Locked => ("IN USE", Intent::Warning),
                            SlotStatus::Newer { .. } => ("NEWER", Intent::Warning),
                            _ => ("CORRUPT", Intent::Danger),
                        };
                        // A save held by another process cannot be read, but
                        // its file time still says when it was last written.
                        let dash = || Line::styled("—", t.faint());
                        let played = file_age(&slot.path, self.now_unix).map_or_else(dash, |ago| {
                            Line::styled(
                                format!("{} ago", crate::ui::spans::duration(ago)),
                                t.muted(),
                            )
                        });
                        vec![
                            Line::from(name),
                            dash(),
                            dash(),
                            dash(),
                            played,
                            chip(text, intent),
                        ]
                    }
                }
            })
            .collect();
        DataTable {
            theme: t,
            columns: vec![
                Column::fill("save"),
                Column::left("species", 10),
                Column::right("planets", 7),
                Column::right("fleet", 5),
                Column::right("played", 11),
                Column::left("state", 9),
            ],
            rows,
            selected: None,
            striped: true,
            surface: Surface::Base,
            groups: Vec::new(),
        }
    }

    pub fn draw(&self, frame: &mut Frame<'_>) {
        let t = self.theme();
        let area = frame.area();
        crate::ui::chrome::fill(frame.buffer_mut(), area, t.surface(Surface::Base));
        let [nav, body, status] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .areas(area);
        frame.render_widget(
            Navbar {
                theme: &t,
                mark: "HOSHI",
                crumbs: vec!["Title".into(), self.page_label().into()],
                right: Vec::new(),
            },
            nav,
        );
        frame.render_widget(
            StatusBar {
                theme: &t,
                crumbs: vec!["title".into(), self.page_label().into()],
                hints: vec![
                    ("↑↓".into(), "select".into()),
                    (self.config.keys.primary(Binding::Confirm), "open".into()),
                    (self.config.keys.primary(Binding::Help), "help".into()),
                    (self.config.keys.primary(Binding::Quit), "quit".into()),
                ],
                right: None,
            },
            status,
        );
        let inner = Rect {
            x: body.x + 2,
            y: body.y + 1,
            width: body.width.saturating_sub(4),
            height: body.height.saturating_sub(1),
        };
        let mut lines: Vec<Line> = Vec::new();
        if inner.width >= 40 && inner.height >= 10 {
            lines.push(Line::from(Span::styled("hoshi", t.strong())));
            lines.push(Line::styled(
                "an idle space-strategy game for the terminal",
                t.muted(),
            ));
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
                for (i, item) in self.menu_items().into_iter().enumerate() {
                    lines.push(if i == self.selected {
                        Line::from(vec![
                            Span::styled("▌ ", t.intent(Intent::Primary)),
                            Span::styled(format!("{item:<16}"), t.selected()),
                        ])
                    } else {
                        Line::styled(format!("  {item}"), t.text())
                    });
                }
                lines.push(Line::from(""));
                lines.push(Line::from(format!(
                    "{} help  {} copy seed",
                    self.config.keys.primary(Binding::Help),
                    self.config.keys.primary(Binding::CopySeed)
                )));
            }
            Page::Season => {
                let cat = SeasonCatalog::default();
                let window = cat.window(self.now_unix);
                match cat.current(self.now_unix) {
                    None => lines.push(Line::from("no season is active")),
                    Some(ruleset) => {
                        let left = format_remaining(window.end.saturating_sub(self.now_unix));
                        let seed = cat.seed_for(&ruleset.id, &window).unwrap_or(0);
                        let prestige = if ruleset.prestige {
                            "shares prestige"
                        } else {
                            "prestige isolated"
                        };
                        lines.push(Line::from(format!(
                            "Season  {}  {left}  {prestige}",
                            ruleset.name
                        )));
                        lines.push(Line::from(ruleset.blurb.clone()));
                        lines.push(Line::from(format!(
                            "seed {}  local UTC window, no network",
                            encode_galaxy_seed(seed)
                        )));
                        lines.push(Line::from(format!(
                            "{} starts this window. {} back. {} help.",
                            self.config.keys.primary(Binding::Confirm),
                            self.config.keys.primary(Binding::Cancel),
                            self.config.keys.primary(Binding::Help)
                        )));
                    }
                }
                lines.push(Line::from(""));
                lines.push(Line::from("Past seasons"));
                let mut any = false;
                for spec in self.achievements.iter() {
                    if !matches!(spec.condition, AchievementCondition::SeasonComplete { .. }) {
                        continue;
                    }
                    any = true;
                    let mark = if self.career.unlocks.contains_key(&spec.id) {
                        "finished"
                    } else {
                        "—"
                    };
                    lines.push(Line::from(format!("  {}  {mark}", spec.title)));
                }
                if !any {
                    lines.push(Line::from("  none recorded"));
                }
            }
            Page::New => {
                let spec = self.selected_species();
                let wide = inner.width >= 120;
                lines.push(Line::from(format!("New game  {}", spec.name)));
                lines.push(field_line(
                    "name",
                    &self.new_name,
                    self.new_field == NewField::Name,
                ));
                lines.push(field_line(
                    "seed (code or number, blank = random)",
                    &self.new_seed,
                    self.new_field == NewField::Seed,
                ));
                lines.push(field_line(
                    "difficulty",
                    self.new_difficulty.as_label(),
                    self.new_field == NewField::Difficulty,
                ));
                lines.push(field_line(
                    "species",
                    &spec.name,
                    self.new_field == NewField::Species,
                ));
                lines.push(field_line(
                    "rivals",
                    if self.new_rivals { "on" } else { "off" },
                    self.new_field == NewField::Rivals,
                ));
                if self.new_rivals {
                    for (i, row) in self.rival_catalog.iter().enumerate() {
                        let mark =
                            if i == self.new_rival_row && self.new_field == NewField::RivalRoster {
                                ">"
                            } else {
                                " "
                            };
                        let on = if self.new_personalities.iter().any(|id| id == &row.id) {
                            "[x]"
                        } else {
                            "[ ]"
                        };
                        lines.push(Line::from(format!("{mark} {on} {}", row.name)));
                    }
                    lines.push(field_line(
                        "rival difficulty",
                        &format!(
                            "{}  agg {:.1}  eff {:.1}  +{:.2}",
                            self.rival_difficulty_label(),
                            self.new_aggression,
                            self.new_efficiency,
                            self.new_starting_bonus
                        ),
                        self.new_field == NewField::RivalDifficulty,
                    ));
                }
                let summary = self.prestige_summary();
                if !summary.is_empty() {
                    lines.push(Line::from(summary));
                }
                for (i, row) in self.species_catalog.iter().enumerate() {
                    let mark = if i == self.new_species { ">" } else { " " };
                    let lock = if self.species_unlocked(row) {
                        String::new()
                    } else {
                        format!("  locked, {} prestige", row.unlock_cost)
                    };
                    lines.push(Line::from(format!("{mark} {}{lock}", row.name)));
                }
                if wide {
                    lines.extend(self.species_card_lines(spec, true));
                }
                lines.push(Line::from(self.seed_preview_line()));
                lines.push(Line::from(format!(
                    "{} on difficulty, species or rivals creates the slot. {} cycles species or rivals. {} back. {} help.",
                    self.config.keys.primary(Binding::Confirm),
                    self.config.keys.primary(Binding::Toggle),
                    self.config.keys.primary(Binding::Cancel),
                    self.config.keys.primary(Binding::Help)
                )));
            }
            Page::Load => {
                lines.push(Line::from(format!(
                    "Load game  {} play  {} delete  {} rename  {} copy  {} back  {} help",
                    self.config.keys.primary(Binding::Confirm),
                    self.config.keys.primary(Binding::DeleteSlot),
                    self.config.keys.primary(Binding::RenameSlot),
                    self.config.keys.primary(Binding::CopySeed),
                    self.config.keys.primary(Binding::Cancel),
                    self.config.keys.primary(Binding::Help)
                )));
                if self.slots.is_empty() {
                    lines.push(Line::from("no saves yet"));
                }
                let grouped = self.slots.iter().any(slot_is_seasonal);
                let mut last_seasonal: Option<bool> = None;
                for (i, slot) in self.slots.iter().enumerate() {
                    let seasonal = slot_is_seasonal(slot);
                    if grouped && last_seasonal != Some(seasonal) {
                        if last_seasonal.is_some() {
                            lines.push(Line::from(""));
                        }
                        lines.push(Line::from(if seasonal { "Seasons" } else { "Galaxies" }));
                        last_seasonal = Some(seasonal);
                    }
                    let mark = if i == self.selected { ">" } else { " " };
                    let mut status = self.slot_status_line(&slot.status, self.now_unix);
                    if matches!(slot.status, SlotStatus::Locked)
                        && let Some(ago) = file_age(&slot.path, self.now_unix)
                    {
                        status.push_str(&format!(
                            "  last written {} ago",
                            crate::ui::spans::duration(ago)
                        ));
                    }
                    lines.push(Line::from(format!("{mark} {}  {status}", slot_label(slot))));
                }
                if let Some(from) = &self.rename_from {
                    lines.push(Line::from(format!("rename {from} -> {}", self.rename_to)));
                }
            }
            Page::Settings => {
                lines.push(Line::from(format!(
                    "Settings  {} toggles  takes effect now  {} help",
                    self.config.keys.primary(Binding::Toggle),
                    self.config.keys.primary(Binding::Help)
                )));
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
                lines.push(toggle_line(
                    "portraits",
                    self.config.portraits,
                    self.selected == 4,
                ));
                lines.push(toggle_line(
                    "notifications (off by default)",
                    self.config.notify,
                    self.selected == 5,
                ));
                lines.push(toggle_line(
                    "tutorial hints",
                    self.config.tutorial,
                    self.selected == 6,
                ));
                lines.push(menu_line("reset tutorial hints", self.selected == 7));
                lines.push(menu_line("keybindings", self.selected == 8));
                lines.push(menu_line("advisor model", self.selected == 9));
                lines.push(menu_line("Help", self.selected == 10));
                lines.push(toggle_line(
                    "cheats (permanently modifies used saves)",
                    self.config.cheats,
                    self.selected == 11,
                ));
            }
            Page::Advisor => {
                let endpoint = if self.advisor_editing && self.advisor_selected == 1 {
                    format!("{}█", self.advisor_draft)
                } else if self.config.advisor_model_url.is_empty() {
                    "(none)".into()
                } else {
                    self.config.advisor_model_url.clone()
                };
                let model = if self.advisor_editing && self.advisor_selected == 2 {
                    format!("{}█", self.advisor_draft)
                } else if self.config.advisor_model_name.is_empty() {
                    "(none)".into()
                } else {
                    self.config.advisor_model_name.clone()
                };
                lines.push(Line::from(format!(
                    "Advisor  {} toggles  {} test  {} back  {} help",
                    self.config.keys.primary(Binding::Toggle),
                    self.config.keys.primary(Binding::Confirm),
                    self.config.keys.primary(Binding::Cancel),
                    self.config.keys.primary(Binding::Help)
                )));
                lines.push(toggle_line(
                    "local AI advisor (off by default)",
                    self.config.ai_advisor,
                    self.advisor_selected == 0,
                ));
                lines.push(menu_line(
                    &format!("endpoint  {endpoint}"),
                    self.advisor_selected == 1,
                ));
                lines.push(menu_line(
                    &format!("model     {model}"),
                    self.advisor_selected == 2,
                ));
                lines.push(toggle_line(
                    "allow private LAN host",
                    self.config.advisor_allow_lan,
                    self.advisor_selected == 3,
                ));
                let test = if self.advisor_probe.is_some() {
                    "test connection  (running)"
                } else {
                    "test connection"
                };
                lines.push(menu_line(test, self.advisor_selected == 4));
                if !self.advisor_probe_status.is_empty() {
                    lines.push(Line::from(self.advisor_probe_status.clone()));
                }
                if !cfg!(feature = "advisor-model") {
                    lines.push(Line::from(
                        "rebuild with --features advisor-model to rephrase suggestions",
                    ));
                }
            }
            Page::Keys => {
                let actions = Binding::REMAPPABLE;
                let start = self.keys_selected.saturating_sub(8);
                lines.push(Line::from(format!(
                    "Keybindings  {} rebind  {} reset  {} back  {} help",
                    self.config.keys.primary(Binding::Confirm),
                    self.config.keys.primary(Binding::DeleteSlot),
                    self.config.keys.primary(Binding::Cancel),
                    self.config.keys.primary(Binding::Help)
                )));
                let visible = actions.iter().copied().enumerate().skip(start).take(12);
                for (i, binding) in visible {
                    let mark = if i == self.keys_selected { ">" } else { " " };
                    let keys = self.config.keys.display(binding);
                    let screens = screens_for(binding);
                    lines.push(Line::from(format!(
                        "{mark} {keys:<12}  {}  {screens}",
                        HelpContext::label(binding)
                    )));
                }
                let reset_mark = if self.keys_selected == actions.len() {
                    ">"
                } else {
                    " "
                };
                lines.push(Line::from(format!("{reset_mark} reset all keys")));
            }
            Page::Achievements => {
                let earned = self.career.unlocks.len();
                let total = self.achievements.len();
                lines.push(Line::from(format!(
                    "Achievements and stats  {} back  {} help",
                    self.config.keys.primary(Binding::Cancel),
                    self.config.keys.primary(Binding::Help)
                )));
                lines.push(Line::from(format!(
                    "play {}s  battles {}  wonders {}",
                    self.career.playtime_secs, self.career.battles, self.career.wonders
                )));
                lines.push(Line::from(format!(
                    "galaxies  wonder {}  score {}",
                    self.career.galaxies_wonder, self.career.galaxies_score
                )));
                lines.push(Line::from(format!("earned {earned}/{total}")));
                let specs: Vec<_> = self.achievements.iter().collect();
                let start = self.stats_selected.saturating_sub(6);
                let meters = self.career.meters();
                for (i, spec) in specs.into_iter().enumerate().skip(start).take(10) {
                    let mark = if i == self.stats_selected { ">" } else { " " };
                    let unlocked = self.career.unlocks.contains_key(&spec.id);
                    let title = spec.listed_title(unlocked);
                    let line = if unlocked {
                        format!("{mark} {title}")
                    } else if spec.hidden {
                        format!("{mark} ???")
                    } else if let Some((current, target)) = spec.condition.measured(&meters) {
                        format!("{mark} {title}  {current:.0}/{target:.0}")
                    } else {
                        format!("{mark} {title}")
                    };
                    lines.push(Line::from(line));
                }
            }
            Page::Prestige => {
                lines.push(Line::from(format!(
                    "Prestige  run {}  {} unspent  lifetime {}",
                    self.career.prestige.current.index,
                    self.career.prestige.currency,
                    self.career.prestige.lifetime
                )));
                lines.push(Line::from(format!(
                    "{} buy (confirm twice)  {} back  {} help",
                    self.config.keys.primary(Binding::Confirm),
                    self.config.keys.primary(Binding::Cancel),
                    self.config.keys.primary(Binding::Help)
                )));
                for (i, spec) in self.prestige_cfg.bonuses.iter().enumerate() {
                    let mark = if i == self.prestige_selected {
                        ">"
                    } else {
                        " "
                    };
                    let owned = self.career.prestige.owned_count(&spec.id);
                    lines.push(Line::from(format!(
                        "{mark} {}  cost {}  owned {owned}",
                        spec.name, spec.cost
                    )));
                }
                let n_bonus = self.prestige_cfg.bonuses.len();
                for (i, spec) in self.locked_species().iter().enumerate() {
                    let mark = if n_bonus + i == self.prestige_selected {
                        ">"
                    } else {
                        " "
                    };
                    lines.push(Line::from(format!(
                        "{mark} unlock {}  cost {}",
                        spec.name, spec.unlock_cost
                    )));
                }
            }
            Page::Credits => {
                lines.push(Line::from(
                    "hoshi  an idle space-strategy game for the terminal",
                ));
                lines.push(Line::from(
                    "core owns the rules. the terminal only renders.",
                ));
                lines.push(Line::from(format!(
                    "{} back  {} help",
                    self.config.keys.primary(Binding::Confirm),
                    self.config.keys.primary(Binding::Help)
                )));
            }
        }
        if !self.status.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                self.status.clone(),
                Style::default().add_modifier(Modifier::ITALIC),
            )));
        }
        if self.page == Page::Menu && inner.width >= 90 {
            let header_rows = if inner.height >= 10 { 5 } else { 2 };
            let menu_lines = lines.split_off(header_rows.min(lines.len() as u16) as usize);
            let [header, below] =
                Layout::vertical([Constraint::Length(header_rows), Constraint::Min(1)])
                    .areas(inner);
            frame.render_widget(Paragraph::new(lines).style(t.text()), header);
            let [menu, _, saves] = Layout::horizontal([
                Constraint::Length(34),
                Constraint::Length(2),
                Constraint::Min(40),
            ])
            .areas(below);
            frame.render_widget(Paragraph::new(menu_lines).style(t.text()), menu);
            frame.render_widget(
                Paragraph::new(crate::ui::panels::section(&t, "saves")),
                saves,
            );
            let table = Rect {
                y: saves.y + 1,
                height: saves.height.saturating_sub(1),
                ..saves
            };
            if self.slots.is_empty() {
                frame.render_widget(
                    Paragraph::new(vec![
                        Line::from(vec![
                            Span::styled("▌ ", t.intent(Intent::Primary)),
                            Span::styled("No saves yet", t.strong()),
                        ]),
                        Line::from(vec![
                            Span::styled("▌ ", t.intent(Intent::Primary)),
                            Span::styled("New game starts one in this folder.", t.muted()),
                        ]),
                    ]),
                    table,
                );
            } else {
                frame.render_widget(self.save_table(&t), table);
            }
        } else {
            frame.render_widget(Paragraph::new(lines).style(t.text()), inner);
        }
        if self.help_open {
            let mut help_lines = if self.help_commands {
                crate::help::command_lines("bridge")
            } else {
                let mut lines = self.config.keys.help_lines(self.help_context());
                lines.insert(0, "[Keys]   Commands".into());
                lines
            };
            help_lines.push("v Changelog   u dismiss update".into());
            help_lines.push("p Support report".into());
            if self.config.bug_report_target != "off" {
                help_lines.push("b Report a bug".into());
            }
            help_lines.push("tab page   ? close   esc close".into());
            let help = help_lines
                .into_iter()
                .map(|line| Line::styled(line, t.text()))
                .collect::<Vec<_>>();
            crate::ui::chrome::dim(frame.buffer_mut(), area, &t);
            let width = area.width.saturating_sub(4).min(84);
            let height = (help.len() as u16 + 4).min(area.height.saturating_sub(2));
            let rect = Rect {
                x: area.x + (area.width - width) / 2,
                y: area.y + (area.height - height) / 2,
                width,
                height,
            };
            frame.render_widget(Clear, rect);
            let dialog = Dialog {
                theme: &t,
                title: "Help".into(),
                subtitle: String::new(),
                close_hint: self.config.keys.primary(Binding::Cancel),
                footer_note: None,
                buttons: Vec::new(),
            };
            let help_inner = dialog.inner(rect);
            frame.render_widget(dialog, rect);
            frame.render_widget(
                Paragraph::new(help),
                Rect {
                    x: help_inner.x + 2,
                    y: help_inner.y + 1,
                    width: help_inner.width.saturating_sub(4),
                    height: help_inner.height.saturating_sub(1),
                },
            );
        }
        if let Some(updates) = &self.updates {
            updates.draw(frame);
        }
        if self.crash_notice.is_some() && self.support.is_none() {
            let area = frame.area();
            frame.render_widget(
                Paragraph::new("Previous session crashed.  r Report this crash   Esc later").style(
                    ratatui::style::Style::default()
                        .fg(ratatui::style::Color::White)
                        .bg(ratatui::style::Color::Black),
                ),
                Rect::new(area.x, area.y.saturating_add(1), area.width, 1),
            );
        }
        if let Some(report) = &self.support {
            report.draw(frame);
        }
        if let Some(bug) = &self.bug {
            bug.draw(frame);
        }
    }

    pub fn new_game_spec(&self) -> NewGameSpec {
        NewGameSpec {
            path: self
                .play_path
                .clone()
                .unwrap_or_else(|| self.dir.join(format!("{}.sqlite", self.new_name))),
            seed: parse_seed(&self.new_seed).ok().flatten(),
            difficulty: self.new_difficulty.as_label().to_string(),
            prestige: self.career.prestige.clone(),
            season_id: self.season_id.clone(),
            species_id: self.selected_species().id.clone(),
            rivals: self.rival_settings(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct NewGameSpec {
    pub path: PathBuf,
    pub seed: Option<u64>,
    pub difficulty: String,
    pub prestige: Prestige,
    pub season_id: Option<String>,
    pub species_id: String,
    pub rivals: RivalSettings,
}

pub fn parse_seed(text: &str) -> Result<Option<u64>, String> {
    parse_galaxy_seed(text)
}

fn slot_is_seasonal(slot: &Slot) -> bool {
    match &slot.status {
        SlotStatus::Ready(g) | SlotStatus::Harness(g) => g.seasonal,
        _ => slot.name.starts_with("season-"),
    }
}

fn slot_label(slot: &Slot) -> String {
    match &slot.status {
        SlotStatus::Ready(g) | SlotStatus::Harness(g) if g.seasonal => g
            .season_id
            .as_deref()
            .and_then(|id| SeasonCatalog::default().get(id).map(|row| row.name.clone()))
            .unwrap_or_else(|| slot.name.clone()),
        _ => slot.name.clone(),
    }
}

fn format_remaining(secs: u64) -> String {
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3_600;
    if days > 0 {
        format!("{days}d {hours}h left")
    } else if hours > 0 {
        format!("{hours}h left")
    } else {
        format!("{}m left", secs / 60)
    }
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
        if name.contains(".mikura.state.") {
            continue;
        }
        let stem = if name.ends_with(".mikura") {
            name.trim_end_matches(".mikura").to_string()
        } else if name.ends_with(".chisei") {
            name.trim_end_matches(".chisei").to_string()
        } else if name.ends_with(".sqlite") {
            name.trim_end_matches(".sqlite").to_string()
        } else {
            continue;
        };
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
        Err(SaveError::SchemaTooNew { found, .. }) => SlotStatus::Newer {
            found,
            kind: "schema",
        },
        Err(SaveError::GeneratorTooNew { found, .. }) => SlotStatus::Newer {
            found,
            kind: "generator",
        },
        Err(SaveError::Corrupt(msg)) => SlotStatus::Corrupt(msg.to_string()),
        Err(e) => SlotStatus::Corrupt(e.to_string()),
    }
}

/// Seconds since a save file was last written, read from the file system
/// so it works for a save another process holds.
fn file_age(path: &Path, now_unix: u64) -> Option<u64> {
    let at = std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()?;
    let secs = at.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    Some(now_unix.saturating_sub(secs))
}

fn newer_message(kind: &str, found: u32) -> String {
    format!("made by a newer hoshi ({kind} {found}); please update")
}

pub fn slot_message(status: &SlotStatus) -> String {
    match status {
        SlotStatus::Locked => "in use by another hoshi process".into(),
        SlotStatus::Newer { found, kind } => newer_message(kind, *found),
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

fn screens_for(binding: Binding) -> String {
    const CTX: &[HelpContext] = &[
        HelpContext::Overview,
        HelpContext::Planet,
        HelpContext::Shipyard,
        HelpContext::Wonder,
        HelpContext::Prestige,
        HelpContext::Achievements,
        HelpContext::Daily,
        HelpContext::Production,
        HelpContext::Market,
        HelpContext::Diplomacy,
        HelpContext::Laws,
        HelpContext::Reports,
        HelpContext::History,
        HelpContext::Staff,
        HelpContext::Ops,
        HelpContext::Map,
        HelpContext::Cockpit,
        HelpContext::Dispatch,
        HelpContext::Away,
        HelpContext::Replay,
        HelpContext::Story,
        HelpContext::Tactical,
        HelpContext::ReportView,
        HelpContext::TitleMenu,
        HelpContext::TitleNew,
        HelpContext::TitleLoad,
        HelpContext::TitleDelete,
        HelpContext::TitleRename,
        HelpContext::TitleSettings,
        HelpContext::TitleAdvisor,
        HelpContext::TitleKeys,
        HelpContext::TitleAchievements,
        HelpContext::TitlePrestige,
        HelpContext::TitleCredits,
    ];
    let mut names: Vec<&str> = CTX
        .iter()
        .copied()
        .filter(|ctx| ctx.bindings().contains(&binding))
        .map(HelpContext::name)
        .collect();
    if binding == Binding::Activity || binding == Binding::Advisor {
        names.insert(0, "play");
    }
    if names.is_empty() {
        String::new()
    } else {
        names.join(",")
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
    config.starting_resources.iron *= scale;
    config.starting_resources.silicon *= scale;
    config.starting_resources.deuterium *= scale;
}

pub struct AwaySnapshot {
    pub tick: u64,
    pub builds: u32,
    pub research: u32,
    pub deliveries: usize,
    pub stalls: usize,
    pub completed: usize,
}

pub fn snapshot_away(state: &hoshi_core::GameState) -> AwaySnapshot {
    AwaySnapshot {
        tick: state.last_tick_unix,
        builds: building_levels(state),
        research: research_levels(state),
        deliveries: state.logistics.deliveries.len(),
        stalls: state.logistics.stalls.len(),
        completed: state.logistics.completed.len(),
    }
}

/// Summarize a gap from the snapshot taken before
/// [`hoshi_core::GameState::catch_up_ledger`] and the ledger it returned.
pub fn summarize_away(
    before: &AwaySnapshot,
    ledger: hoshi_core::CatchUpLedger,
    after: &hoshi_core::GameState,
) -> AwaySummary {
    AwaySummary {
        elapsed_secs: after.last_tick_unix.saturating_sub(before.tick),
        ledger,
        builds: after_building_levels(after).saturating_sub(before.builds),
        research: after
            .tech_levels
            .values()
            .copied()
            .sum::<u32>()
            .saturating_sub(before.research),
        deliveries: after
            .logistics
            .deliveries
            .iter()
            .skip(before.deliveries)
            .cloned()
            .collect(),
        stalls: after
            .logistics
            .stalls
            .iter()
            .skip(before.stalls)
            .cloned()
            .collect(),
        completed: after
            .logistics
            .completed
            .iter()
            .skip(before.completed)
            .cloned()
            .collect(),
        // The event log is capped, so new events are told apart by time, not
        // by how many there were before.
        events: after
            .delivered_events()
            .filter(|e| e.at > before.tick)
            .map(|e| e.text.clone())
            .collect(),
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
    use hoshi_core::{
        BalanceConfig, GameState, Prestige, RivalCatalog, RivalSettings, SCHEMA_VERSION,
        SeasonCatalog, SpeciesCatalog, load_state, lock_save, open_save, save_is_locked,
        save_state,
    };
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
    fn settings_help_opens_both_pages_without_opening_a_save() {
        let dir = unique_dir();
        let mut title = Title::new(dir.clone(), 1_000);
        title.page = Page::Settings;
        title.selected = 10;
        title.handle_key(KeyCode::Enter);
        assert!(render(&title, 80, 24).contains("[Keys]"));
        title.handle_key(KeyCode::Tab);
        let commands = render(&title, 80, 24);
        for command in ["hoshi state", "hoshi advise", "hoshi goals"] {
            assert!(commands.contains(command), "{commands}");
        }
        title.handle_key(KeyCode::Esc);
        assert!(!title.help_open);
        std::fs::remove_dir_all(dir).unwrap();
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
        assert!(text.contains("Achievements"), "{text}");
        assert!(
            text.contains("> Continue") || text.contains("Continue"),
            "{text}"
        );
        let small = render(&title, 80, 24);
        let _ = small;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn season_page_shows_the_current_window_seed_and_past_results() {
        let dir = unique_dir();
        let mut title = Title::new(dir.clone(), 1_800_000_000);
        // No saves, so the menu starts on New game; one step reaches the page.
        title.handle_key(KeyCode::Char('j'));
        title.handle_key(KeyCode::Enter);
        let text = render(&title, 80, 24);
        assert!(text.contains("Closed markets"), "{text}");
        assert!(text.contains("left"), "{text}");
        assert!(text.contains("seed"), "{text}");
        assert!(text.contains("Past seasons"), "{text}");
        assert!(text.contains("Double haul"), "{text}");
        assert!(text.contains("Pirate weather"), "{text}");
        assert!(text.contains("local UTC window, no network"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn season_menu_hides_when_disabled_or_before_epoch() {
        let dir = unique_dir();
        let mut title = Title::new(dir.clone(), 1_800_000_000);
        let menu = render(&title, 80, 24);
        assert!(menu.contains("Season"), "{menu}");
        title.config.season = false;
        let hidden = render(&title, 80, 24);
        assert!(!hidden.contains("Season"), "{hidden}");
        title.config.season = true;
        title.now_unix = 1;
        let early = render(&title, 80, 24);
        assert!(!early.contains("Season"), "{early}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wide_menu_shows_saves_as_a_table() {
        let dir = unique_dir();
        let empty = Title::new(dir.clone(), 1_000);
        let text = render(&empty, 120, 40);
        assert!(text.contains("No saves yet"), "{text}");
        write_save(&dir, "live", &GameState::new(10));
        let title = Title::new(dir.clone(), 3_610);
        let text = render(&title, 120, 40);
        assert!(text.contains("HOSHI"), "{text}");
        assert!(text.contains("Continue"), "{text}");
        assert!(text.contains("SAVES"), "{text}");
        assert!(text.contains("▸ live"), "{text}");
        assert!(text.contains("Humans"), "{text}");
        assert!(text.contains("1h 00m ago"), "{text}");
        assert!(text.contains("READY"), "{text}");
        let narrow = render(&title, 80, 24);
        assert!(narrow.contains("Continue"), "{narrow}");
        assert!(!narrow.contains("SAVES"), "{narrow}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_groups_seasonal_saves_apart_from_galaxies() {
        let dir = unique_dir();
        write_save(&dir, "live", &GameState::new(10));
        let cat = SeasonCatalog::default();
        let seasonal =
            GameState::new_season(1_800_000_000, &cat, "double_haul", BalanceConfig::default())
                .unwrap();
        write_save(&dir, "season-double_haul-1704067200", &seasonal);
        let mut title = Title::new(dir.clone(), 1_800_000_000);
        title.page = Page::Load;
        let text = render(&title, 80, 24);
        assert!(text.contains("Galaxies"), "{text}");
        assert!(text.contains("Seasons"), "{text}");
        assert!(text.contains("Double haul"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn past_season_results_use_stored_unlocks() {
        let dir = unique_dir();
        let mut title = Title::new(dir.clone(), 1_800_000_000);
        title.page = Page::Season;
        title.career.unlocks.insert("season_double_haul".into(), 1);
        let text = render(&title, 80, 24);
        assert!(text.contains("finished"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn summarize_away_includes_a_spy_event_that_was_pending() {
        let mut state = GameState::new(0);
        state.events.push(hoshi_core::GameEvent {
            at: 0,
            text: "old".into(),
        });
        state.last_tick_unix = 10;
        state.events.push(hoshi_core::GameEvent {
            at: 100,
            text: "spy 1:2:3 activity detected=no lost=0/1".into(),
        });
        let before = snapshot_away(&state);
        state.last_tick_unix = 100;
        let summary = summarize_away(&before, hoshi_core::CatchUpLedger::default(), &state);
        assert!(
            summary.events.iter().any(|text| text.contains("spy")),
            "{:?}",
            summary.events
        );
    }

    #[test]
    fn away_summary_equals_the_saved_and_reloaded_delta() {
        let mut state = GameState::with_config(0, BalanceConfig::default());
        state.next_raid_at = Some(3_600);
        let before = snapshot_away(&state);
        let ledger = state.catch_up_ledger(10 * 3_600);
        let summary = summarize_away(&before, ledger, &state);
        assert_eq!(summary.ledger.raids() as usize, state.combat_logs.len());
        assert!(summary.ledger.raids_lost >= 1);
        let mut conn = open_save(std::path::Path::new(":memory:")).unwrap();
        save_state(&mut conn, &state).unwrap();
        let loaded = load_state(&conn, BalanceConfig::default()).unwrap();
        for planet in &summary.ledger.planets {
            let saved = loaded.resources_at(planet.coord).unwrap();
            for kind in hoshi_core::ResourceKind::ALL {
                let delta = saved.get(kind) - planet.before.get(kind);
                assert!(
                    (planet.net(kind) - delta).abs() < 1e-9,
                    "{kind:?}: summary {} saved {delta}",
                    planet.net(kind)
                );
            }
        }
    }

    #[test]
    fn summarize_away_keeps_new_events_when_the_log_is_full() {
        let mut state = GameState::new(0);
        for at in 0..32 {
            state.events.push(hoshi_core::GameEvent {
                at,
                text: format!("old {at}"),
            });
        }
        state.last_tick_unix = 40;
        let before = snapshot_away(&state);
        state.events.drain(..2);
        for at in [50, 60] {
            state.events.push(hoshi_core::GameEvent {
                at,
                text: format!("new {at}"),
            });
        }
        state.last_tick_unix = 60;
        let summary = summarize_away(&before, hoshi_core::CatchUpLedger::default(), &state);
        assert_eq!(summary.events, vec!["new 50", "new 60"]);
    }

    #[test]
    fn summarize_away_lists_deliveries_stalls_and_completed() {
        let mut state = GameState::new(0);
        let before = snapshot_away(&state);
        state
            .logistics
            .deliveries
            .push("delivered 10 metal to 1:1:2".into());
        state
            .logistics
            .stalls
            .push("stall: full storage (metal) at 1:1:1".into());
        state.logistics.completed.push("metal mine lv 2".into());
        state.last_tick_unix = 60;
        let summary = summarize_away(&before, hoshi_core::CatchUpLedger::default(), &state);
        assert_eq!(summary.elapsed_secs, 60);
        assert_eq!(summary.deliveries, vec!["delivered 10 metal to 1:1:2"]);
        assert_eq!(summary.stalls, vec!["stall: full storage (metal) at 1:1:1"]);
        assert_eq!(summary.completed, vec!["metal mine lv 2"]);
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
    fn new_game_keeps_rivals_off_until_toggled() {
        let dir = unique_dir();
        let mut title = Title::new(dir.clone(), 100);
        title.page = Page::New;
        let text = render(&title, 80, 24);
        assert!(text.contains("rivals"), "{text}");
        assert!(text.contains("off"), "{text}");
        assert!(!text.contains("Greedy Trader"), "{text}");
        assert!(!title.new_game_spec().rivals.enabled);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn new_game_lists_rival_personalities_from_data_files() {
        let dir = unique_dir();
        let mut title = Title::new(dir.clone(), 100);
        title.page = Page::New;
        title.new_field = NewField::Rivals;
        title.handle_key(KeyCode::Char(' '));
        let text = render(&title, 80, 24);
        for spec in RivalCatalog::default().iter() {
            assert!(text.contains(&spec.name), "missing {} in {text}", spec.name);
        }
        assert!(text.contains("[x]"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rivals_on_with_zero_personalities_is_refused() {
        let dir = unique_dir();
        let mut title = Title::new(dir.clone(), 100);
        title.page = Page::New;
        title.new_field = NewField::Rivals;
        title.handle_key(KeyCode::Char(' '));
        title.new_personalities.clear();
        title.new_field = NewField::RivalRoster;
        let action = title.handle_key(KeyCode::Enter);
        assert_eq!(action, TitleAction::None);
        assert!(title.status.contains("personality"), "{}", title.status);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_slot_shows_rival_personalities() {
        let dir = unique_dir();
        let mut state = GameState::new(10);
        state
            .spawn_rivals(&RivalCatalog::default(), RivalSettings::on())
            .unwrap();
        write_save(&dir, "war", &state);
        let mut title = Title::new(dir.clone(), 20);
        title.page = Page::Load;
        let text = render(&title, 80, 24);
        assert!(text.contains("rivals"), "{text}");
        assert!(
            text.contains("Greedy Trader") || text.contains("trader"),
            "{text}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn new_game_lists_every_species_as_a_list_at_80x24() {
        let dir = unique_dir();
        let mut title = Title::new(dir.clone(), 100);
        title.page = Page::New;
        let text = render(&title, 80, 24);
        for spec in SpeciesCatalog::default().iter() {
            assert!(text.contains(&spec.name), "missing {} in {text}", spec.name);
        }
        assert!(text.contains("locked, 2 prestige"), "{text}");
        assert!(!text.contains("start standing"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn new_game_shows_a_species_card_on_wide_terminals() {
        let dir = unique_dir();
        let mut title = Title::new(dir.clone(), 100);
        title.page = Page::New;
        let text = render(&title, 160, 48);
        assert!(text.contains("lean diplomacy"), "{text}");
        assert!(text.contains("start standing"), "{text}");
        assert!(text.contains("trade and diplomacy"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn locked_species_are_visible_but_do_not_create_a_save() {
        let dir = unique_dir();
        let mut title = Title::new(dir.clone(), 100);
        title.page = Page::New;
        title.new_field = NewField::Species;
        title.handle_key(KeyCode::Char(' '));
        assert_eq!(title.selected_species().id, "kryth");
        let action = title.handle_key(KeyCode::Enter);
        assert_eq!(action, TitleAction::None);
        assert!(title.status.contains("locked"), "{}", title.status);
        assert!(!dir.join("save.sqlite").exists() && title.play_path.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_slot_shows_the_chosen_species() {
        let dir = unique_dir();
        let mut prestige = Prestige::default();
        prestige.unlocked_species.insert("aurelians".into());
        let state = GameState::new_with_species(
            10,
            &SpeciesCatalog::default(),
            "aurelians",
            BalanceConfig::default(),
            7,
            prestige,
        )
        .unwrap();
        write_save(&dir, "alchemy", &state);
        let mut title = Title::new(dir.clone(), 20);
        title.page = Page::Load;
        let text = render(&title, 80, 24);
        assert!(text.contains("Aurelians"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn new_game_rejects_an_invalid_seed() {
        let dir = unique_dir();
        let mut title = Title::new(dir.clone(), 100);
        assert_eq!(title.menu_items()[title.selected], "New game");
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
        title.selected = title
            .menu_items()
            .iter()
            .position(|item| *item == "Load game")
            .expect("Load game");
        title.handle_key(KeyCode::Enter);
        let text = render(&title, 100, 30);
        assert!(text.contains("in use by another hoshi process"), "{text}");
        assert!(text.contains("newer hoshi"), "{text}");
        assert!(text.contains("corrupt"), "{text}");
        assert!(text.contains("harness save"), "{text}");
        // The locked row still says when its file was last written.
        let wide = render(&title, 140, 30);
        let busy_row = wide.lines().find(|line| line.contains("busy")).unwrap();
        assert!(busy_row.contains("last written"), "{busy_row}");

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
    fn load_omits_seed_for_pre_galaxy_saves() {
        let dir = unique_dir();
        write_save(&dir, "legacy", &GameState::new(10));
        {
            let conn = open_save(&dir.join("legacy.sqlite")).unwrap();
            conn.execute_batch(
                "ALTER TABLE meta DROP COLUMN galaxy_seed; PRAGMA user_version = 4;",
            )
            .unwrap();
        }
        let mut title = Title::new(dir.clone(), 40);
        title.page = Page::Load;
        let fabricated = hoshi_core::encode_galaxy_seed(0);
        let text = render(&title, 100, 30);
        assert!(!text.contains(&fabricated), "{text}");
        title.selected = title.slots.iter().position(|s| s.name == "legacy").unwrap();
        title.handle_key(KeyCode::Char('y'));
        assert!(title.status.contains("no seed"), "{}", title.status);
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
        title.selected = 4;
        title.handle_key(KeyCode::Char(' '));
        assert!(!title.config.portraits);
        let loaded = UserConfig::load(&dir);
        assert!(loaded.reduced_motion);
        assert!(!loaded.bridge);
        assert!(!loaded.ai_advisor);
        assert!(!loaded.portraits);
        title.selected = 5;
        title.handle_key(KeyCode::Char(' '));
        assert!(title.config.notify);
        let loaded = UserConfig::load(&dir);
        assert!(loaded.notify);
        let settings = render(&title, 80, 24);
        assert!(settings.contains("advisor model"), "{settings}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn advisor_settings_save_to_config_not_the_save() {
        let dir = unique_dir();
        let mut title = Title::new(dir.clone(), 1);
        title.page = Page::Advisor;
        title.advisor_selected = 0;
        assert!(!title.config.ai_advisor);
        title.handle_key(KeyCode::Char(' '));
        assert!(title.config.ai_advisor);
        title.advisor_selected = 3;
        title.handle_key(KeyCode::Char(' '));
        assert!(title.config.advisor_allow_lan);
        title.advisor_selected = 1;
        title.handle_key(KeyCode::Enter);
        for c in "http://8.8.8.8/v1".chars() {
            title.handle_key(KeyCode::Char(c));
        }
        title.handle_key(KeyCode::Enter);
        assert!(title.config.advisor_model_url.is_empty());
        assert!(
            title.status.contains("loopback") || title.status.contains("LAN"),
            "{}",
            title.status
        );
        title.advisor_draft.clear();
        title.advisor_editing = true;
        title.advisor_selected = 1;
        for c in "http://127.0.0.1:11434/v1".chars() {
            title.handle_key(KeyCode::Char(c));
        }
        title.handle_key(KeyCode::Enter);
        assert_eq!(title.config.advisor_model_url, "http://127.0.0.1:11434/v1");
        title.advisor_selected = 2;
        title.handle_key(KeyCode::Enter);
        for c in "llama3".chars() {
            title.handle_key(KeyCode::Char(c));
        }
        title.handle_key(KeyCode::Enter);
        assert_eq!(title.config.advisor_model_name, "llama3");
        let loaded = UserConfig::load(&dir);
        assert!(loaded.ai_advisor);
        assert_eq!(loaded.advisor_model_url, "http://127.0.0.1:11434/v1");
        assert_eq!(loaded.advisor_model_name, "llama3");
        assert!(loaded.advisor_allow_lan);
        title.advisor_selected = 4;
        let started = std::time::Instant::now();
        title.handle_key(KeyCode::Enter);
        assert!(started.elapsed() < std::time::Duration::from_millis(50));
        assert!(title.advisor_probe.is_some());
        for _ in 0..20 {
            title.tick();
            if title.advisor_probe.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(title.advisor_probe.is_none());
        assert!(
            title.advisor_probe_status.contains("ok")
                || title.advisor_probe_status.contains("failed"),
            "{}",
            title.advisor_probe_status
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn help_overlay_lists_title_menu_actions() {
        let dir = unique_dir();
        let mut title = Title::new(dir.clone(), 100);
        title.handle_key(KeyCode::Char('?'));
        let text = render(&title, 80, 24);
        assert!(text.contains("help"), "{text}");
        assert!(text.contains("keys for this screen"), "{text}");
        assert!(text.contains("quit"), "{text}");
        title.handle_key(KeyCode::Char('?'));
        let closed = render(&title, 80, 24);
        assert!(closed.contains("Continue"), "{closed}");
        assert!(!closed.contains("keys for this screen"), "{closed}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prestige_page_is_hidden_until_a_reset_is_available() {
        let dir = unique_dir();
        write_save(&dir, "fresh", &GameState::new(1));
        let title = Title::new(dir.clone(), 1);
        let text = render(&title, 80, 24);
        assert!(!text.contains("Prestige"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prestige_page_buys_with_confirmation_and_new_game_lists_bonuses() {
        let dir = unique_dir();
        let mut state = GameState::new(10);
        state.completed_wonders.push("observatory".into());
        state.prestige.currency = 3;
        state.prestige.lifetime = 3;
        state.prestige.runs.push(hoshi_core::RunStats::new(1, 0));
        write_save(&dir, "alpha", &state);
        let mut title = Title::new(dir.clone(), 20);
        let menu = render(&title, 80, 24);
        assert!(menu.contains("Prestige"), "{menu}");
        title.page = Page::Prestige;
        title.handle_key(KeyCode::Enter);
        assert!(title.status.contains("confirm"), "{}", title.status);
        title.handle_key(KeyCode::Enter);
        assert!(title.status.contains("bought"), "{}", title.status);
        let conn = open_save(&dir.join("alpha.sqlite")).unwrap();
        let loaded = load_state(&conn, BalanceConfig::default()).unwrap();
        assert_eq!(loaded.prestige.owned_count("starter_cache"), 1);
        title.page = Page::New;
        let new_text = render(&title, 80, 24);
        assert!(new_text.contains("starter cache"), "{new_text}");
        let spec = title.new_game_spec();
        assert_eq!(spec.prestige.owned_count("starter_cache"), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn keybindings_page_rebinds_rejects_conflicts_and_resets() {
        let dir = unique_dir();
        let mut title = Title::new(dir.clone(), 1);
        title.page = Page::Keys;
        let text = render(&title, 80, 24);
        assert!(text.contains("quit"), "{text}");
        assert!(text.contains("overview"), "{text}");
        title.handle_key(KeyCode::Enter);
        title.handle_key(KeyCode::Char('p'));
        assert!(title.config.keys.matches(Binding::Quit, KeyCode::Char('p')));
        let loaded = UserConfig::load(&dir);
        assert!(loaded.keys.matches(Binding::Quit, KeyCode::Char('p')));
        let listed = render(&title, 80, 24);
        assert!(listed.contains("p"), "{listed}");
        title.handle_key(KeyCode::Char('x'));
        title.handle_key(KeyCode::Char('x'));
        title.handle_key(KeyCode::Char('?'));
        let help = render(&title, 80, 24);
        assert!(help.contains("p"), "{help}");
        title.handle_key(KeyCode::Char('?'));
        title.page = Page::Keys;
        title.keys_selected = 0;
        title.handle_key(KeyCode::Enter);
        title.handle_key(KeyCode::Char('j'));
        assert!(title.status.contains("conflict"), "{}", title.status);
        assert!(title.config.keys.matches(Binding::Quit, KeyCode::Char('p')));
        let copy = Binding::REMAPPABLE
            .iter()
            .position(|b| *b == Binding::CopySeed)
            .unwrap();
        title.keys_selected = copy;
        title.handle_key(KeyCode::Enter);
        title.handle_key(KeyCode::Char('q'));
        assert!(
            title
                .config
                .keys
                .matches(Binding::CopySeed, KeyCode::Char('q'))
        );
        title.keys_selected = 0;
        title.handle_key(KeyCode::Char('d'));
        title.handle_key(KeyCode::Enter);
        assert!(title.status.contains("conflict"), "{}", title.status);
        assert!(title.config.keys.matches(Binding::Quit, KeyCode::Char('p')));
        title.keys_selected = copy;
        title.handle_key(KeyCode::Char('d'));
        title.handle_key(KeyCode::Enter);
        assert!(
            title
                .config
                .keys
                .matches(Binding::CopySeed, KeyCode::Char('y'))
        );
        title.keys_selected = 0;
        title.handle_key(KeyCode::Char('d'));
        title.handle_key(KeyCode::Enter);
        assert!(title.config.keys.matches(Binding::Quit, KeyCode::Char('q')));
        title.keys_selected = Binding::REMAPPABLE.len();
        title.handle_key(KeyCode::Enter);
        title.handle_key(KeyCode::Enter);
        assert_eq!(title.config.keys, Keymap::defaults());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn achievements_page_lists_unlocks_hides_locked_and_aggregates_stats() {
        let dir = unique_dir();
        let mut alpha = GameState::new(50);
        alpha.last_tick_unix = 150;
        alpha.achievements.insert("first_cut".into(), 7);
        write_save(&dir, "alpha", &alpha);
        let mut beta = GameState::new(20);
        beta.last_tick_unix = 80;
        beta.achievements.insert("lab_open".into(), 9);
        beta.completed_wonders.push("observatory".into());
        write_save(&dir, "beta", &beta);
        let mut title = Title::new(dir.clone(), 200);
        title.selected = title
            .menu_items()
            .iter()
            .position(|item| *item == "Achievements")
            .expect("Achievements");
        title.handle_key(KeyCode::Enter);
        let text = render(&title, 80, 24);
        assert!(text.contains("Achievements and stats"), "{text}");
        assert!(text.contains("First cut"), "{text}");
        assert!(text.contains("Lab open"), "{text}");
        assert!(text.contains("160s"), "{text}");
        assert!(text.contains("wonders 1"), "{text}");
        assert!(text.contains("earned 2/"), "{text}");
        let hidden = title
            .achievements
            .iter()
            .position(|spec| spec.hidden)
            .expect("hidden achievement");
        title.stats_selected = hidden;
        let hidden_text = render(&title, 80, 24);
        assert!(hidden_text.contains("???"), "{hidden_text}");
        assert!(!hidden_text.contains("Armada"), "{hidden_text}");
        assert!(!save_is_locked(&dir.join("alpha.sqlite")));
        assert!(!save_is_locked(&dir.join("beta.sqlite")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_seed_accepts_blank_and_u64_only() {
        assert_eq!(parse_seed("").unwrap(), None);
        assert_eq!(parse_seed("42").unwrap(), Some(42));
        let code = hoshi_core::encode_galaxy_seed(42);
        assert_eq!(parse_seed(&code).unwrap(), Some(42));
        assert!(parse_seed("nope").is_err());
        let mut title = Title::new(unique_dir(), 1);
        title.page = Page::New;
        title.new_field = NewField::Seed;
        title.new_seed = code.clone();
        let text = render(&title, 80, 24);
        assert!(text.contains(&code), "{text}");
        assert!(text.contains("rocky"), "{text}");
        assert!(text.contains("generator 1"), "{text}");
        title.new_seed = "abc".into();
        let bad = render(&title, 80, 24);
        assert!(bad.contains("invalid seed"), "{bad}");
        let mut tampered: Vec<char> = code.chars().collect();
        let last = tampered.len() - 1;
        tampered[last] = if tampered[last] == '0' { '1' } else { '0' };
        title.new_seed = tampered.into_iter().collect();
        let checksum = render(&title, 80, 24);
        assert!(checksum.contains("checksum"), "{checksum}");
        let _ = std::fs::remove_dir_all(&title.dir);
    }

    #[test]
    fn future_generator_is_newer_generator_not_schema() {
        let dir = unique_dir();
        write_save(&dir, "gen", &GameState::new(1));
        let path = dir.join("gen.sqlite");
        let conn = open_save(&path).unwrap();
        conn.execute(
            "UPDATE meta SET generator_version = ?1 WHERE id = 1",
            [hoshi_core::GALAXY_GENERATOR_VERSION as i64 + 1],
        )
        .unwrap();
        drop(conn);
        let status = classify_slot(&path);
        let found = hoshi_core::GALAXY_GENERATOR_VERSION + 1;
        assert_eq!(
            status,
            SlotStatus::Newer {
                found,
                kind: "generator"
            }
        );
        let msg = slot_message(&status);
        assert!(msg.contains("generator"), "{msg}");
        assert!(!msg.contains("schema"), "{msg}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
