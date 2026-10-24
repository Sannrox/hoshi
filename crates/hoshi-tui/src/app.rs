//! Overview, planet, shipyard, wonder, prestige, achievements, daily, goals, production, market, diplomacy, laws, reports, ops and map screens.
//! Core owns the rules; this module only renders and turns keys into core commands.

mod dismantle;
mod mission_form;

use crate::clip::copy_to_clipboard;
use crate::detail_panel::{self, DOCK_MIN_WIDTH};
use crate::hubs::{self, Hub};
use crate::keymap::{Binding, HelpContext, Keymap};
use crate::notify::{self, Prefs as NotifyPrefs, Watch as NotifyWatch};
use crate::portraits::{self, PortraitView, Protocol, Slot};
use crate::title::AwaySummary;
use crate::ui::chrome::{Dialog, Navbar, StatusBar, Tabs};
use crate::ui::panels::{Metric, MetricStrip};
use crate::ui::table::{Column, DataTable};
use crate::ui::{self, Depth, Intent, Surface, Theme};
use hoshi_core::{
    Actor, AssignmentTarget, BuildingKind, CombatLog, CombatWinner, CommentaryCatalog, Coord,
    DefenceKind, Detail, DetailSubject, Fleet, GameState, HintScreen, HistoryKind, HistoryQuery,
    IntelValue, Jurisdiction, JurisdictionKind, LawStatus, Permission, ResourceKind, Resources,
    RoleKind, Scenario, ShipKind, SpyReport, StoryEvent, TechBranch, TechNodeState, WonderError,
    advise, encode_galaxy_seed, polish_all_named, rephrase_lines,
};
use ratatui::Frame;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};
use std::cell::Cell;

mod views;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Overview,
    Planet,
    Shipyard,
    Wonder,
    Prestige,
    Achievements,
    Daily,
    Goals,
    Production,
    Maintenance,
    Market,
    Diplomacy,
    Laws,
    Reports,
    History,
    Staff,
    Ops,
    Automation,
    Finance,
    Kpis,
    Incidents,
    Map,
    Cockpit,
    Scenarios,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HistoryPane {
    List,
    Lineage,
    AsOf,
}

/// Shortest jump a running game treats as a wake, so a low launch threshold
/// cannot open the away summary on ordinary frames.
pub const MIN_WAKE_SECS: u64 = 60;
const AWAY_EVENTS_PER_PAGE: usize = 4;
const MIN_TRADE_AMOUNT: f64 = 10.0;
const MAX_TRADE_AMOUNT: f64 = 1_000_000.0;
/// Largest shipyard batch one Enter queues as a single job.
const MAX_SHIP_BATCH: u32 = 999;

/// `+`/`-` step on the market amount: one digit below its size, at least 10,
/// so 10 → 100 takes nine presses and 100 → 1000 nine more.
fn trade_step(amount: f64) -> f64 {
    10f64
        .powf(amount.max(MIN_TRADE_AMOUNT).log10().floor())
        .max(MIN_TRADE_AMOUNT)
}
const MAP_SYSTEM_MAX: u16 = 81;
const MAP_SLOT_COUNT: u16 = 16;

#[derive(Debug, Clone, Copy, PartialEq)]
struct DispatchForm {
    dest: Coord,
    ships: Fleet,
    selected: usize,
    mission: Mission,
}

/// What a star map dispatch does at its target: the same core actions as
/// `hoshi fleet send`. Deliveries to own colonies are planned in Fleet › Ops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mission {
    Attack,
    Spy,
    Invade,
    Colonize,
    Expedition,
}

impl Mission {
    fn label(self) -> &'static str {
        match self {
            Self::Attack => "attack",
            Self::Spy => "spy",
            Self::Invade => "invade",
            Self::Colonize => "colonize",
            Self::Expedition => "expedition",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct ReplayView {
    log: CombatLog,
    frame: usize,
    paused: bool,
    commentary: Vec<Option<String>>,
}

const TRADE_PAIRS: [(ResourceKind, ResourceKind); 6] = [
    (ResourceKind::Iron, ResourceKind::Silicon),
    (ResourceKind::Iron, ResourceKind::Deuterium),
    (ResourceKind::Silicon, ResourceKind::Iron),
    (ResourceKind::Silicon, ResourceKind::Deuterium),
    (ResourceKind::Deuterium, ResourceKind::Iron),
    (ResourceKind::Deuterium, ResourceKind::Silicon),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    SupportReport,
    ReportBug,
    None,
    Quit,
}

pub struct App {
    pub updates: Option<crate::update_ui::UpdateUi>,
    dismantle_confirm: Option<(Coord, BuildingKind)>,
    mission_form: Option<mission_form::MissionForm>,
    pub support: Option<crate::support_ui::SupportUi>,
    cheat_prompt: Option<String>,
    bug: Option<crate::bug_ui::BugUi>,
    pub state: GameState,
    pub portraits: PortraitView,
    keymap: Keymap,
    help_open: bool,
    help_commands: bool,
    screen: Screen,
    selected: usize,
    planet_index: usize,
    status: String,
    confirming_prestige: bool,
    trade_amount: f64,
    /// Units the shipyard queues per Enter, as one job in one queue slot.
    ship_batch: u32,
    away: Option<AwaySummary>,
    away_hidden: bool,
    /// Page of away-dialog events shown, newest page first.
    away_event_page: usize,
    /// A running game shows the away summary when one catch-up advances game
    /// time by at least this many seconds (wake from sleep). `None` disables.
    pub away_on_wake: Option<u64>,
    /// A wake summary whose desktop notice has not gone out yet.
    away_notice_due: bool,
    opened_report: bool,
    map_system: u16,
    map_position: u16,
    map_galaxy: u16,
    dispatch: Option<DispatchForm>,
    tree_branch: TechBranch,
    replay: Option<ReplayView>,
    pub reduced_motion: bool,
    pub no_animation: bool,
    pub show_tutorial: bool,
    pub commentary: bool,
    pub commentary_model_url: String,
    pub ai_advisor: bool,
    pub advisor_model_url: String,
    pub advisor_model_name: String,
    pub advisor_allow_lan: bool,
    pub governance: bool,
    pending_invasion: Option<DispatchForm>,
    notify: NotifyPrefs,
    watch: NotifyWatch,
    bridge_ui: bool,
    activity_open: bool,
    advisor_open: bool,
    advisor_lines: Vec<String>,
    activity_scroll: usize,
    bridge_log: Vec<BridgeLine>,
    prod_history: Vec<f64>,
    history_pane: HistoryPane,
    history_kind: Option<HistoryKind>,
    history_actor: Option<String>,
    cockpit: crate::cockpit::Cockpit,
    gauge_history: Vec<Resources>,
    pub high_contrast: bool,
    theme: Theme,
    detail_panel: bool,
    detail_overlay: bool,
    detail_dirty: bool,
    last_width: Cell<u16>,
    scenario: Option<Scenario>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BridgeLine {
    source: String,
    effect: String,
    alert: bool,
}

impl App {
    #[cfg(test)]
    pub fn new(state: GameState) -> Self {
        let mut app = Self::with_keymap(state, Keymap::defaults());
        app.screen = Screen::Overview;
        app
    }

    pub fn with_keymap(state: GameState, keymap: Keymap) -> Self {
        let watch = NotifyWatch::from_state(&state);
        Self {
            support: None,
            cheat_prompt: None,
            bug: None,
            state,
            portraits: PortraitView::default(),
            keymap,
            updates: None,
            dismantle_confirm: None,
            mission_form: None,
            help_open: false,
            help_commands: false,
            screen: Screen::Cockpit,
            selected: 0,
            planet_index: 0,
            status: String::new(),
            confirming_prestige: false,
            trade_amount: MIN_TRADE_AMOUNT,
            ship_batch: 1,
            away: None,
            away_hidden: false,
            away_event_page: 0,
            away_on_wake: None,
            away_notice_due: false,
            opened_report: false,
            map_system: Coord::HOME.system,
            map_position: Coord::HOME.position,
            map_galaxy: Coord::HOME.galaxy,
            dispatch: None,
            tree_branch: TechBranch::Industry,
            replay: None,
            reduced_motion: false,
            no_animation: false,
            show_tutorial: true,
            commentary: false,
            commentary_model_url: String::new(),
            ai_advisor: false,
            advisor_model_url: String::new(),
            advisor_model_name: String::new(),
            advisor_allow_lan: false,
            governance: false,
            pending_invasion: None,
            notify: NotifyPrefs::default(),
            watch,
            bridge_ui: false,
            activity_open: false,
            advisor_open: false,
            advisor_lines: Vec::new(),
            activity_scroll: 0,
            bridge_log: Vec::new(),
            prod_history: Vec::new(),
            history_pane: HistoryPane::List,
            history_kind: None,
            history_actor: None,
            cockpit: crate::cockpit::Cockpit::default(),
            gauge_history: Vec::new(),
            high_contrast: false,
            theme: Theme::default(),
            detail_panel: true,
            detail_overlay: false,
            detail_dirty: false,
            last_width: Cell::new(80),
            scenario: None,
        }
    }

    pub fn take_detail_panel(&mut self) -> Option<bool> {
        if self.detail_dirty {
            self.detail_dirty = false;
            Some(self.detail_panel)
        } else {
            None
        }
    }

    fn toggle_detail(&mut self) {
        if self.last_width.get() >= DOCK_MIN_WIDTH {
            self.detail_panel = !self.detail_panel;
            self.detail_dirty = true;
            self.detail_overlay = false;
        } else {
            self.detail_overlay = !self.detail_overlay;
        }
    }

    fn shows_docked_detail(&self) -> bool {
        self.detail_panel
            && self.last_width.get() >= DOCK_MIN_WIDTH
            && self.current_detail().is_some()
    }

    fn docks_dispatch(&self) -> bool {
        self.dispatch.is_some()
            && self.screen == Screen::Map
            && self.last_width.get() >= DOCK_MIN_WIDTH
    }

    fn shows_detail_overlay(&self) -> bool {
        self.detail_overlay
            && self.last_width.get() < DOCK_MIN_WIDTH
            && self.current_detail().is_some()
    }

    fn current_detail(&self) -> Option<Detail> {
        let subject = self.detail_subject()?;
        self.state.describe(subject, Some(self.map_cursor()))
    }

    fn detail_subject(&self) -> Option<DetailSubject> {
        match self.screen {
            Screen::Planet => {
                BuildingKind::ALL
                    .get(self.selected)
                    .copied()
                    .map(|kind| DetailSubject::Building {
                        kind,
                        planet: self.selected_coord(),
                    })
            }
            Screen::Overview => self.tech_id_at(self.selected).map(DetailSubject::Tech),
            Screen::Shipyard => {
                if self.selected < ShipKind::ALL.len() {
                    ShipKind::ALL
                        .get(self.selected)
                        .copied()
                        .map(DetailSubject::Ship)
                } else {
                    DefenceKind::ALL
                        .get(self.selected - ShipKind::ALL.len())
                        .copied()
                        .map(DetailSubject::Defence)
                }
            }
            Screen::Wonder => self
                .state
                .config
                .wonders
                .get(self.selected)
                .map(|spec| DetailSubject::Wonder(spec.id.clone())),
            Screen::Laws => {
                let templates: Vec<&str> = self
                    .state
                    .config
                    .laws
                    .iter()
                    .map(|spec| spec.id.as_str())
                    .collect();
                if self.selected < templates.len() {
                    Some(DetailSubject::Law(templates[self.selected].to_string()))
                } else {
                    self.state
                        .laws
                        .laws
                        .get(self.selected - templates.len())
                        .map(|law| DetailSubject::Law(law.template_id.clone()))
                }
            }
            Screen::Market => {
                TRADE_PAIRS
                    .get(self.selected)
                    .map(|&(pay, recv)| DetailSubject::Trade {
                        pay,
                        recv,
                        amount: self.trade_amount as u64,
                    })
            }
            Screen::Production => {
                let steps: Vec<_> = self
                    .state
                    .production_report()
                    .steps
                    .into_iter()
                    .filter(|step| step.planet == self.selected_coord())
                    .collect();
                steps
                    .get(self.selected)
                    .map(|step| DetailSubject::Production {
                        planet: step.planet,
                        id: step.id.clone(),
                    })
            }
            Screen::Diplomacy => self
                .state
                .config
                .diplomacy
                .iter()
                .nth(self.selected)
                .map(|spec| DetailSubject::Faction(spec.id.clone())),
            Screen::Map => Some(DetailSubject::Coord(self.map_cursor())),
            _ => None,
        }
    }

    pub fn take_cockpit_layout(&mut self) -> Option<String> {
        if self.cockpit.take_layout_dirty() {
            Some(self.cockpit.layout.as_key().into())
        } else {
            None
        }
    }

    pub fn apply_cockpit(&mut self, user: &crate::config::UserConfig) {
        self.high_contrast = user.high_contrast;
        self.theme = Theme::resolve(
            Depth::detect(|key| std::env::var(key).ok()),
            user.high_contrast,
        );
        self.detail_panel = user.detail_panel;
        if let Some(layout) = crate::cockpit::CockpitLayout::from_key(&user.cockpit_layout) {
            self.cockpit.layout = layout;
        }
        self.cockpit.presets = crate::cockpit::Presets::parse(
            &user.cockpit_overview,
            &user.cockpit_economy,
            &user.cockpit_war,
        );
    }

    #[cfg(any(test, feature = "bridge"))]
    pub fn set_bridge_ui(&mut self, enabled: bool) {
        self.bridge_ui = enabled;
        if !enabled {
            self.activity_open = false;
        }
    }

    #[cfg(any(test, feature = "bridge"))]
    pub fn record_bridge(&mut self, source: &str, effect: &str, alert: bool) {
        self.bridge_log.push(BridgeLine {
            source: source.to_string(),
            effect: effect.to_string(),
            alert,
        });
        const CAP: usize = 100;
        if self.bridge_log.len() > CAP {
            let drop = self.bridge_log.len() - CAP;
            self.bridge_log.drain(..drop);
        }
        self.activity_scroll = 0;
    }

    pub fn replay_active(&self) -> bool {
        self.replay.is_some()
    }

    pub fn tick_replay(&mut self) {
        let still = self.reduced_motion || self.no_animation;
        let Some(replay) = self.replay.as_mut() else {
            return;
        };
        if replay.paused || still {
            return;
        }
        if replay.frame + 1 < replay.log.report.frames.len() {
            replay.frame += 1;
        }
    }

    fn open_latest_replay(&mut self) {
        match self.state.combat_logs.len().checked_sub(1) {
            Some(index) => self.open_replay(index),
            None => self.status = "no combat log".into(),
        }
    }

    fn open_replay(&mut self, index: usize) {
        let Some(log) = self.state.combat_logs.get(index).cloned() else {
            return;
        };
        let last = log.report.frames.len().saturating_sub(1);
        let still = self.reduced_motion || self.no_animation;
        let commentary = if self.commentary {
            let mut lines = CommentaryCatalog::bundled().lines_for(&log.report);
            if !self.commentary_model_url.is_empty() {
                rephrase_lines(&mut lines, &self.commentary_model_url);
            }
            lines
        } else {
            Vec::new()
        };
        self.replay = Some(ReplayView {
            frame: if still { last } else { 0 },
            paused: still,
            log,
            commentary,
        });
        self.away_hidden = true;
    }

    pub fn set_away(&mut self, summary: AwaySummary) {
        self.away = Some(summary);
        self.away_hidden = false;
        self.away_event_page = 0;
    }

    pub fn set_notify(&mut self, prefs: NotifyPrefs) {
        self.notify = prefs;
        self.watch = NotifyWatch::from_state(&self.state);
    }

    pub fn poll_notify(&mut self) -> Vec<u8> {
        let now = self.state.last_tick_unix;
        let mut notices = self.watch.observe(&self.state, now);
        if std::mem::take(&mut self.away_notice_due)
            && let Some(away) = &self.away
        {
            notices.extend(notify::summarize_away(away, self.notify));
        }
        notify::encode(self.notify, now, &notices)
    }

    pub fn notify_away(&self, summary: &AwaySummary) -> Vec<u8> {
        let Some(notice) = notify::summarize_away(summary, self.notify) else {
            return Vec::new();
        };
        notify::encode(self.notify, self.state.last_tick_unix, &[notice])
    }

    pub fn catch_up_now(&mut self, now_unix: u64) {
        self.catch_up_to(self.state.effective_time(now_unix));
        self.record_production_sample();
        crate::cockpit::record_gauge(&mut self.gauge_history, self.state.resources);
    }

    /// Catch up to the effective game time `target`. A gap of at least
    /// `away_on_wake` opens the away summary; returns true when it did.
    fn catch_up_to(&mut self, target: u64) -> bool {
        self.state.interactive_tactics = true;
        let gap = target.saturating_sub(self.state.last_tick_unix);
        if !self.away_on_wake.is_some_and(|secs| gap >= secs) {
            self.state.catch_up(target);
            return false;
        }
        let before = crate::title::snapshot_away(&self.state);
        let ledger = self.state.catch_up_ledger(target);
        self.set_away(crate::title::summarize_away(&before, ledger, &self.state));
        self.away_notice_due = true;
        true
    }

    pub fn open_support(&mut self, save: &std::path::Path) {
        match crate::support_ui::SupportUi::open(save) {
            Ok(report) => self.support = Some(report),
            Err(error) => self.status = error,
        }
    }

    pub fn open_bug(&mut self, save: &std::path::Path) {
        self.bug = Some(crate::bug_ui::BugUi::open(
            save,
            save.parent().unwrap_or(std::path::Path::new(".")),
            false,
        ));
    }

    pub fn handle_key(&mut self, code: KeyCode, now_unix: u64) -> Action {
        if let Some(bug) = &mut self.bug {
            if bug.handle_key(code) {
                self.bug = None;
            }
            return Action::None;
        }
        let now_unix = self.state.effective_time(now_unix);
        if let Some(prompt) = &mut self.cheat_prompt {
            match code {
                KeyCode::Esc => self.cheat_prompt = None,
                KeyCode::Enter => {
                    let code = self.cheat_prompt.take().unwrap_or_default();
                    let intent = hoshi_core::Intent::Cheat {
                        code,
                        actor: Actor::Player,
                    };
                    self.status = match hoshi_core::propose(&self.state, &intent, now_unix)
                        .and_then(|proposal| {
                            hoshi_core::apply(&mut self.state, &proposal, now_unix)
                        }) {
                        Ok(()) => "cheat applied; save permanently modified".into(),
                        Err(error) => error.to_string(),
                    };
                }
                KeyCode::Backspace => {
                    prompt.pop();
                }
                KeyCode::Char(ch) if prompt.len() < 256 => prompt.push(ch),
                _ => {}
            }
            return Action::None;
        }

        if let Some(report) = &mut self.support {
            if report.handle_key(code) {
                self.support = None;
            }
            return Action::None;
        }
        if let Some(updates) = &mut self.updates {
            if self.help_open && code == KeyCode::Char('u') {
                updates.dismiss_available();
                return Action::None;
            }
            if self.help_open && code == KeyCode::Char('v') {
                updates.open_changelog();
                return Action::None;
            }
            if updates.handle_key(code) {
                return Action::None;
            }
        }
        if self.dismantle_confirm.is_some() {
            return self.handle_dismantle_confirm(code, now_unix);
        }
        if self.mission_form.is_some() {
            return self.handle_mission_form(code, now_unix);
        }

        if self.help_open && code == KeyCode::Char('b') {
            return Action::ReportBug;
        }
        if self.help_open && code == KeyCode::Char('p') {
            return Action::SupportReport;
        }

        if self.screen == Screen::Overview && self.keymap.matches(Binding::CheatConsole, code) {
            self.open_cheat_console();
            return Action::None;
        }
        if self.catch_up_to(now_unix) {
            // The key that woke the game must not dismiss the summary it opened.
            return Action::None;
        }
        self.clamp_planet();
        if self.help_open {
            if matches!(code, KeyCode::Tab | KeyCode::BackTab) {
                self.help_commands = !self.help_commands;
                return Action::None;
            }
            if self.keymap.matches(Binding::Quit, code) {
                return Action::Quit;
            }
            if self.keymap.matches(Binding::Help, code)
                || self.keymap.matches(Binding::Cancel, code)
                || self.keymap.matches(Binding::Confirm, code)
            {
                self.help_open = false;
            }
            return Action::None;
        }
        if self.advisor_open {
            if self.keymap.matches(Binding::Quit, code) {
                return Action::Quit;
            }
            if self.keymap.matches(Binding::Advisor, code)
                || self.keymap.matches(Binding::Cancel, code)
                || self.keymap.matches(Binding::Confirm, code)
            {
                self.advisor_open = false;
                return Action::None;
            }
            return Action::None;
        }
        if self.activity_open {
            if self.keymap.matches(Binding::Quit, code) {
                return Action::Quit;
            }
            if self.keymap.matches(Binding::Activity, code)
                || self.keymap.matches(Binding::Cancel, code)
                || self.keymap.matches(Binding::Confirm, code)
            {
                self.activity_open = false;
                return Action::None;
            }
            if self.keymap.matches(Binding::SelectNext, code) {
                let max = self.bridge_log.len().saturating_sub(1);
                self.activity_scroll = (self.activity_scroll + 1).min(max);
            }
            if self.keymap.matches(Binding::SelectPrev, code) {
                self.activity_scroll = self.activity_scroll.saturating_sub(1);
            }
            return Action::None;
        }
        let ctx = self.help_context();
        if self.keymap.lookup(ctx, code) == Some(Binding::Help) {
            self.help_open = true;
            self.help_commands = false;
            return Action::None;
        }
        if self.replay.is_some() {
            return self.handle_replay_key(code);
        }
        if self.away.is_some() && !self.away_hidden {
            if self.keymap.matches(Binding::Toggle, code) {
                let events = self.away.as_ref().map_or(0, |away| away.events.len());
                let pages = events.div_ceil(AWAY_EVENTS_PER_PAGE).max(1);
                self.away_event_page = (self.away_event_page + 1) % pages;
                return Action::None;
            }
            match self.keymap.lookup(ctx, code) {
                Some(Binding::Confirm | Binding::Cancel) => self.away_hidden = true,
                Some(Binding::Replay) => self.open_latest_replay(),
                Some(Binding::Quit) => return Action::Quit,
                _ => {}
            }
            return Action::None;
        }
        if self.state.pending_tactical.is_some() {
            return self.handle_tactical_key(code, now_unix);
        }
        if self.shows_victory_overlay() {
            match self.keymap.lookup(ctx, code) {
                Some(Binding::Confirm) => {
                    let _ = self.state.keep_playing_after_victory();
                }
                Some(Binding::Quit) => return Action::Quit,
                _ => {}
            }
            return Action::None;
        }
        if self.story_event().is_some() {
            match self.keymap.lookup(ctx, code) {
                Some(Binding::Quit) => return Action::Quit,
                _ => {
                    if let KeyCode::Char(c) = code
                        && c.is_ascii_digit()
                        && c != '0'
                    {
                        let idx = (c as u8 - b'1') as usize;
                        self.choose_story_at(idx, now_unix);
                    }
                }
            }
            return Action::None;
        }
        if self.opened_report {
            match self.keymap.lookup(ctx, code) {
                Some(Binding::Confirm | Binding::Cancel) => self.opened_report = false,
                Some(Binding::Quit) => return Action::Quit,
                _ => {}
            }
            return Action::None;
        }
        if self.pending_invasion.is_some() {
            match self.keymap.lookup(ctx, code) {
                Some(Binding::Confirm) => self.resolve_invasion(true, now_unix),
                Some(Binding::Cancel) => self.resolve_invasion(false, now_unix),
                Some(Binding::Quit) => return Action::Quit,
                _ => {}
            }
            return Action::None;
        }
        if self.dispatch.is_none()
            && self.show_tutorial
            && let Some(hint) = self.state.pending_hint()
        {
            if matches!(code, KeyCode::Char('g')) {
                self.screen = match hint.screen {
                    HintScreen::Overview => Screen::Overview,
                    HintScreen::Planet => Screen::Planet,
                    HintScreen::Shipyard => Screen::Shipyard,
                    HintScreen::Map => Screen::Map,
                };
                self.state.dismiss_hint(&hint.id);
                return Action::None;
            }
            if matches!(code, KeyCode::Char('s')) {
                self.state.skip_tutorial();
                return Action::None;
            }
            if self.keymap.matches(Binding::Cancel, code) {
                self.state.dismiss_hint(&hint.id);
                return Action::None;
            }
        }
        if self.bridge_ui && self.keymap.matches(Binding::Activity, code) {
            self.activity_open = true;
            return Action::None;
        }
        if self.ai_advisor && self.keymap.matches(Binding::Advisor, code) {
            self.open_advisor();
            return Action::None;
        }
        if self.dispatch.is_none()
            && self.pending_invasion.is_none()
            && self.keymap.lookup(ctx, code).is_none()
        {
            if let KeyCode::Char(c) = code
                && let Some(hub) = Hub::from_digit(c)
            {
                self.open(hubs::enter(self.screen, hub));
                return Action::None;
            }
            if code == KeyCode::BackTab {
                self.cycle_screen(false);
                return Action::None;
            }
        }
        if self.dispatch.is_none() && self.keymap.matches(Binding::Cockpit, code) {
            self.open(Screen::Cockpit);
            return Action::None;
        }
        if self.screen == Screen::Cockpit && self.dispatch.is_none() {
            return self.handle_cockpit_key(code);
        }
        match self.keymap.lookup(ctx, code) {
            Some(Binding::Quit) => return Action::Quit,
            Some(Binding::Replay) if self.screen == Screen::History => {
                self.toggle_history_as_of();
            }
            Some(Binding::Replay) if self.screen == Screen::Overview => {
                self.open_latest_replay();
            }
            Some(Binding::ShowAway) if self.screen == Screen::Overview && self.away.is_some() => {
                self.away_hidden = false;
            }
            Some(Binding::NextScreen) => self.cycle_screen(true),
            Some(Binding::NextPlanet) => self.cycle_planet(1),
            Some(Binding::PrevPlanet) => self.cycle_planet(-1),
            Some(Binding::SelectNext) if self.dispatch.is_some() => self.nudge_dispatch(1),
            Some(Binding::SelectPrev) if self.dispatch.is_some() => self.nudge_dispatch(-1),
            Some(Binding::CursorLeft) if self.screen == Screen::Map && self.dispatch.is_none() => {
                self.nudge_map_position(-1);
            }
            Some(Binding::CursorRight) if self.screen == Screen::Map && self.dispatch.is_none() => {
                self.nudge_map_position(1);
            }
            Some(Binding::CursorLeft) if self.screen == Screen::Overview => {
                self.nudge_tree_branch(-1);
            }
            Some(Binding::CursorRight) if self.screen == Screen::Overview => {
                self.nudge_tree_branch(1);
            }
            Some(Binding::SelectNext) if self.screen == Screen::Map && self.dispatch.is_none() => {
                self.nudge_map_system(1);
            }
            Some(Binding::SelectPrev) if self.screen == Screen::Map && self.dispatch.is_none() => {
                self.nudge_map_system(-1);
            }
            Some(Binding::SelectNext) => {
                let len = self.list_len();
                if len > 0 {
                    self.selected = (self.selected + 1) % len;
                }
            }
            Some(Binding::SelectPrev) => {
                let len = self.list_len();
                if len > 0 {
                    self.selected = (self.selected + len - 1) % len;
                }
            }
            Some(Binding::Increase) if self.dispatch.is_some() => self.nudge_dispatch_ships(1),
            Some(Binding::Decrease) if self.dispatch.is_some() => self.nudge_dispatch_ships(-1),
            Some(Binding::Increase) if self.screen == Screen::History => {
                self.cycle_history_kind();
            }
            Some(Binding::Decrease) if self.screen == Screen::History => {
                self.cycle_history_actor();
            }
            Some(Binding::Increase) if self.screen == Screen::Staff => {
                self.recruit_selected_leader(now_unix);
            }
            Some(Binding::Increase) if self.screen == Screen::Ops => {
                self.register_ops_vehicle(now_unix);
            }
            Some(Binding::Increase) if self.screen == Screen::Shipyard => {
                self.ship_batch = (self.ship_batch + 1).min(MAX_SHIP_BATCH);
            }
            Some(Binding::Decrease) if self.screen == Screen::Shipyard => {
                self.ship_batch = self.ship_batch.saturating_sub(1).max(1);
            }
            Some(Binding::Toggle) if self.screen == Screen::Shipyard => {
                self.ship_batch = self.max_ship_batch();
            }
            Some(Binding::Increase) if self.screen == Screen::Market => {
                self.trade_amount =
                    (self.trade_amount + trade_step(self.trade_amount)).min(MAX_TRADE_AMOUNT);
            }
            Some(Binding::Toggle) if self.screen == Screen::Market => {
                if let Some(&(pay, _)) = TRADE_PAIRS.get(self.selected) {
                    let max = self.state.max_trade_amount(pay).floor();
                    self.trade_amount = max.clamp(MIN_TRADE_AMOUNT, MAX_TRADE_AMOUNT);
                }
            }
            Some(Binding::Increase) if self.screen == Screen::Automation => {
                self.add_default_automation_rule();
            }
            Some(Binding::Increase) if self.screen == Screen::Scenarios => {
                self.scenario_fork_or_queue();
            }
            Some(Binding::Toggle) if self.screen == Screen::Automation => {
                self.toggle_selected_automation_rule();
            }
            Some(Binding::Toggle) if self.screen == Screen::Scenarios => {
                self.scenario_advance();
            }
            Some(Binding::Decrease) if self.screen == Screen::Market => {
                self.trade_amount =
                    (self.trade_amount - trade_step(self.trade_amount - 1.0)).max(MIN_TRADE_AMOUNT);
            }
            Some(Binding::SendProbe) if self.screen == Screen::Reports && !self.opened_report => {
                self.send_probe(now_unix);
            }
            Some(Binding::Dismantle) if self.screen == Screen::Planet => {
                if let Some(kind) = BuildingKind::ALL.get(self.selected).copied() {
                    self.dismantle_confirm = Some((self.selected_coord(), kind));
                }
            }
            Some(Binding::Detail) => self.toggle_detail(),
            Some(Binding::Confirm) => self.activate(now_unix),
            Some(Binding::Cancel) if self.detail_overlay => {
                self.detail_overlay = false;
            }
            Some(Binding::Cancel) if code == KeyCode::Esc && self.esc_closes_window() => {
                self.open(Screen::Cockpit);
            }
            Some(Binding::Cancel) => self.cancel(now_unix),
            Some(Binding::CopySeed) => {
                let seed = encode_galaxy_seed(self.state.galaxy_seed);
                copy_to_clipboard(&seed);
                self.status = format!("copied {seed}");
            }
            Some(Binding::Toggle) if self.dispatch.is_some() => self.cycle_dispatch_mission(),
            Some(Binding::Toggle) if self.screen == Screen::Map && self.dispatch.is_none() => {
                self.cycle_map_galaxy();
            }
            _ if self.keymap.matches(Binding::CheatConsole, code) => self.open_cheat_console(),
            _ => {}
        }
        Action::None
    }

    fn handle_cockpit_key(&mut self, code: KeyCode) -> Action {
        let ctx = HelpContext::Cockpit;
        let map = self.cockpit.focused_panel() == Some(crate::cockpit::Panel::Map);
        let len = crate::cockpit::select_len(&self.state, &self.cockpit);
        match self.keymap.lookup(ctx, code) {
            Some(Binding::Quit) => return Action::Quit,
            Some(Binding::NextScreen) => self.cycle_screen(true),
            Some(Binding::Toggle) => self.cockpit.cycle_layout(),
            // The bridge alert describes the latest raid, so `r` opens that one.
            Some(Binding::Replay) => match self
                .state
                .combat_logs
                .iter()
                .rposition(|log| log.raid_plunder.is_some())
            {
                Some(index) => self.open_replay(index),
                None => self.open_latest_replay(),
            },
            Some(Binding::CursorLeft) if map => self.nudge_map_position(-1),
            Some(Binding::CursorRight) if map => self.nudge_map_position(1),
            Some(Binding::SelectNext) if map => self.nudge_map_system(1),
            Some(Binding::SelectPrev) if map => self.nudge_map_system(-1),
            Some(Binding::CursorLeft) => self.cockpit.nudge_focus(-1),
            Some(Binding::CursorRight) => self.cockpit.nudge_focus(1),
            Some(Binding::SelectNext) => self.cockpit.nudge_select(1, len),
            Some(Binding::SelectPrev) => self.cockpit.nudge_select(-1, len),
            Some(Binding::Confirm) => self.cockpit_confirm(),
            Some(Binding::Cancel) => {
                let _ = self.cockpit.back();
            }
            Some(Binding::CopySeed) => {
                let seed = encode_galaxy_seed(self.state.galaxy_seed);
                copy_to_clipboard(&seed);
                self.status = format!("copied {seed}");
            }
            _ if self.keymap.matches(Binding::CheatConsole, code) => self.open_cheat_console(),
            _ => {}
        }
        Action::None
    }

    fn cockpit_confirm(&mut self) {
        let alerts = crate::cockpit::alerts(&self.state);
        let fleets = crate::cockpit::fleet_targets(&self.state);
        let planets = crate::cockpit::planet_targets(&self.state);
        self.cockpit.confirm(
            &alerts,
            &fleets,
            &planets,
            Coord {
                galaxy: self.state.home.galaxy,
                system: self.map_system,
                position: self.map_position,
            },
        );
        if let Some(crate::cockpit::Target::Planet(coord)) = self.cockpit.top() {
            self.map_system = coord.system;
            self.map_position = coord.position;
        }
    }

    fn cycle_screen(&mut self, forward: bool) {
        self.open(hubs::step(self.screen, forward));
    }

    fn esc_closes_window(&self) -> bool {
        self.screen != Screen::Cockpit
            && self.dispatch.is_none()
            && self.pending_invasion.is_none()
            && !self.confirming_prestige
            && self.history_pane == HistoryPane::List
    }

    pub(crate) fn open(&mut self, screen: Screen) {
        self.screen = screen;
        self.selected = 0;
        self.status.clear();
        self.confirming_prestige = false;
        self.opened_report = false;
        self.dispatch = None;
        self.pending_invasion = None;
        self.history_pane = HistoryPane::List;
    }

    /// Open the console, or say cheats are off. Overview binds the key
    /// directly; elsewhere it is the last resort, after every other handler
    /// (context bindings, hub digits, tutorial keys) passed on the key, so a
    /// remapped console key never shadows one.
    fn open_cheat_console(&mut self) {
        if self.state.cheats_enabled() {
            self.cheat_prompt = Some(String::new());
        } else {
            self.status = "cheats are disabled".into();
        }
    }

    fn help_context(&self) -> HelpContext {
        if self.replay.is_some() {
            return HelpContext::Replay;
        }
        if self.away.is_some() && !self.away_hidden {
            return HelpContext::Away;
        }
        if self.state.pending_tactical.is_some() {
            return HelpContext::Tactical;
        }
        if self.shows_victory_overlay() {
            return HelpContext::Victory;
        }
        if self.story_event().is_some() {
            return HelpContext::Story;
        }
        if self.opened_report {
            return HelpContext::ReportView;
        }
        if self.pending_invasion.is_some() || self.dispatch.is_some() {
            return HelpContext::Dispatch;
        }
        match self.screen {
            Screen::Overview => HelpContext::Overview,
            Screen::Planet => HelpContext::Planet,
            Screen::Shipyard => HelpContext::Shipyard,
            Screen::Wonder => HelpContext::Wonder,
            Screen::Prestige => HelpContext::Prestige,
            Screen::Achievements => HelpContext::Achievements,
            Screen::Daily => HelpContext::Daily,
            Screen::Goals => HelpContext::Goals,
            Screen::Production => HelpContext::Production,
            Screen::Maintenance => HelpContext::Maintenance,
            Screen::Market => HelpContext::Market,
            Screen::Diplomacy => HelpContext::Diplomacy,
            Screen::Laws => HelpContext::Laws,
            Screen::Reports => HelpContext::Reports,
            Screen::History => HelpContext::History,
            Screen::Staff => HelpContext::Staff,
            Screen::Ops => HelpContext::Ops,
            Screen::Automation => HelpContext::Automation,
            Screen::Finance => HelpContext::Finance,
            Screen::Kpis => HelpContext::Kpis,
            Screen::Incidents => HelpContext::Incidents,
            Screen::Map => HelpContext::Map,
            Screen::Cockpit => HelpContext::Cockpit,
            Screen::Scenarios => HelpContext::Scenarios,
        }
    }

    fn cycle_planet(&mut self, delta: i32) {
        let n = self.state.owned_coords().len() as i32;
        if n <= 0 {
            return;
        }
        let next = (self.planet_index as i32 + delta).rem_euclid(n);
        self.planet_index = next as usize;
        self.status.clear();
    }

    fn clamp_planet(&mut self) {
        let n = self.state.owned_coords().len().max(1);
        if self.planet_index >= n {
            self.planet_index = n - 1;
        }
    }

    fn selected_coord(&self) -> Coord {
        let coords = self.state.owned_coords();
        let i = self.planet_index.min(coords.len().saturating_sub(1));
        coords[i]
    }

    pub fn draw(&self, frame: &mut Frame<'_>) {
        let area = frame.area();
        self.last_width.set(area.width);
        let t = &self.theme;
        ui::chrome::fill(frame.buffer_mut(), area, t.surface(Surface::Base));
        let [nav, metrics, rule, body, status] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(MetricStrip::HEIGHT),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .areas(area);
        let hub = Hub::of(self.screen);
        frame.render_widget(
            Navbar {
                theme: t,
                mark: "HOSHI",
                crumbs: vec![hub.title().into(), hubs::label(self.screen).into()],
                right: self.navbar_right(),
            },
            nav,
        );
        frame.render_widget(
            MetricStrip {
                theme: t,
                metrics: self.metrics(area.width),
            },
            metrics,
        );
        ui::chrome::put(
            frame.buffer_mut(),
            rule,
            &Line::styled("─".repeat(rule.width as usize), t.line()),
        );
        let mut crumbs = Vec::new();
        if area.width >= 100 {
            crumbs.push("Bridge".to_string());
            if hub != Hub::Bridge {
                crumbs.push(hub.title().into());
            }
            crumbs.push(hubs::label(self.screen).into());
        }
        frame.render_widget(
            StatusBar {
                theme: t,
                crumbs,
                hints: self.status_hints(area.width),
                right: Some(self.seed_line(area.width)),
            },
            status,
        );
        if self.screen == Screen::Cockpit {
            let footer = self.footer_lines(body.width as usize);
            self.cockpit_draw_inner(frame, body, footer);
        } else {
            let win = window_area(area, body);
            if win != body {
                self.cockpit_draw_inner(frame, body, Vec::new());
                ui::chrome::dim(frame.buffer_mut(), body, t);
            }
            self.draw_window(frame, win, hub);
        }
        self.draw_overlays(frame);
    }

    fn draw_window(&self, frame: &mut Frame<'_>, area: Rect, hub: Hub) {
        let t = &self.theme;
        let note = if !self.status.is_empty() {
            Some(Line::styled(self.status.clone(), t.intent(Intent::Warning)))
        } else if self.screen == Screen::Planet {
            Some(Line::styled(
                "Costs in red exceed current stock.",
                t.label(),
            ))
        } else {
            self.state
                .events
                .last()
                .map(|event| Line::styled(event.text.clone(), t.label()))
        };
        let dialog = Dialog {
            theme: t,
            title: hub.title().into(),
            subtitle: self.window_subtitle(hub),
            close_hint: "✕ esc".into(),
            footer_note: note,
            buttons: self.window_buttons(),
        };
        let inner = pad_x(dialog.inner(area), 1);
        frame.render_widget(dialog, area);
        let screens = hub.screens();
        let callouts = self.window_callouts(hub);
        let footer = self.footer_lines(inner.width as usize);
        let [tabs, callout_area, body, footer_area] = Layout::vertical([
            Constraint::Length(Tabs::HEIGHT + 1),
            Constraint::Length(callouts.len() as u16),
            Constraint::Min(1),
            Constraint::Length(footer.len() as u16),
        ])
        .areas(inner);
        frame.render_widget(
            Tabs {
                theme: t,
                items: screens
                    .iter()
                    .map(|s| hubs::label(*s).to_string())
                    .collect(),
                selected: screens.iter().position(|s| *s == self.screen).unwrap_or(0),
            },
            tabs,
        );
        frame.render_widget(Paragraph::new(callouts), callout_area);
        if self.shows_docked_detail() || self.docks_dispatch() {
            let [list, divider, card] = Layout::horizontal([
                Constraint::Min(24),
                Constraint::Length(3),
                Constraint::Length(detail_column_width(body.width)),
            ])
            .areas(body);
            self.draw_list(frame, list);
            for y in divider.top()..divider.bottom() {
                ui::chrome::put(
                    frame.buffer_mut(),
                    Rect::new(divider.x + 1, y, 1, 1),
                    &Line::styled("│", t.line()),
                );
            }
            if let Some(form) = self.dispatch.as_ref().filter(|_| self.docks_dispatch()) {
                frame.render_widget(Paragraph::new(self.dispatch_lines(form)), card);
            } else if let Some(detail) = self.current_detail() {
                let (kind, extra) = self.inspector_context();
                let levelled = matches!(
                    self.detail_subject(),
                    Some(
                        DetailSubject::Building { .. }
                            | DetailSubject::Tech(_)
                            | DetailSubject::Wonder(_)
                    )
                );
                frame.render_widget(
                    Paragraph::new(detail_panel::inspector(&detail, kind, levelled, extra, t))
                        .wrap(Wrap { trim: false }),
                    card,
                );
            }
        } else {
            let (list_area, portrait) = if self.screen == Screen::Staff {
                portraits::split_area(body)
            } else {
                (body, None)
            };
            self.draw_list(frame, list_area);
            if let Some(area) = portrait {
                let art = Layout::vertical([
                    Constraint::Length(portraits::ASCII_ROWS as u16),
                    Constraint::Min(0),
                ])
                .split(area);
                let slot = self.current_portrait_slot();
                let mood = portraits::mood_for(slot, self.state.visible_events(Actor::Player));
                frame.render_widget(
                    Paragraph::new(portraits::portrait_lines_for(
                        self.portraits,
                        slot,
                        mood,
                        self.state.species.as_ref().map(|run| run.portrait.as_str()),
                    )),
                    art[0],
                );
            }
        }
        frame.render_widget(Paragraph::new(footer).style(t.muted()), footer_area);
    }

    fn window_subtitle(&self, hub: Hub) -> String {
        let coord = self.selected_coord();
        let place = if coord == self.state.home {
            "Home"
        } else {
            "Colony"
        };
        match hub {
            Hub::Colony => {
                let kind = self
                    .state
                    .planet_kind(coord)
                    .map(|k| k.as_key())
                    .unwrap_or("unknown");
                let work = if self.screen == Screen::Overview {
                    let queued = match self.state.research_queue.len() {
                        0 => String::new(),
                        n => format!(" +{n} queued"),
                    };
                    match &self.state.research {
                        Some(job) => format!(
                            "research {} → lv{} {}{queued}",
                            job.tech,
                            job.from_level + 1,
                            ui::spans::duration(
                                job.completes_at.saturating_sub(self.state.last_tick_unix)
                            )
                        ),
                        None => "research idle".into(),
                    }
                } else {
                    let queue = self.state.queue_at(coord).map_or(0, <[_]>::len);
                    format!("queue {queue}/{}", self.state.build_queue_cap())
                };
                format!("{place} {coord} · {kind} · {work}")
            }
            Hub::Fleet => format!(
                "{place} {coord} · ship queue {}",
                self.state.ship_queue.len()
            ),
            _ => format!("{place} {coord}"),
        }
    }

    fn inspector_context(&self) -> (&'static str, Vec<(String, Line<'static>)>) {
        let t = &self.theme;
        match self.detail_subject() {
            Some(DetailSubject::Building { kind, planet }) => {
                let mut extra = vec![(
                    "Planet".to_string(),
                    Line::styled(planet.to_string(), t.intent(Intent::Primary)),
                )];
                extra.extend(
                    building_deposits(&self.state, kind, planet)
                        .into_iter()
                        .map(|(id, richness)| {
                            (
                                "Deposit".into(),
                                Line::styled(
                                    format!(
                                        "{} {richness:.2}",
                                        ResourceKind::from_key(&id).map_or_else(
                                            || id.replace('_', " "),
                                            |r| r.label().into()
                                        )
                                    ),
                                    t.text(),
                                ),
                            )
                        }),
                );
                ("building", extra)
            }
            Some(DetailSubject::Tech(_)) => ("tech", Vec::new()),
            Some(DetailSubject::Ship(_)) => ("ship", Vec::new()),
            Some(DetailSubject::Defence(_)) => ("defence", Vec::new()),
            Some(DetailSubject::Wonder(_)) => ("wonder", Vec::new()),
            Some(DetailSubject::Law(_)) => ("law", Vec::new()),
            Some(DetailSubject::Trade { .. }) => ("trade", Vec::new()),
            Some(DetailSubject::Production { .. }) => ("production", Vec::new()),
            Some(DetailSubject::Faction(_)) => ("faction", Vec::new()),
            Some(DetailSubject::Coord(_)) | None => ("location", Vec::new()),
        }
    }

    fn navbar_right(&self) -> Vec<Span<'static>> {
        let t = &self.theme;
        let mut spans = Vec::new();
        // The next raid shows as soon as it is scheduled; inside the warning
        // window it turns into a ticking countdown.
        if let Some(secs) = self.state.next_raid_in_secs() {
            let (text, intent) = match self.state.raid_countdown_secs() {
                Some(_) => (ui::spans::countdown(secs), Intent::Danger),
                None => (ui::spans::duration(secs), Intent::Warning),
            };
            spans.push(ui::spans::tag(t, &format!("RAID {text}"), intent, false));
            spans.push(Span::raw("  "));
        }
        if self.state.is_modified() {
            spans.push(ui::spans::tag(t, "MODIFIED", Intent::Warning, false));
            spans.push(Span::raw("  "));
        }
        if let Some(run) = self.state.species.as_ref() {
            spans.push(Span::styled(run.name.clone(), t.muted()));
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(self.selected_coord().to_string(), t.text()));
        spans
    }

    fn metrics(&self, width: u16) -> Vec<Metric> {
        let t = &self.theme;
        let s = &self.state;
        let wide = width / 8 >= 14;
        let rate_span = |rate: f64| {
            let style = if rate < 0.0 {
                t.intent(Intent::Danger)
            } else if rate > 0.0 {
                t.intent(Intent::Success)
            } else {
                t.label()
            };
            Span::styled(
                format!(
                    "{}{}/h",
                    if rate > 0.0 { "+" } else { "" },
                    ui::spans::compact(rate)
                ),
                style,
            )
        };
        let mut out: Vec<Metric> = ResourceKind::ALL
            .iter()
            .map(|&kind| {
                let stock = s.resources.get(kind);
                let cap = s.storage_cap(kind);
                let rate = s.production_per_hour(kind);
                let fill = if cap > 0.0 { stock / cap } else { 0.0 };
                let intent = if fill >= 0.95 {
                    Intent::Warning
                } else {
                    Intent::Primary
                };
                let value_style = if fill >= 0.95 {
                    t.intent(Intent::Warning)
                } else {
                    t.strong()
                };
                if !wide {
                    return Metric {
                        label: kind.abbr().into(),
                        value: Line::styled(ui::spans::compact(stock), value_style),
                        foot: Line::from(rate_span(rate)),
                    };
                }
                let mut foot = ui::spans::bar(t, fill, 6, intent);
                foot.push(Span::raw(" "));
                foot.push(rate_span(rate));
                Metric {
                    label: kind.label().to_uppercase(),
                    value: Line::from(vec![
                        Span::styled(ui::spans::compact(stock), value_style),
                        Span::styled(format!(" / {}", ui::spans::compact(cap)), t.label()),
                    ]),
                    foot: Line::from(foot),
                }
            })
            .collect();
        let waste = s.waste_home();
        let balance = s.energy_balance();
        let energy_style = if balance < 0.0 {
            t.intent(Intent::Danger)
        } else {
            t.strong()
        };
        let heat_style = if waste.overflowing() {
            t.intent(Intent::Danger)
        } else {
            t.strong()
        };
        let heat_fill = if waste.heat_sink > 0.0 {
            waste.heat_load / waste.heat_sink
        } else {
            0.0
        };
        if wide {
            let energy = s.production_report();
            out.push(Metric {
                label: "ENERGY".into(),
                value: Line::from(vec![
                    Span::styled(format!("{balance:+.0}"), energy_style),
                    Span::styled(" home", t.label()),
                ]),
                // One story per state: how many planets (home included) run
                // short, or else how much of the supply is drawn. Compact
                // numbers keep every digit inside the cell.
                foot: match s
                    .owned_coords()
                    .into_iter()
                    .filter(|&coord| s.energy_balance_at(coord).is_some_and(|b| b < -1e-9))
                    .count()
                {
                    0 => Line::styled(
                        format!(
                            "{}/{} used",
                            ui::spans::compact(energy.energy_demand),
                            ui::spans::compact(energy.energy_supply)
                        ),
                        t.label(),
                    ),
                    1 => Line::styled("1 planet short", t.intent(Intent::Danger)),
                    n => Line::styled(format!("{n} planets short"), t.intent(Intent::Danger)),
                },
            });
            let heat_intent = if waste.overflowing() {
                Intent::Danger
            } else if heat_fill >= 0.75 {
                Intent::Warning
            } else {
                Intent::Primary
            };
            let mut foot = ui::spans::bar(t, heat_fill, 6, heat_intent);
            foot.push(Span::styled(
                format!(" {:.0}%", heat_fill * 100.0),
                t.label(),
            ));
            out.push(Metric {
                label: "HEAT".into(),
                value: Line::from(vec![
                    Span::styled(format!("{:.0}", waste.heat_load), heat_style),
                    Span::styled(format!(" / {:.0}", waste.heat_sink), t.label()),
                ]),
                foot: Line::from(foot),
            });
        } else {
            out.push(Metric {
                label: "Energy".into(),
                value: Line::styled(format!("{balance:+.0}"), energy_style),
                foot: Line::styled(
                    format!("heat {:.0}/{:.0}", waste.heat_load, waste.heat_sink),
                    if waste.overflowing() {
                        t.intent(Intent::Danger)
                    } else {
                        t.label()
                    },
                ),
            });
        }
        out
    }

    fn status_hints(&self, width: u16) -> Vec<(String, String)> {
        let k = &self.keymap;
        let mut hints = self.context_hints();
        hints.push(("1-6".to_string(), "hubs".to_string()));
        if width >= 100 && self.screen != Screen::Cockpit {
            hints.push((k.primary(Binding::NextScreen), "tabs".into()));
        }
        if self.screen != Screen::Cockpit {
            hints.push(("esc".into(), "bridge".into()));
        }
        hints.push((k.primary(Binding::Help), "help".into()));
        hints.push((k.primary(Binding::Quit), "quit".into()));
        hints
    }

    /// Per-screen keys as `(key, label)` pairs, split from `context_text`.
    fn context_hints(&self) -> Vec<(String, String)> {
        let cancel = self.keymap.primary(Binding::Cancel);
        self.context_text()
            .split("  ")
            .filter_map(|part| match part.trim().split_once(' ') {
                Some((key, label)) => Some((key.to_string(), label.to_string())),
                None if part.trim() == cancel => Some((cancel.clone(), "cancel".into())),
                None => None,
            })
            .collect()
    }

    fn window_buttons(&self) -> Vec<Span<'static>> {
        let action = match self.screen {
            Screen::Planet => "build",
            Screen::Shipyard => "queue",
            Screen::Overview => "research",
            Screen::Wonder => "contribute",
            Screen::Prestige => "buy",
            Screen::Market => "trade",
            Screen::Diplomacy => "tribute",
            Screen::Laws => "propose",
            Screen::Reports => "open report",
            Screen::History => "lineage",
            Screen::Staff => "assign",
            Screen::Automation => "dry-run",
            Screen::Map => "dispatch",
            Screen::Finance if self.state.finance.pending.is_empty() => return Vec::new(),
            Screen::Finance => "approve",
            Screen::Kpis => "events",
            Screen::Incidents => "resolve",
            Screen::Maintenance => "schedule",
            Screen::Scenarios => "promote",
            Screen::Cockpit
            | Screen::Achievements
            | Screen::Daily
            | Screen::Goals
            | Screen::Production
            | Screen::Ops => return Vec::new(),
        };
        let key = self.keymap.primary(Binding::Confirm);
        let mut buttons = Vec::new();
        if matches!(self.screen, Screen::Planet | Screen::Shipyard) {
            let cancel = self.keymap.primary(Binding::Cancel);
            buttons.push(ui::spans::button(
                &self.theme,
                &format!("{cancel} queue"),
                false,
            ));
        }
        buttons.push(ui::spans::button(
            &self.theme,
            &format!("{} {action}", if key == "enter" { "⏎" } else { &key }),
            true,
        ));
        buttons
    }

    fn seed_line(&self, width: u16) -> Line<'static> {
        let t = &self.theme;
        let code = Span::styled(encode_galaxy_seed(self.state.galaxy_seed), t.muted());
        let gen_label = format!(" gen {} ", self.state.generator_version);
        if width >= 100 {
            Line::from(vec![
                Span::styled("seed ", t.label()),
                code,
                Span::styled(format!(" {gen_label}"), t.label()),
            ])
        } else {
            Line::from(vec![code, Span::styled(gen_label, t.label())])
        }
    }

    fn window_callouts(&self, hub: Hub) -> Vec<Line<'static>> {
        let waste = self.state.waste_home();
        let warn = self.state.config.waste.warn_ratio;
        if hub != Hub::Colony
            || self.screen == Screen::Planet
            || !(waste.overflowing() || waste.warns(warn))
        {
            return Vec::new();
        }
        let intent = if waste.overflowing() {
            Intent::Danger
        } else {
            Intent::Warning
        };
        let mut lines = ui::panels::Callout {
            theme: &self.theme,
            intent,
            title: Line::from(waste_header_line(&self.state)),
            detail: None,
        }
        .lines();
        lines.push(Line::default());
        lines
    }

    fn cockpit_draw_inner(
        &self,
        frame: &mut Frame<'_>,
        area: ratatui::layout::Rect,
        footer: Vec<Line<'static>>,
    ) {
        crate::cockpit::draw(
            frame,
            area,
            crate::cockpit::View {
                theme: &self.theme,
                state: &self.state,
                cockpit: &self.cockpit,
                map_system: self.map_system,
                map_position: self.map_position,
                gauges: &self.gauge_history,
            },
            footer,
            self.cockpit.selected,
        );
    }

    fn draw_overlays(&self, frame: &mut Frame<'_>) {
        if self.replay.is_some() {
            self.draw_replay(frame);
        } else if let Some(away) = &self.away
            && !self.away_hidden
        {
            self.draw_away(frame, away);
        } else if self.state.pending_tactical.is_some() {
            self.draw_tactical(frame);
        } else if self.shows_victory_overlay() {
            self.draw_victory(frame);
        } else if let Some(event) = self.story_event() {
            self.draw_story(frame, event);
        } else if self.opened_report {
            self.draw_report(frame);
        } else if self.pending_invasion.is_some() {
            self.draw_invasion_approval(frame);
        } else if let Some(form) = &self.dispatch {
            if !self.docks_dispatch() {
                self.draw_dispatch(frame, form);
            }
        } else if self.screen == Screen::History && self.history_pane != HistoryPane::List {
            self.draw_history_overlay(frame);
        }
        if self.activity_open {
            self.draw_activity(frame);
        }
        if self.advisor_open {
            self.draw_advisor(frame);
        }
        if self.shows_detail_overlay()
            && let Some(detail) = self.current_detail()
        {
            self.dialog(
                frame,
                "Detail",
                String::new(),
                detail_panel::paragraph(&detail),
                &[],
            );
        }
        if self.help_open {
            self.draw_help(frame);
        }
        if let Some(updates) = &self.updates {
            updates.draw(frame);
        }
        if self.dismantle_confirm.is_some() {
            self.draw_dismantle_confirm(frame);
        }
        if self.mission_form.is_some() {
            self.draw_mission_form(frame);
        }
        if let Some(report) = &self.support {
            report.draw(frame);
        }
        if let Some(prompt) = &self.cheat_prompt {
            self.dialog(
                frame,
                "Cheat console",
                "Use permanently modifies this save".into(),
                vec![
                    Line::from(format!("> {prompt}")),
                    Line::from("Enter run  Esc close"),
                    Line::from(self.status.clone()),
                ],
                &[],
            );
        }
        if let Some(bug) = &self.bug {
            bug.draw(frame);
        }
    }

    fn current_portrait_slot(&self) -> Slot {
        let coord = self.selected_coord();
        portraits::slot_for_screen(
            self.screen == Screen::Planet,
            self.screen == Screen::Shipyard || self.screen == Screen::Staff,
            self.state.planet_kind(coord),
            self.state.rare_bonus_at(coord).is_some(),
            self.state
                .visible_events(Actor::Player)
                .last()
                .map(|e| e.text.as_str()),
        )
    }

    /// Overlay an inline graphics image after ratatui has painted the cells.
    pub fn paint_inline_graphics(
        &self,
        out: &mut impl std::io::Write,
        frame: ratatui::layout::Rect,
    ) -> std::io::Result<()> {
        if !self.portraits.enabled || self.portraits.protocol != Protocol::Graphics {
            return Ok(());
        }
        if self.help_open
            || self
                .updates
                .as_ref()
                .is_some_and(crate::update_ui::UpdateUi::overlay_open)
            || self.dismantle_confirm.is_some()
            || self.shows_detail_overlay()
            || self.activity_open
            || self.advisor_open
            || (self.away.is_some() && !self.away_hidden)
            || self.state.pending_tactical.is_some()
            || self.shows_victory_overlay()
            || self.story_event().is_some()
            || self.opened_report
            || self.dispatch.is_some()
            || self.pending_invasion.is_some()
            || self.replay.is_some()
            || (self.screen == Screen::History && self.history_pane != HistoryPane::List)
            || self.screen != Screen::Staff
        {
            return portraits::erase_inline_graphics(out);
        }
        let inner = ratatui::layout::Rect {
            x: frame.x.saturating_add(1),
            y: frame.y.saturating_add(1),
            width: frame.width.saturating_sub(2),
            height: frame.height.saturating_sub(2),
        };
        let Some(area) = portraits::split_area(inner).1 else {
            return Ok(());
        };
        let slot = self.current_portrait_slot();
        let mood = portraits::mood_for(slot, self.state.visible_events(Actor::Player));
        portraits::paint_inline_graphics(out, area, slot, mood)
    }

    fn story_event(&self) -> Option<&StoryEvent> {
        let pending = self.state.story_pending.as_ref()?;
        self.state.config.story.get(&pending.event_id)
    }

    fn choose_story_at(&mut self, index: usize, now_unix: u64) {
        let Some(id) = self
            .story_event()
            .and_then(|event| event.choices.get(index))
            .map(|choice| choice.id.clone())
        else {
            return;
        };
        match self.state.choose_story(&id, now_unix) {
            Ok(()) => {
                self.status = self
                    .state
                    .events
                    .last()
                    .map(|e| e.text.clone())
                    .unwrap_or_default();
            }
            Err(e) => self.status = e.to_string(),
        }
    }

    fn handle_replay_key(&mut self, code: KeyCode) -> Action {
        match self.keymap.lookup(HelpContext::Replay, code) {
            Some(Binding::Quit) => Action::Quit,
            Some(Binding::Cancel) => {
                self.replay = None;
                Action::None
            }
            Some(Binding::Toggle) => {
                if let Some(replay) = self.replay.as_mut() {
                    replay.paused = !replay.paused;
                }
                Action::None
            }
            Some(Binding::ReplayStep) => {
                if let Some(replay) = self.replay.as_mut()
                    && replay.frame + 1 < replay.log.report.frames.len()
                {
                    replay.frame += 1;
                    replay.paused = true;
                }
                Action::None
            }
            _ => Action::None,
        }
    }

    fn draw_replay(&self, frame: &mut Frame<'_>) {
        let Some(replay) = &self.replay else {
            return;
        };
        let report = &replay.log.report;
        let n = report.frames.len().max(1);
        let idx = replay.frame.min(n - 1);
        let snap = &report.frames[idx];
        let winner = match report.winner {
            CombatWinner::Attacker => "pirates",
            CombatWinner::Defender => "you",
            CombatWinner::Draw => "draw",
        };
        let t = &self.theme;
        let most = snap.attacker.total().max(snap.defender.total()).max(1) as f64;
        let side = |name: &str, n: u32, intent: Intent| {
            let mut spans = vec![Span::styled(format!("{name:<8}"), t.label())];
            spans.extend(ui::spans::bar(t, n as f64 / most, 24, intent));
            spans.push(Span::styled(format!("  {n}"), t.strong()));
            Line::from(spans)
        };
        let mut lines = vec![
            Line::from(vec![
                Span::styled(
                    format!("round {}/{}  ", snap.round, report.rounds.max(snap.round)),
                    t.text(),
                ),
                ui::spans::tag(
                    t,
                    &format!("WINNER {}", winner.to_uppercase()),
                    Intent::Primary,
                    true,
                ),
            ]),
            Line::default(),
            side("PIRATES", snap.attacker.total(), Intent::Danger),
            side("YOU", snap.defender.total(), Intent::Success),
        ];
        if snap.shots > 0 {
            lines.push(Line::from(format!(
                "shots {}  shields flare  {} explode",
                snap.shots, snap.explosions
            )));
        }
        if let Some(line) = replay.commentary.get(idx).and_then(|s| s.as_deref()) {
            lines.push(Line::from(line.to_string()));
        }
        // The report: who fought, what each side lost and what was taken.
        lines.push(Line::default());
        for (name, before, after) in [
            ("pirates", report.attacker_before, report.attacker_after),
            ("you", report.defender_before, report.defender_after),
        ] {
            lines.push(Line::from(vec![
                Span::styled(format!("{name:<9}"), t.label()),
                Span::styled(
                    format!(
                        "{} · lost {}",
                        before.label(),
                        before.losses_to(after).label()
                    ),
                    t.text(),
                ),
            ]));
        }
        if let Some(d) = replay.log.raid_defence {
            for (name, text) in defence_layer_lines(d) {
                lines.push(Line::from(vec![
                    Span::styled(format!("{name:<9}"), t.label()),
                    Span::styled(text, t.text()),
                ]));
            }
        }
        let plunder = resource_list(replay.log.raid_plunder.unwrap_or(Resources::ZERO));
        if !plunder.is_empty() {
            lines.push(Line::from(vec![
                Span::styled(format!("{:<9}", "plunder"), t.label()),
                Span::styled(plunder, t.intent(Intent::Danger)),
            ]));
        }
        let age = self.state.last_tick_unix.saturating_sub(replay.log.at);
        self.dialog(
            frame,
            "Combat report",
            format!(
                "{} ago · {} rounds",
                ui::spans::duration(age),
                report.rounds
            ),
            lines,
            &[
                (self.keymap.primary(Binding::Toggle), "pause"),
                (self.keymap.primary(Binding::ReplayStep), "step"),
                (self.keymap.primary(Binding::Cancel), "skip"),
            ],
        );
    }

    fn draw_away(&self, frame: &mut Frame<'_>, away: &AwaySummary) {
        let t = &self.theme;
        let ledger = &away.ledger;
        let total = ledger.total();
        let mut lines = Vec::new();
        let plundered = resource_list(total.plundered);
        if !plundered.is_empty() {
            lines.push(Line::styled(
                format!("Pirates plundered {plundered} while you were away."),
                t.intent(Intent::Danger),
            ));
        }
        if ledger.planets.len() > 1 {
            for planet in &ledger.planets {
                let nets: Vec<String> = ResourceKind::ALL
                    .iter()
                    .filter(|&&kind| planet.net(kind).abs() >= 0.5)
                    .map(|&kind| format!("{} {}", kind.abbr(), ui::spans::signed(planet.net(kind))))
                    .collect();
                if !nets.is_empty() {
                    lines.push(Line::styled(
                        format!("{:<9}{}", planet.coord.to_string(), nets.join("  ")),
                        t.text(),
                    ));
                }
            }
        }
        let rows = away_rows(&total);
        if !rows.is_empty() {
            let title = if ledger.planets.len() > 1 {
                "total"
            } else {
                ""
            };
            lines.push(Line::styled(
                format!(
                    "{title:<6}{:>10}{:>9}{:>9}{:>9}{:>9}{:>9}",
                    "PRODUCED", "SPENT", "RAIDS", "OTHER", "NET", "CAPPED"
                ),
                t.label(),
            ));
            lines.extend(rows.into_iter().map(|row| Line::styled(row, t.text())));
        }
        let raids = match (ledger.raids_won, ledger.raids_lost) {
            (0, lost) => lost.to_string(),
            (won, lost) => format!("{} ({won} won, {lost} lost)", won + lost),
        };
        lines.push(Line::styled(
            format!(
                "builds {}  research {}  fleets returned {}  expeditions {}  raids {raids}",
                away.builds, away.research, ledger.fleets_returned, ledger.expeditions
            ),
            t.text(),
        ));
        for (title, items) in [
            ("stalls", &away.stalls),
            ("deliveries", &away.deliveries),
            ("completed", &away.completed),
        ] {
            if items.is_empty() {
                continue;
            }
            lines.push(Line::default());
            lines.push(ui::panels::section(t, title));
            for item in items.iter().take(4) {
                lines.push(Line::styled(format!("· {item}"), t.text()));
            }
            if items.len() > 4 {
                lines.push(Line::styled(
                    format!("+{} more", items.len() - 4),
                    t.muted(),
                ));
            }
        }
        let toggle = self.keymap.primary(Binding::Toggle);
        if !away.events.is_empty() {
            lines.push(Line::default());
            lines.push(ui::panels::section(t, "events"));
            let skip = self.away_event_page * AWAY_EVENTS_PER_PAGE;
            for event in away
                .events
                .iter()
                .rev()
                .skip(skip)
                .take(AWAY_EVENTS_PER_PAGE)
            {
                lines.push(Line::styled(event.clone(), t.muted()));
            }
            let total = away.events.len();
            if total > AWAY_EVENTS_PER_PAGE {
                let last = (skip + AWAY_EVENTS_PER_PAGE).min(total);
                lines.push(Line::styled(
                    format!(
                        "{}-{last} of {total}, newest first ({toggle} for more)",
                        skip + 1
                    ),
                    t.muted(),
                ));
            }
        }
        self.dialog(
            frame,
            "While you were away",
            ui::spans::duration(away.elapsed_secs),
            lines,
            &[
                (self.keymap.primary(Binding::Confirm), "dismiss"),
                (self.keymap.primary(Binding::Replay), "replay"),
                (self.keymap.primary(Binding::ShowAway), "reopen on overview"),
            ],
        );
    }

    fn handle_tactical_key(&mut self, code: KeyCode, now_unix: u64) -> Action {
        match self.keymap.lookup(HelpContext::Tactical, code) {
            Some(Binding::Quit) => Action::Quit,
            Some(Binding::Confirm) => {
                self.state
                    .queue_tactical_action(hoshi_core::RoundAction::Hold);
                self.state.progress_tactical(now_unix);
                Action::None
            }
            Some(Binding::Cancel) => {
                self.state
                    .queue_tactical_action(hoshi_core::RoundAction::Retreat);
                self.state.progress_tactical(now_unix);
                Action::None
            }
            Some(Binding::Toggle) => {
                self.state
                    .queue_tactical_action(hoshi_core::RoundAction::CommitReserves);
                self.state.progress_tactical(now_unix);
                Action::None
            }
            _ => Action::None,
        }
    }

    fn draw_tactical(&self, frame: &mut Frame<'_>) {
        let Some(pending) = &self.state.pending_tactical else {
            return;
        };
        let round = pending.actions.len().saturating_add(1);
        let admiral = self
            .state
            .admirals
            .iter()
            .find(|a| Some(a.id) == self.state.home_admiral_id);
        let mut lines = vec![
            Line::from(format!(
                "round {round}  {} / {}",
                pending.def_plan.formation, pending.def_plan.stance
            )),
            Line::from(format!(
                "pirates {}   you {}",
                pending.attacker.total(),
                pending.defender.total()
            )),
        ];
        if let Some(adm) = admiral {
            lines.push(Line::from(format!(
                "admiral {}  {}  xp {}",
                adm.name, adm.trait_id, adm.xp
            )));
        }
        self.dialog(
            frame,
            "Tactical",
            format!("round {round}"),
            lines,
            &[
                ("⏎".into(), "hold"),
                ("x".into(), "retreat"),
                ("space".into(), "commit reserves"),
            ],
        );
    }

    fn draw_story(&self, frame: &mut Frame<'_>, event: &StoryEvent) {
        let t = &self.theme;
        let mut lines = vec![Line::styled(event.text.clone(), t.text()), Line::default()];
        for (i, choice) in event.choices.iter().enumerate() {
            let mut spans = vec![
                ui::spans::keycap(t, &(i + 1).to_string()),
                Span::styled(format!(" {}", choice.label), t.strong()),
            ];
            if choice.id == event.timeout_choice {
                spans.push(Span::styled("  default if ignored", t.muted()));
            }
            lines.push(Line::from(spans));
        }
        self.dialog(
            frame,
            &event.title,
            "story".into(),
            lines,
            &[("1-9".into(), "chooses; ignored too long uses the default")],
        );
    }

    fn scenario_fork_or_queue(&mut self) {
        if self.scenario.is_none() {
            self.scenario = Some(Scenario::create(&self.state));
            self.status.clear();
            return;
        }
        let Some(scenario) = self.scenario.as_mut() else {
            return;
        };
        let fork = scenario.baseline().at;
        let intent = hoshi_core::Intent::Build {
            planet: Coord::HOME,
            building: BuildingKind::MetalMine,
            actor: Actor::Player,
        };
        match scenario.queue(intent, fork) {
            Ok(()) => self.status.clear(),
            Err(e) => self.status = e.to_string(),
        }
    }

    fn scenario_advance(&mut self) {
        let Some(scenario) = self.scenario.as_mut() else {
            self.status = "no scenario".into();
            return;
        };
        let until = scenario.outcome().at.saturating_add(3600);
        scenario.advance(until);
        self.status.clear();
    }

    fn scenario_promote(&mut self, now_unix: u64) {
        let Some(scenario) = self.scenario.as_ref() else {
            self.status = "no scenario".into();
            return;
        };
        match scenario.promote(&mut self.state, now_unix) {
            Ok(()) => {
                self.scenario = None;
                self.status.clear();
            }
            Err(e) => self.status = e.to_string(),
        }
    }

    fn scenario_discard(&mut self) {
        self.scenario = None;
        self.status.clear();
    }

    fn scenario_row_count(&self) -> usize {
        self.scenario_compare_rows().len()
    }

    fn scenario_compare_rows(&self) -> Vec<(String, String, String, String)> {
        let Some(scenario) = self.scenario.as_ref() else {
            return Vec::new();
        };
        let live = Scenario::create(&self.state).outcome();
        let plan = scenario.outcome();
        let mut rows = Vec::new();
        for kind in ResourceKind::ALL {
            rows.push((
                kind.as_key().into(),
                format!("{:.0}", live.resources.get(kind)),
                format!("{:.0}", plan.resources.get(kind)),
                String::new(),
            ));
        }
        let mut kpi_keys = Vec::new();
        for row in live.kpis.iter().chain(plan.kpis.iter()) {
            let key = row.kpi.as_key();
            if !kpi_keys.contains(&key) {
                kpi_keys.push(key);
            }
        }
        for key in kpi_keys {
            let left = live
                .kpis
                .iter()
                .find(|row| row.kpi.as_key() == key)
                .map(|row| format!("{:.1}", row.value))
                .unwrap_or_else(|| "-".into());
            let right = plan
                .kpis
                .iter()
                .find(|row| row.kpi.as_key() == key)
                .map(|row| format!("{:.1}", row.value))
                .unwrap_or_else(|| "-".into());
            rows.push((key.into(), left, right, String::new()));
        }
        let mut risk_ids = Vec::new();
        for row in live.risks.iter().chain(plan.risks.iter()) {
            if !risk_ids.contains(&row.id) {
                risk_ids.push(row.id);
            }
        }
        for id in risk_ids {
            let a = live.risks.iter().find(|row| row.id == id);
            let b = plan.risks.iter().find(|row| row.id == id);
            let name = a
                .or(b)
                .map(|row| row.name.as_str())
                .unwrap_or("risk")
                .to_string();
            let left = a
                .map(|row| format!("{:.2}", row.likelihood))
                .unwrap_or_else(|| "-".into());
            let right = b
                .map(|row| format!("{:.2}", row.likelihood))
                .unwrap_or_else(|| "-".into());
            rows.push((name, left, right, String::new()));
        }
        let mut keys = Vec::new();
        for fact in live.intel.iter().chain(plan.intel.iter()) {
            let key = (fact.object.clone(), fact.field.clone());
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
        keys.sort();
        for (object, field) in keys {
            let left_fact = live
                .intel
                .iter()
                .find(|fact| fact.object == object && fact.field == field);
            let right_fact = plan
                .intel
                .iter()
                .find(|fact| fact.object == object && fact.field == field);
            rows.push((
                format!("{object}.{field}"),
                left_fact.map(intel_fact_cell).unwrap_or_else(|| "-".into()),
                right_fact
                    .map(intel_fact_cell)
                    .unwrap_or_else(|| "-".into()),
                intel_conf_cell(left_fact, right_fact),
            ));
        }
        rows
    }

    fn activate(&mut self, now_unix: u64) {
        match self.screen {
            Screen::Planet => {
                let Some(kind) = BuildingKind::ALL.get(self.selected).copied() else {
                    return;
                };
                let coord = self.selected_coord();
                match self.state.enqueue_build_on(coord, kind, now_unix) {
                    Ok(()) => self.status.clear(),
                    Err(e) => self.status = e.to_string(),
                }
            }
            Screen::Shipyard => {
                if let Some(kind) = ShipKind::ALL.get(self.selected).copied() {
                    match self.state.enqueue_ships(kind, self.ship_batch, now_unix) {
                        Ok(()) => self.status.clear(),
                        Err(e) => self.status = e.to_string(),
                    }
                    return;
                }
                let di = self.selected.saturating_sub(ShipKind::ALL.len());
                let Some(kind) = DefenceKind::ALL.get(di).copied() else {
                    return;
                };
                match self.state.enqueue_defence(kind, self.ship_batch, now_unix) {
                    Ok(()) => self.status.clear(),
                    Err(e) => self.status = e.to_string(),
                }
            }
            Screen::Overview => {
                let Some(id) = self.tech_id_at(self.selected) else {
                    return;
                };
                match self.state.queue_research(&id, now_unix) {
                    Ok(()) => self.status.clear(),
                    Err(e) => self.status = e.to_string(),
                }
            }
            Screen::Wonder => self.activate_wonder(now_unix),
            Screen::Prestige => self.activate_prestige(now_unix),
            Screen::Achievements | Screen::Daily | Screen::Goals | Screen::Production => {}
            Screen::Ops => self.open_mission_form(),
            Screen::Automation => self.activate_automation(),
            Screen::Finance => self.activate_finance(now_unix),
            Screen::Kpis => self.activate_kpi(),
            Screen::Incidents => self.activate_incident(),
            Screen::Maintenance => self.activate_maintenance(),
            Screen::Market => self.activate_market(now_unix),
            Screen::Diplomacy => self.activate_diplomacy(now_unix),
            Screen::Laws => self.activate_law(now_unix),
            Screen::History => self.activate_history(),
            Screen::Staff => self.activate_staff(now_unix),
            Screen::Map => self.activate_map(now_unix),
            Screen::Cockpit => self.cockpit_confirm(),
            Screen::Scenarios => self.scenario_promote(now_unix),
            Screen::Reports => {
                if self
                    .selected_outpost()
                    .and_then(|c| self.state.latest_report(c))
                    .is_some()
                {
                    self.opened_report = true;
                    self.status.clear();
                } else {
                    self.status = "no spy report for this outpost".into();
                }
            }
        }
    }

    fn selected_outpost(&self) -> Option<Coord> {
        self.state.outposts.get(self.selected).map(|o| o.coord)
    }

    fn send_probe(&mut self, now_unix: u64) {
        let Some(dest) = self.selected_outpost() else {
            self.status = "no outpost selected".into();
            return;
        };
        let mut ships = Fleet::default();
        ships.add(ShipKind::EspionageProbe, 1);
        match self.state.launch_spy(ships, dest, 100, now_unix) {
            Ok(_) => self.status = format!("probe launched to {dest}"),
            Err(e) => self.status = e.to_string(),
        }
    }

    fn activate_law(&mut self, now_unix: u64) {
        let templates: Vec<_> = self.state.config.laws.iter().cloned().collect();
        if self.selected < templates.len() {
            let spec = &templates[self.selected];
            let jurisdiction = match spec.jurisdiction {
                JurisdictionKind::Empire => Jurisdiction::Empire,
                JurisdictionKind::Planet => Jurisdiction::Planet(self.selected_coord()),
            };
            match self
                .state
                .propose_law(&spec.id, spec.default, jurisdiction, 0, now_unix)
            {
                Ok(id) => self.status = format!("proposed {} #{id}", spec.title),
                Err(e) => self.status = e.to_string(),
            }
            return;
        }
        let index = self.selected - templates.len();
        let Some(law) = self.state.laws.laws.get(index).cloned() else {
            return;
        };
        let result = match law.status {
            LawStatus::Proposed | LawStatus::RepealProposed => self
                .state
                .approve_law(law.id, now_unix)
                .map(|()| format!("approved {} #{}", law.template_id, law.id)),
            LawStatus::Enacted => self
                .state
                .repeal_law(law.id, 0, now_unix)
                .map(|()| format!("repeal proposed {} #{}", law.template_id, law.id)),
            LawStatus::Repealed => {
                self.status = "already repealed".into();
                return;
            }
        };
        match result {
            Ok(msg) => self.status = msg,
            Err(e) => self.status = e.to_string(),
        }
    }

    fn map_cursor(&self) -> Coord {
        Coord {
            galaxy: self.map_galaxy,
            system: self.map_system,
            position: self.map_position,
        }
    }

    fn cycle_map_galaxy(&mut self) {
        let home = self.state.home.galaxy;
        let frontier = self.state.config.frontier.galaxy;
        if self.state.frontier.revealed == 0 || frontier == home {
            self.map_galaxy = home;
            return;
        }
        self.map_galaxy = if self.map_galaxy == home {
            frontier
        } else {
            home
        };
        self.status = format!("map galaxy {}", self.map_galaxy);
    }

    fn nudge_map_system(&mut self, delta: i32) {
        let next = i32::from(self.map_system) + delta;
        self.map_system = if next < 1 {
            MAP_SYSTEM_MAX
        } else if next > i32::from(MAP_SYSTEM_MAX) {
            1
        } else {
            next as u16
        };
    }

    fn nudge_map_position(&mut self, delta: i32) {
        let next = i32::from(self.map_position) + delta;
        self.map_position = if next < 1 {
            MAP_SLOT_COUNT
        } else if next > i32::from(MAP_SLOT_COUNT) {
            1
        } else {
            next as u16
        };
    }

    fn nudge_dispatch(&mut self, delta: i32) {
        let Some(form) = self.dispatch.as_mut() else {
            return;
        };
        let len = ShipKind::ALL.len() as i32;
        form.selected = ((form.selected as i32 + delta).rem_euclid(len)) as usize;
    }

    fn nudge_dispatch_ships(&mut self, delta: i32) {
        let Some(form) = self.dispatch.as_mut() else {
            return;
        };
        let kind = ShipKind::ALL[form.selected];
        let have = self.state.fleet.get(kind);
        let cur = form.ships.get(kind);
        let next = (cur as i32 + delta).clamp(0, have as i32) as u32;
        if next > cur {
            form.ships.add(kind, next - cur);
        } else {
            form.ships.saturating_sub(kind, cur - next);
        }
    }

    fn activate_map(&mut self, now_unix: u64) {
        if self.dispatch.is_some() {
            self.confirm_dispatch(now_unix);
            return;
        }
        let dest = self.map_cursor();
        if dest == self.state.home {
            self.status = "same location".into();
            return;
        }
        let Some(&mission) = self.dispatch_missions(dest).first() else {
            self.status = format!("{dest} is yours: plan deliveries in Fleet › Ops");
            return;
        };
        self.dispatch = Some(DispatchForm {
            dest,
            ships: Fleet::default(),
            selected: 0,
            mission,
        });
        self.status.clear();
    }

    /// Missions that fit `dest`, the default first: the expedition slot,
    /// an outpost (invade first once its defences are gone), or anywhere
    /// else to settle. Own planets have none; Ops plans their deliveries.
    fn dispatch_missions(&self, dest: Coord) -> &'static [Mission] {
        if dest == self.state.expedition_dest() {
            &[Mission::Expedition]
        } else if let Some(outpost) = self.state.outpost(dest) {
            if outpost.defences.total() == 0 {
                &[Mission::Invade, Mission::Attack, Mission::Spy]
            } else {
                &[Mission::Attack, Mission::Spy, Mission::Invade]
            }
        } else if self.state.owned_coords().contains(&dest) {
            &[]
        } else {
            &[Mission::Colonize]
        }
    }

    /// The selected shipyard unit, as a ship or a defence.
    fn shipyard_unit(&self) -> Option<Result<ShipKind, DefenceKind>> {
        match ShipKind::ALL.get(self.selected) {
            Some(&kind) => Some(Ok(kind)),
            None => DefenceKind::ALL
                .get(self.selected - ShipKind::ALL.len())
                .map(|&kind| Err(kind)),
        }
    }

    /// Cost and build time of `count` of the selected shipyard unit, priced
    /// as the queue charges it (the unit cost, like the table rows).
    fn shipyard_batch(&self, count: u32) -> Option<(Resources, u64)> {
        let n = f64::from(count);
        Some(match self.shipyard_unit()? {
            Ok(kind) => {
                let unit = self.state.config.ships.get(kind).unit_cost();
                (
                    Resources::from_parts(unit.iron * n, unit.silicon * n, unit.deuterium * n),
                    self.state.ship_time_secs(kind) * u64::from(count),
                )
            }
            Err(kind) => (
                self.state.defence_cost(kind, count),
                self.state.defence_time_secs(kind) * u64::from(count),
            ),
        })
    }

    /// The largest batch of the selected unit the stockpile pays for (at least 1).
    fn max_ship_batch(&self) -> u32 {
        let Some((unit, _)) = self.shipyard_batch(1) else {
            return 1;
        };
        let stock = self.state.resources;
        ResourceKind::ALL
            .iter()
            .filter(|&&kind| unit.get(kind) > 0.0)
            .map(|&kind| (stock.get(kind) / unit.get(kind)).floor())
            .fold(f64::from(MAX_SHIP_BATCH), f64::min)
            .clamp(1.0, f64::from(MAX_SHIP_BATCH)) as u32
    }

    fn cycle_dispatch_mission(&mut self) {
        let Some(form) = self.dispatch else {
            return;
        };
        let missions = self.dispatch_missions(form.dest);
        if missions.is_empty() {
            // The target became ours while the form was open.
            self.status = format!("{} is yours: plan deliveries in Fleet › Ops", form.dest);
            self.dispatch = None;
            return;
        }
        let at = missions
            .iter()
            .position(|m| *m == form.mission)
            .unwrap_or(0);
        if let Some(form) = self.dispatch.as_mut() {
            form.mission = missions[(at + 1) % missions.len()];
        }
    }

    fn confirm_dispatch(&mut self, now_unix: u64) {
        let Some(form) = self.dispatch else {
            return;
        };
        if self.governance && self.is_invasion_dispatch(&form) {
            self.pending_invasion = Some(form);
            self.status = format!(
                "invasion-approval: invasion of {} needs human approval",
                form.dest
            );
            return;
        }
        let (ships, dest) = (form.ships, form.dest);
        let launched = match form.mission {
            Mission::Attack => self.state.launch_attack(ships, dest, 100, now_unix),
            Mission::Spy => self.state.launch_spy(ships, dest, 100, now_unix),
            Mission::Invade => self.state.launch_invasion(ships, dest, 100, now_unix),
            Mission::Colonize => {
                if ships.get(ShipKind::ColonyShip) != 1 || ships.total() != 1 {
                    self.status = "colonize needs exactly one colony ship".into();
                    return;
                }
                self.state.launch_colony(dest, 100, now_unix)
            }
            Mission::Expedition => {
                let hold = self.state.config.expedition.min_hold_secs;
                self.state.launch_expedition(ships, hold, 100, now_unix)
            }
        };
        match launched {
            Ok(_) => {
                self.status = format!("{} launched to {dest}", form.mission.label());
                self.dispatch = None;
            }
            Err(e) => self.status = e.to_string(),
        }
    }

    fn is_invasion_dispatch(&self, form: &DispatchForm) -> bool {
        form.mission == Mission::Invade
            && form.ships.get(ShipKind::TroopTransport) > 0
            && self
                .state
                .outpost(form.dest)
                .is_some_and(|outpost| outpost.defences.total() == 0)
    }

    fn resolve_invasion(&mut self, grant: bool, now_unix: u64) {
        let Some(form) = self.pending_invasion.take() else {
            return;
        };
        if !grant {
            self.status = format!(
                "invasion-approval: invasion of {} needs human approval (denied)",
                form.dest
            );
            self.dispatch = None;
            return;
        }
        match self
            .state
            .launch_invasion(form.ships, form.dest, 100, now_unix)
        {
            Ok(_) => {
                self.status = format!("invasion of {} approved", form.dest);
                self.dispatch = None;
            }
            Err(e) => {
                self.status = e.to_string();
                self.pending_invasion = Some(form);
            }
        }
    }

    fn draw_invasion_approval(&self, frame: &mut Frame<'_>) {
        let dest = self
            .pending_invasion
            .map(|form| form.dest)
            .unwrap_or(Coord::HOME);
        let t = &self.theme;
        let mut lines = vec![Line::styled(
            format!("Approve invasion of {dest}?"),
            t.strong(),
        )];
        lines.push(Line::default());
        lines.push(Line::from(vec![
            Span::styled("▌ ", t.intent(Intent::Warning)),
            Span::styled("policy invasion-approval", t.muted()),
        ]));
        self.dialog(
            frame,
            "Invasion approval",
            String::new(),
            lines,
            &[("⏎".into(), "grant"), ("x".into(), "deny")],
        );
    }

    fn draw_dispatch(&self, frame: &mut Frame<'_>, form: &DispatchForm) {
        self.dialog(
            frame,
            "Dispatch",
            String::new(),
            self.dispatch_lines(form),
            &[],
        );
    }

    fn activate_diplomacy(&mut self, now_unix: u64) {
        let Some(spec) = self.state.config.diplomacy.iter().nth(self.selected) else {
            return;
        };
        let id = spec.id.clone();
        let name = spec.name.clone();
        match self.state.pay_tribute(&id, now_unix) {
            Ok(()) => self.status = format!("tribute paid to {name}"),
            Err(e) => self.status = e.to_string(),
        }
    }

    fn activate_market(&mut self, now_unix: u64) {
        let Some(&(pay, recv)) = TRADE_PAIRS.get(self.selected) else {
            return;
        };
        match self
            .state
            .trade_market(pay, recv, self.trade_amount, now_unix)
        {
            Ok(quote) => {
                self.status = format!(
                    "{:.0} {} -> {:.1} {}",
                    quote.pay_amount,
                    pay.label(),
                    quote.recv_amount,
                    recv.label()
                );
            }
            Err(e) => self.status = e.to_string(),
        }
    }

    fn activate_wonder(&mut self, now_unix: u64) {
        let Some(spec) = self.state.config.wonders.get(self.selected) else {
            return;
        };
        let id = spec.id.clone();
        if let Some(job) = &self.state.wonder
            && job.id != id
        {
            self.status = format!("wonder {} is already under construction", job.id);
            return;
        }
        if self.state.wonder.is_none() {
            match self.state.start_wonder(&id, now_unix) {
                Ok(()) => {}
                Err(e) => {
                    self.status = e.to_string();
                    return;
                }
            }
        }
        let coord = self.selected_coord();
        match self.state.contribute_wonder(coord, now_unix) {
            Ok(_) => self.status.clear(),
            Err(WonderError::NothingToContribute)
                if self
                    .state
                    .wonder
                    .as_ref()
                    .is_some_and(|job| job.completes_at.is_some()) =>
            {
                self.status.clear();
            }
            Err(e) => self.status = e.to_string(),
        }
    }

    fn cancel(&mut self, now_unix: u64) {
        match self.screen {
            Screen::Planet => {
                let coord = self.selected_coord();
                match self.state.cancel_build_on(coord, 0, now_unix) {
                    Ok(_) => self.status.clear(),
                    Err(e) => self.status = e.to_string(),
                }
            }
            Screen::Shipyard => match self.state.cancel_ships(0, now_unix) {
                Ok(_) => self.status.clear(),
                Err(e) => self.status = e.to_string(),
            },
            Screen::Overview => match self.state.cancel_research(now_unix) {
                Ok(_) => self.status.clear(),
                Err(e) => self.status = e.to_string(),
            },
            Screen::Wonder => {}
            Screen::Prestige => {
                self.confirming_prestige = false;
                self.status.clear();
            }
            Screen::Map => {
                self.dispatch = None;
                self.pending_invasion = None;
            }
            Screen::Cockpit => {
                let _ = self.cockpit.back();
            }
            Screen::History => {
                self.history_pane = HistoryPane::List;
            }
            Screen::Staff => self.revoke_selected_leader(now_unix),
            Screen::Achievements
            | Screen::Daily
            | Screen::Goals
            | Screen::Production
            | Screen::Market
            | Screen::Diplomacy
            | Screen::Laws
            | Screen::Reports
            | Screen::Ops
            | Screen::Finance
            | Screen::Kpis
            | Screen::Incidents
            | Screen::Maintenance => {}
            Screen::Scenarios => self.scenario_discard(),
            Screen::Automation => self.remove_selected_automation_rule(),
        }
    }

    fn list_len(&self) -> usize {
        match self.screen {
            Screen::Planet => BuildingKind::ALL.len(),
            Screen::Shipyard => ShipKind::ALL.len() + DefenceKind::ALL.len(),
            Screen::Overview => self.overview_techs().len(),
            Screen::Wonder => self.state.config.wonders.len(),
            Screen::Prestige => 1 + self.state.config.prestige.bonuses.len(),
            Screen::Achievements => self.state.config.achievements.len(),
            Screen::Daily => self.state.daily.slots.len(),
            Screen::Goals => 3,
            Screen::Production => self
                .state
                .production_report()
                .steps
                .iter()
                .filter(|step| step.planet == self.selected_coord())
                .count(),
            Screen::Market => TRADE_PAIRS.len(),
            Screen::Diplomacy => self.state.config.diplomacy.len(),
            Screen::Laws => self.state.config.laws.len() + self.state.laws.laws.len(),
            Screen::Reports => self.state.outposts.len(),
            Screen::History => self.history_rows().len(),
            Screen::Staff => self.state.leaders.len(),
            Screen::Automation => {
                self.state.automation_catalog().tier.len() + self.state.automation_rules.len()
            }
            Screen::Finance => self.state.finance.pending.len() + self.state.finance.budgets.len(),
            Screen::Kpis => self.state.kpi_snapshot().measurements.len(),
            Screen::Incidents => self.state.incident_snapshot().open.len(),
            Screen::Maintenance => self.state.maintenance_snapshot().assets.len(),
            Screen::Ops => {
                self.state.vehicles.len()
                    + self.state.planned.len()
                    + self
                        .state
                        .mission_schedule(self.state.last_tick_unix)
                        .alerts
                        .len()
            }
            Screen::Map => 0,
            Screen::Cockpit => crate::cockpit::select_len(&self.state, &self.cockpit),
            Screen::Scenarios => self.scenario_row_count(),
        }
    }

    fn tech_id_at(&self, index: usize) -> Option<String> {
        self.overview_techs().get(index).map(|spec| spec.id.clone())
    }

    /// Visible techs of the current branch by tier, with the branch's
    /// keystone last so it gets its own group on the Research screen.
    fn overview_techs(&self) -> Vec<&hoshi_core::TechSpec> {
        let mut techs: Vec<_> = self
            .state
            .config
            .techs
            .branch_techs(self.tree_branch)
            .into_iter()
            .filter(|spec| self.state.tech_node_state(&spec.id) != TechNodeState::Hidden)
            .collect();
        techs.sort_by_key(|spec| spec.keystone);
        techs
    }

    fn visible_tree_branches(&self) -> Vec<TechBranch> {
        let mut branches = TechBranch::PLAYABLE.to_vec();
        if self.state.ancient_unlocked() {
            branches.push(TechBranch::Ancient);
        }
        branches
    }

    fn nudge_tree_branch(&mut self, delta: i32) {
        let branches = self.visible_tree_branches();
        if branches.is_empty() {
            return;
        }
        let i = branches
            .iter()
            .position(|b| *b == self.tree_branch)
            .unwrap_or(0);
        let next = (i as i32 + delta).rem_euclid(branches.len() as i32) as usize;
        self.tree_branch = branches[next];
        self.selected = 0;
    }

    fn bridge_bar(&self) -> Option<Line<'static>> {
        if !self.bridge_ui {
            return None;
        }
        let now = self.state.last_tick_unix;
        let mut parts: Vec<String> = Vec::new();
        if let Some(line) = self.bridge_log.last() {
            let mark = if line.alert { "!" } else { " " };
            parts.push(format!("{mark}{}  {}", line.source, line.effect));
        } else {
            parts.push("bridge idle".into());
        }
        if let Some(until) = self.state.activity.prod_boost_until
            && now < until
        {
            parts.push(format!("mines {}s", until - now));
        }
        if let Some(until) = self.state.activity.research_boost_until
            && now < until
        {
            parts.push(format!("lab {}s", until - now));
        }
        let style = if self.bridge_log.last().is_some_and(|l| l.alert) {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        Some(Line::from(Span::styled(parts.join("  ·  "), style)))
    }

    fn footer_lines(&self, width: usize) -> Vec<Line<'static>> {
        let mut rows = Vec::new();
        // The hint waits until the update notice, drawn over the same rows, closes.
        let disclosure = self
            .updates
            .as_ref()
            .is_some_and(crate::update_ui::UpdateUi::disclosure_open);
        if self.show_tutorial
            && !disclosure
            && let Some(hint) = self.state.pending_hint()
        {
            for line in wrap_text(&format!("Hint: {}", hint.title), width) {
                rows.push(Line::from(line));
            }
            for line in wrap_text(&hint.body, width) {
                rows.push(Line::from(line));
            }
            for line in wrap_text("x dismiss  g show me  s skip tutorial", width) {
                rows.push(Line::from(line));
            }
        }
        if let Some(bar) = self.bridge_bar() {
            rows.push(bar);
        }
        rows
    }

    fn context_text(&self) -> String {
        let k = &self.keymap;
        let planets = format!(
            "{}/{} planet",
            k.primary(Binding::PrevPlanet),
            k.primary(Binding::NextPlanet)
        );
        let select = format!(
            "{}/{} select",
            k.primary(Binding::SelectNext),
            k.primary(Binding::SelectPrev)
        );
        let confirm = k.primary(Binding::Confirm);
        let cancel = k.primary(Binding::Cancel);
        let activity = if self.bridge_ui {
            format!("  {} log", k.primary(Binding::Activity))
        } else {
            String::new()
        };
        let advisor = if self.ai_advisor {
            format!("  {} advisor", k.primary(Binding::Advisor))
        } else {
            String::new()
        };
        format!(
            "{}{activity}{advisor}",
            match self.screen {
                Screen::Overview => format!(
                    "{planets}  {}/{} branch  {select}  {confirm} research  {} replay  {cancel}  {} copy seed  {} detail",
                    k.primary(Binding::CursorLeft),
                    k.primary(Binding::CursorRight),
                    k.primary(Binding::Replay),
                    k.primary(Binding::CopySeed),
                    k.primary(Binding::Detail)
                ),
                Screen::Planet => format!(
                    "{planets}  {select}  {confirm} build  {cancel} queue  {} detail",
                    k.primary(Binding::Detail)
                ),
                Screen::Shipyard => format!(
                    "{planets}  {select}  {confirm} queue  {cancel} queue  {} detail",
                    k.primary(Binding::Detail)
                ),
                Screen::Wonder => format!(
                    "{planets}  {select}  {confirm} start/contribute  {} detail",
                    k.primary(Binding::Detail)
                ),
                Screen::Prestige => {
                    format!("{select}  {confirm} buy/reset  {cancel}")
                }
                Screen::Achievements | Screen::Daily | Screen::Goals => {
                    format!("{planets}  {select}")
                }
                Screen::Production =>
                    format!("{planets}  {select}  {} detail", k.primary(Binding::Detail)),
                Screen::Market => format!(
                    "{select}  {}/{} amount  {confirm} trade  {} detail",
                    k.primary(Binding::Increase),
                    k.primary(Binding::Decrease),
                    k.primary(Binding::Detail)
                ),
                Screen::Diplomacy => {
                    format!(
                        "{select}  {confirm} tribute  {} detail",
                        k.primary(Binding::Detail)
                    )
                }
                Screen::Laws => {
                    format!(
                        "{select}  {confirm} propose/approve/repeal  {} detail",
                        k.primary(Binding::Detail)
                    )
                }
                Screen::Reports => format!(
                    "{select}  {confirm} report  {} send probe",
                    k.primary(Binding::SendProbe)
                ),
                Screen::History => format!(
                    "{select}  {confirm} lineage  {} glance  +/- filter",
                    k.primary(Binding::Replay)
                ),
                Screen::Staff => format!(
                    "{select}  {confirm} assign/delegate  {cancel} revoke  {} recruit",
                    k.primary(Binding::Increase)
                ),
                Screen::Automation => {
                    format!(
                        "{select}  {confirm} dry-run  {} toggle  {} add  {cancel} remove",
                        k.primary(Binding::Toggle),
                        k.primary(Binding::Increase)
                    )
                }
                // Approve only means something while a spend is held.
                Screen::Finance if self.state.finance.pending.is_empty() => select,
                Screen::Finance => {
                    format!("{select}  {confirm} approve")
                }
                Screen::Kpis => {
                    format!("{select}  {confirm} events")
                }
                Screen::Incidents => {
                    format!("{select}  {confirm} resolve")
                }
                Screen::Maintenance => {
                    format!("{select}  {confirm} schedule")
                }
                Screen::Ops => select,
                Screen::Map => format!(
                    "{}{}{}{} cursor  {confirm} dispatch  {} detail",
                    k.primary(Binding::CursorLeft),
                    k.primary(Binding::SelectNext),
                    k.primary(Binding::SelectPrev),
                    k.primary(Binding::CursorRight),
                    k.primary(Binding::Detail)
                ),
                Screen::Cockpit => format!(
                    "{} layout  {}/{} panel  {select}  {confirm} drill  {cancel} back",
                    k.primary(Binding::Toggle),
                    k.primary(Binding::CursorLeft),
                    k.primary(Binding::CursorRight)
                ),
                Screen::Scenarios => format!(
                    "{} fork/queue  {} 1h  {confirm} promote  {cancel} discard",
                    k.primary(Binding::Increase),
                    k.primary(Binding::Toggle)
                ),
            }
        )
    }

    fn draw_help(&self, frame: &mut Frame<'_>) {
        let mut lines = self.keymap.help_lines(self.help_context());
        if self.state.cheats_enabled() {
            lines.push(format!(
                "  {}  local cheat console",
                self.keymap.display(Binding::CheatConsole)
            ));
        }
        lines.push("  b  Report a bug".into());
        if self.bridge_ui {
            lines.insert(
                2,
                format!("  {}  activity log", self.keymap.display(Binding::Activity)),
            );
        }
        if self.ai_advisor {
            lines.insert(
                2,
                format!("  {}  advisor", self.keymap.display(Binding::Advisor)),
            );
        }
        if !matches!(
            self.help_context(),
            HelpContext::Story | HelpContext::Victory
        ) {
            lines.insert(
                2,
                "  1-6  hubs: bridge colony fleet trade empire records".into(),
            );
            lines.insert(
                3,
                "  shift+tab  previous screen   esc  back to bridge".into(),
            );
        }
        lines.insert(2, "Fe iron  Si silicon  REE rare earths".into());
        lines.insert(3, "H2O water ice  He3 helium-3  D deuterium".into());
        lines.insert(4, String::new());
        if self.help_commands {
            lines = crate::help::command_lines(&Hub::of(self.screen).title().to_ascii_lowercase());
        } else {
            if let Some(first) = lines.first_mut() {
                first.push_str("   [Keys]   Commands");
            }
        }
        // Help-only keys stay off every global binding listed above.
        lines.push("v Changelog   u dismiss update".into());
        lines.push("p Support report".into());
        let t = &self.theme;
        let lines = lines
            .into_iter()
            .map(|line| Line::styled(line, t.text()))
            .collect();
        self.dialog(
            frame,
            "Help",
            String::new(),
            lines,
            &[
                ("tab".into(), "page"),
                (self.keymap.primary(Binding::Help), "close"),
            ],
        );
    }

    fn open_advisor(&mut self) {
        let tips = advise(&self.state);
        self.advisor_lines = polish_all_named(
            &tips.into_iter().map(|row| row.text).collect::<Vec<_>>(),
            &self.advisor_model_url,
            self.advisor_allow_lan,
            &self.advisor_model_name,
        );
        self.advisor_open = true;
    }

    fn draw_advisor(&self, frame: &mut Frame<'_>) {
        let t = &self.theme;
        let mut lines = vec![
            Line::styled("advisor (suggestions only; nothing is queued)", t.muted()),
            Line::default(),
        ];
        if self.advisor_lines.is_empty() {
            lines.push(Line::styled("no suggestions", t.faint()));
        }
        for text in &self.advisor_lines {
            lines.push(Line::from(vec![
                Span::styled("▌ ", t.intent(Intent::Primary)),
                Span::styled(text.clone(), t.text()),
            ]));
        }
        self.dialog(
            frame,
            "Advisor",
            String::new(),
            lines,
            &[(self.keymap.primary(Binding::Cancel), "closes")],
        );
    }

    fn draw_activity(&self, frame: &mut Frame<'_>) {
        let mut lines = vec![
            Line::from("latest bridge events (source · effect)"),
            Line::from(""),
        ];
        if self.bridge_log.is_empty() {
            lines.push(Line::from("no events yet"));
        } else {
            let newest_first: Vec<&BridgeLine> = self.bridge_log.iter().rev().collect();
            let start = self
                .activity_scroll
                .min(newest_first.len().saturating_sub(1));
            for line in newest_first.into_iter().skip(start).take(16) {
                let mark = if line.alert { "!" } else { " " };
                let style = if line.alert {
                    Style::default().add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                lines.push(Line::from(Span::styled(
                    format!("{mark}{}  {}", line.source, line.effect),
                    style,
                )));
            }
        }
        self.dialog(
            frame,
            "Activity",
            String::new(),
            lines,
            &[
                (self.keymap.primary(Binding::Cancel), "closes"),
                (
                    format!(
                        "{}/{}",
                        self.keymap.primary(Binding::SelectNext),
                        self.keymap.primary(Binding::SelectPrev)
                    ),
                    "scroll",
                ),
            ],
        );
    }

    fn planet_info_lines(&self) -> Vec<Line<'static>> {
        let coord = self.selected_coord();
        let queue = self.state.queue_at(coord).unwrap_or(&[]);
        let mut rows: Vec<Line<'static>> = Vec::new();
        let kind = self
            .state
            .planet_kind(coord)
            .map(|k| k.as_key())
            .unwrap_or("unknown");
        let size = self.state.planet_size(coord).unwrap_or(0);
        rows.push(Line::from(format!(
            "Planet {coord}  {kind}  size {size}  Queue {}/{}",
            queue.len(),
            self.state.build_queue_cap()
        )));
        if let Some(output) = self.state.energy_produced_at(coord) {
            rows.push(Line::from(format!(
                "Energy {output:.1}/{:.1}  balance {:+.1}  solar flux {:.0}%  solar plant cap lv{}",
                self.state.energy_consumed_at(coord),
                self.state.energy_balance_at(coord).unwrap_or(0.0),
                self.state.config.orbits.solar_flux(coord) * 100.0,
                self.state.max_solar_level(coord).unwrap_or(0)
            )));
        }
        if let Some(rare) = self.state.rare_bonus_at(coord) {
            rows.push(Line::from(format!(
                "Rare: {}  {}",
                rare.name,
                rare.summary()
            )));
        }
        if let Some(waste) = self.state.waste_at(coord) {
            rows.push(Line::from(waste_detail_line(
                &waste,
                self.state.config.waste.warn_ratio,
                self.state.config.waste.unrest_per_overflow,
            )));
        }
        if let Some(moon) = self.state.moon_at(coord) {
            let ready = if moon.jump_ready_at > self.state.last_tick_unix {
                format!(
                    "jump {}",
                    ui::spans::duration(
                        moon.jump_ready_at.saturating_sub(self.state.last_tick_unix)
                    )
                )
            } else {
                "jump ready".into()
            };
            rows.push(Line::from(format!(
                "Moon  phalanx {}  gate {}  {ready}",
                moon.phalanx, moon.jump_gate
            )));
            if moon.phalanx > 0 {
                match self
                    .state
                    .phalanx_contacts(coord, self.state.last_tick_unix)
                {
                    Ok(contacts) if contacts.is_empty() => {
                        rows.push(Line::from("Phalanx  no fleets in range"));
                    }
                    Ok(contacts) => {
                        for contact in contacts {
                            rows.push(Line::from(format!(
                                "Phalanx  fleet #{} {} {} → {}  {}  ships {}",
                                contact.id,
                                contact.phase,
                                contact.origin,
                                contact.dest,
                                ui::spans::duration(contact.eta_secs),
                                contact.ships
                            )));
                        }
                    }
                    Err(_) => {}
                }
            }
        } else if let Some(amount) = self.state.debris.get(&coord)
            && *amount > 0.0
        {
            rows.push(Line::from(format!("Debris {amount:.0}")));
        }
        let deposits = self.state.deposits_at(coord);
        let parts: Vec<String> = deposits
            .iter()
            .map(|(id, richness)| format!("{} {richness:.2}", self.state.display_name(id)))
            .collect();
        // Three per line so water ice, the deuterium input, is never cut off.
        for (i, chunk) in parts.chunks(3).enumerate() {
            let label = if i == 0 { "Deposits" } else { "" };
            rows.push(Line::from(format!("{label:<10}{}", chunk.join("  "))));
        }
        for (i, job) in queue.iter().enumerate() {
            let left = job
                .completes_at
                .map(|t| t.saturating_sub(self.state.last_tick_unix));
            let timer = match left {
                Some(s) => ui::spans::duration(s),
                None => "wait".to_string(),
            };
            rows.push(Line::from(format!(
                "  {}. [{}] {} → lv{}  {timer}",
                i + 1,
                job.kind.as_key(),
                job.building,
                if job.kind == hoshi_core::BuildJobKind::Dismantle {
                    job.from_level.saturating_sub(1)
                } else {
                    job.from_level.saturating_add(1)
                }
            )));
        }
        rows
    }

    fn wonder_block_reason(&self, spec: &hoshi_core::WonderSpec) -> Option<String> {
        if self.state.completed_wonders.iter().any(|id| id == &spec.id) {
            return Some("done".into());
        }
        if let Some(job) = &self.state.wonder {
            if job.id == spec.id {
                return None;
            }
            return Some(format!("busy {}", job.id));
        }
        for pre in &spec.prereq_techs {
            let actual = self.state.tech_level(&pre.id);
            if actual < pre.level {
                return Some(format!(
                    "need {} {}",
                    self.state.display_name(&pre.id),
                    pre.level
                ));
            }
        }
        None
    }

    fn defence_block_reason(&self, kind: DefenceKind, cost: Resources) -> Option<String> {
        let spec = self.state.config.defences.get(kind);
        // The same level the queue gate and the inspector check.
        if self.state.available_building_level(BuildingKind::Shipyard) < spec.shipyard {
            return Some(format!("need shipyard {}", spec.shipyard));
        }
        if !self.state.resources.can_afford(cost) {
            return Some(missing_label(self.state.resources.missing(cost)));
        }
        None
    }

    /// Short status-chip form of a prerequisite: `need research lab 1`.
    fn need_label(&self, gap: &hoshi_core::PrereqGap) -> String {
        match gap {
            hoshi_core::PrereqGap::Building {
                building, required, ..
            } => format!("need {building} {required}"),
            hoshi_core::PrereqGap::Tech { id, required, .. } => {
                format!("need {} {required}", self.state.display_name(id))
            }
        }
    }

    fn ship_block_reason(&self, kind: ShipKind, cost: Resources) -> Option<String> {
        let spec = self.state.config.ships.get(kind);
        // The same level the queue gate and the inspector check.
        if self.state.available_building_level(BuildingKind::Shipyard) < spec.shipyard {
            return Some(format!("need shipyard {}", spec.shipyard));
        }
        if let Some(gap) = self.state.missing_ship_prereqs(kind).first() {
            return Some(self.need_label(gap));
        }
        if !self.state.resources.can_afford(cost) {
            return Some(missing_label(self.state.resources.missing(cost)));
        }
        None
    }

    fn building_block_reason(&self, kind: BuildingKind) -> Option<String> {
        let coord = self.selected_coord();
        let buildings = self
            .state
            .buildings_at(coord)
            .unwrap_or(&self.state.buildings);
        let queue = self.state.queue_at(coord).unwrap_or(&[]);
        for &(req, required) in &self.state.config.buildings.get(kind).prerequisites {
            let actual =
                buildings.get(req) + queue.iter().filter(|job| job.building == req).count() as u32;
            if actual < required {
                return Some(format!("need {req} {required}"));
            }
        }
        for prerequisite in &self.state.config.buildings.get(kind).tech_prerequisites {
            if self.state.tech_level(&prerequisite.id) < prerequisite.level {
                return Some(format!("need {} {}", prerequisite.id, prerequisite.level));
            }
        }
        let cost = self.state.build_cost_on(coord, kind);
        let resources = self
            .state
            .resources_at(coord)
            .copied()
            .unwrap_or(self.state.resources);
        if !resources.can_afford(cost) {
            return Some(missing_label(resources.missing(cost)));
        }
        None
    }

    fn tech_block_reason(&self, id: &str, cost: Resources) -> Option<String> {
        match self.state.tech_node_state(id) {
            TechNodeState::Excluded => return Some("excluded".into()),
            TechNodeState::Hidden => return Some("prestige".into()),
            _ => {}
        }
        if let Ok(gaps) = self.state.missing_research_prereqs(id)
            && let Some(gap) = gaps.first()
        {
            return Some(self.need_label(gap));
        }
        if !self.state.resources.can_afford(cost) {
            return Some(missing_label(self.state.resources.missing(cost)));
        }
        None
    }

    fn activate_prestige(&mut self, now_unix: u64) {
        if self.selected == 0 {
            if self.confirming_prestige {
                match self.state.prestige_reset(now_unix) {
                    Ok(_) => {
                        self.confirming_prestige = false;
                        self.status.clear();
                    }
                    Err(e) => {
                        self.confirming_prestige = false;
                        self.status = e.to_string();
                    }
                }
            } else {
                self.confirming_prestige = true;
                self.status.clear();
            }
            return;
        }
        self.confirming_prestige = false;
        let Some(spec) = self.state.config.prestige.bonuses.get(self.selected - 1) else {
            return;
        };
        let id = spec.id.clone();
        match self.state.buy_prestige_bonus(&id, now_unix) {
            Ok(()) => self.status.clear(),
            Err(e) => self.status = e.to_string(),
        }
    }

    fn shows_victory_overlay(&self) -> bool {
        self.state
            .victory
            .as_ref()
            .is_some_and(|record| !record.kept_playing)
    }

    fn draw_victory(&self, frame: &mut Frame<'_>) {
        let Some(record) = &self.state.victory else {
            return;
        };
        let t = &self.theme;
        let lines = vec![
            Line::styled(
                format!("Galaxy complete: {}", record.path.label()),
                t.intent(Intent::Success).add_modifier(Modifier::BOLD),
            ),
            Line::styled(
                format!(
                    "+{} prestige in {}",
                    record.prestige,
                    ui::spans::duration(record.elapsed_secs)
                ),
                t.text(),
            ),
            Line::default(),
            Line::styled("The prestige screen starts a new galaxy.", t.muted()),
        ];
        self.dialog(
            frame,
            "Victory",
            String::new(),
            lines,
            &[(self.keymap.primary(Binding::Confirm), "keep playing")],
        );
    }

    fn record_production_sample(&mut self) {
        let rate = self.state.production_report().metal_per_hour;
        self.prod_history.push(rate);
        const CAP: usize = 24;
        if self.prod_history.len() > CAP {
            let drop = self.prod_history.len() - CAP;
            self.prod_history.drain(..drop);
        }
    }

    fn history_rows(&self) -> Vec<&hoshi_core::HistoryEvent> {
        self.state.history.search(&HistoryQuery {
            kind: self.history_kind,
            actor: self.history_actor.clone(),
            ..HistoryQuery::default()
        })
    }

    fn selected_history(&self) -> Option<&hoshi_core::HistoryEvent> {
        self.history_rows().get(self.selected).copied()
    }

    fn cycle_history_kind(&mut self) {
        self.selected = 0;
        self.history_kind = match self.history_kind {
            None => Some(HistoryKind::ALL[0]),
            Some(kind) => {
                let idx = HistoryKind::ALL
                    .iter()
                    .position(|k| *k == kind)
                    .unwrap_or(0);
                HistoryKind::ALL.get(idx + 1).copied()
            }
        };
    }

    fn cycle_history_actor(&mut self) {
        self.selected = 0;
        self.history_actor = match self.history_actor.as_deref() {
            None => Some("player".into()),
            Some("player") => Some("simulation".into()),
            Some("simulation") => Some("bridge".into()),
            _ => None,
        };
    }

    /// Enter on a fight opens its combat report; other rows toggle lineage.
    fn activate_history(&mut self) {
        let fight =
            self.selected_history()
                .filter(|row| row.kind == HistoryKind::Combat)
                .and_then(|row| {
                    // History records each fight with its winner; two fights can
                    // share a tick, so match both.
                    let winner = row.objects.first().map(String::as_str);
                    self.state.combat_logs.iter().rposition(|log| {
                        log.at == row.at && winner == Some(log.report.winner.as_key())
                    })
                });
        match fight {
            Some(index) if self.history_pane == HistoryPane::List => self.open_replay(index),
            _ => self.toggle_history_lineage(),
        }
    }

    fn toggle_history_lineage(&mut self) {
        self.history_pane = if self.history_pane == HistoryPane::Lineage {
            HistoryPane::List
        } else {
            HistoryPane::Lineage
        };
    }

    fn toggle_history_as_of(&mut self) {
        self.history_pane = if self.history_pane == HistoryPane::AsOf {
            HistoryPane::List
        } else {
            HistoryPane::AsOf
        };
    }

    fn activate_automation(&mut self) {
        let preview = self.state.preview_automation();
        if preview.is_empty() {
            self.status = "no rules".into();
            return;
        }
        self.status = preview
            .iter()
            .map(|p| format!("#{} {}", p.rule_id, p.detail))
            .collect::<Vec<_>>()
            .join(" · ");
    }

    fn toggle_selected_automation_rule(&mut self) {
        let tiers = self.state.automation_catalog().tier.len();
        if self.selected < tiers {
            self.status = "select a rule".into();
            return;
        }
        let idx = self.selected - tiers;
        let Some(id) = self.state.automation_rules.get(idx).map(|r| r.id) else {
            return;
        };
        match self.state.toggle_automation_rule(id) {
            Ok(on) => self.status = format!("rule #{id} {}", if on { "on" } else { "off" }),
            Err(e) => self.status = e.to_string(),
        }
    }

    fn add_default_automation_rule(&mut self) {
        match self.state.add_automation_rule(
            hoshi_core::RuleCondition {
                kind: hoshi_core::RuleConditionKind::StockBelow,
                planet: self.state.home,
                resource: ResourceKind::Iron,
                amount: 2_000.0,
            },
            hoshi_core::RuleAction::QueueBuild {
                planet: self.state.home,
                building: BuildingKind::MetalMine,
            },
        ) {
            Ok(id) => self.status = format!("added rule #{id} keep metal on home"),
            Err(e) => self.status = e.to_string(),
        }
    }

    fn remove_selected_automation_rule(&mut self) {
        let tiers = self.state.automation_catalog().tier.len();
        if self.selected < tiers {
            self.status = "select a rule".into();
            return;
        }
        let idx = self.selected - tiers;
        let Some(id) = self.state.automation_rules.get(idx).map(|r| r.id) else {
            return;
        };
        match self.state.remove_automation_rule(id) {
            Ok(()) => self.status = format!("removed rule #{id}"),
            Err(e) => self.status = e.to_string(),
        }
    }

    fn activate_kpi(&mut self) {
        let snap = self.state.kpi_snapshot();
        let Some(row) = snap.measurements.get(self.selected) else {
            self.status = "select a kpi".into();
            return;
        };
        if row.event_ids.is_empty() {
            self.status = format!(
                "{} {} no source events",
                row.kpi.as_key(),
                row.owner.as_key()
            );
        } else {
            let ids = row
                .event_ids
                .iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(",");
            self.status = format!("{} {} events {ids}", row.kpi.as_key(), row.owner.as_key());
        }
    }

    fn activate_incident(&mut self) {
        let snap = self.state.incident_snapshot();
        let Some(row) = snap.open.get(self.selected) else {
            self.status = "select an incident".into();
            return;
        };
        let id = row.id;
        match self.state.resolve_incident(
            id,
            "resolved from ops",
            hoshi_core::Actor::Player,
            self.state.last_tick_unix,
        ) {
            Ok(()) => self.status = format!("incident #{id} resolved"),
            Err(e) => self.status = e.to_string(),
        }
    }

    fn activate_maintenance(&mut self) {
        let snap = self.state.maintenance_snapshot();
        let Some(asset) = snap.assets.get(self.selected) else {
            self.status = "select an asset".into();
            return;
        };
        let id = asset.id;
        match self.state.schedule_maintenance(
            id,
            hoshi_core::PlanKind::Time,
            hoshi_core::Actor::Player,
            self.state.last_tick_unix,
        ) {
            Ok(order) => self.status = format!("work order #{order} on asset {id}"),
            Err(e) => self.status = e.to_string(),
        }
    }

    fn activate_finance(&mut self, now_unix: u64) {
        let Some(row) = self.state.finance.pending.get(self.selected) else {
            self.status = "select a held spend".into();
            return;
        };
        let id = row.id;
        match self
            .state
            .approve_spend(id, hoshi_core::Actor::Player, now_unix)
        {
            Ok(()) => self.status = format!("approved spend #{id}"),
            Err(e) => self.status = e.to_string(),
        }
    }

    fn selected_leader_id(&self) -> Option<u64> {
        self.state.leaders.get(self.selected).map(|row| row.id)
    }

    fn activate_staff(&mut self, now_unix: u64) {
        let Some(id) = self.selected_leader_id() else {
            return;
        };
        if self.state.assignment_of(id).is_none() {
            let coord = self.selected_coord();
            match self.state.assign_leader(
                id,
                RoleKind::Governor,
                AssignmentTarget::Planet(coord),
                now_unix,
            ) {
                Ok(()) => self.status.clear(),
                Err(e) => self.status = e.to_string(),
            }
            return;
        }
        let permission = self
            .state
            .assignment_of(id)
            .and_then(|row| {
                RoleKind::ALL
                    .iter()
                    .copied()
                    .find(|role| *role == row.role)
                    .and_then(|role| {
                        [
                            Permission::QueueBuild,
                            Permission::AcceptBattle,
                            Permission::ProposeLaw,
                        ]
                        .into_iter()
                        .find(|perm| role.grants(*perm))
                    })
            })
            .unwrap_or(Permission::QueueBuild);
        match self.state.delegate_leader(id, permission, None, now_unix) {
            Ok(()) => self.status.clear(),
            Err(e) => self.status = e.to_string(),
        }
    }

    fn recruit_selected_leader(&mut self, now_unix: u64) {
        match self.state.recruit_leader("", "none", now_unix) {
            Ok(_) => self.status.clear(),
            Err(e) => self.status = e.to_string(),
        }
    }

    fn revoke_selected_leader(&mut self, now_unix: u64) {
        let Some(id) = self.selected_leader_id() else {
            return;
        };
        match self.state.revoke_delegation(id, None, now_unix) {
            Ok(()) => self.status.clear(),
            Err(e) => self.status = e.to_string(),
        }
    }

    fn draw_history_overlay(&self, frame: &mut Frame<'_>) {
        let Some(event) = self.selected_history() else {
            self.dialog(
                frame,
                "History",
                String::new(),
                vec![Line::from("no event selected")],
                &[],
            );
            return;
        };
        match self.history_pane {
            HistoryPane::List => {}
            HistoryPane::Lineage => {
                let mut lines = vec![Line::from(format!("lineage of #{}", event.id))];
                let now = self.state.last_tick_unix;
                for row in self.state.history.lineage(event.id) {
                    lines.push(Line::from(format!(
                        "#{} {} ago {} {} → {}",
                        row.id,
                        ui::spans::duration(now.saturating_sub(row.at)),
                        row.kind.as_key(),
                        row.action.replace('_', " "),
                        row.outcome
                    )));
                }
                lines.push(Line::from(""));
                lines.push(Line::from(format!(
                    "{} closes",
                    self.keymap.primary(Binding::Cancel)
                )));
                self.dialog(frame, "Lineage", String::new(), lines, &[]);
            }
            HistoryPane::AsOf => {
                let snap = self.state.history.as_of(event.at);
                let mut lines = vec![Line::from(format!(
                    "empire {} ago",
                    ui::spans::duration(self.state.last_tick_unix.saturating_sub(event.at))
                ))];
                match snap {
                    Some(snap) => {
                        lines.push(Line::from(format!(
                            "Fe {:.0}  Si {:.0}  D {:.0}  energy {:.0}  heat {:.0}",
                            snap.iron, snap.silicon, snap.deuterium, snap.energy, snap.heat
                        )));
                        lines.push(Line::from(format!(
                            "home {}  buildings {}",
                            snap.home_kind, snap.building_levels
                        )));
                    }
                    None => lines.push(Line::from("outside retained window")),
                }
                lines.push(Line::from(""));
                lines.push(Line::from(format!(
                    "{} closes",
                    self.keymap.primary(Binding::Cancel)
                )));
                self.dialog(frame, "Time travel", String::new(), lines, &[]);
            }
        }
    }

    fn draw_report(&self, frame: &mut Frame<'_>) {
        let dest = self.selected_outpost();
        let title = dest
            .map(|c| format!(" spy {c} "))
            .unwrap_or_else(|| " spy ".into());
        let lines = match dest {
            Some(coord) => {
                let reports = self.state.reports_for(coord);
                if reports.is_empty() {
                    vec![Line::from("no spy report for this outpost")]
                } else {
                    report_overlay_lines(&self.state, &reports, self.state.last_tick_unix)
                }
            }
            None => vec![Line::from("no spy report for this outpost")],
        };
        self.dialog(frame, title.trim(), String::new(), lines, &[]);
    }
}

fn waste_header_line(state: &GameState) -> String {
    let waste = state.waste_home();
    let warn = state.config.waste.warn_ratio;
    let unrest = waste.unrest_factor(state.config.waste.unrest_per_overflow);
    if waste.overflowing() {
        format!(
            "Waste      heat {:.0}/{:.0}  pol {:.0}/{:.0}  unrest {:.2}",
            waste.heat_load, waste.heat_sink, waste.pollution_load, waste.pollution_sink, unrest
        )
    } else if waste.warns(warn) {
        format!(
            "Waste      heat {:.0}/{:.0}  pol {:.0}/{:.0}  WARN",
            waste.heat_load, waste.heat_sink, waste.pollution_load, waste.pollution_sink
        )
    } else {
        format!(
            "Waste      heat {:.0}/{:.0}  pol {:.0}/{:.0}",
            waste.heat_load, waste.heat_sink, waste.pollution_load, waste.pollution_sink
        )
    }
}

fn waste_detail_line(
    waste: &hoshi_core::WasteReport,
    warn_ratio: f64,
    unrest_per_overflow: f64,
) -> String {
    let mut line = format!(
        "Heat {:.0}/{:.0}  Pollution {:.0}/{:.0}",
        waste.heat_load, waste.heat_sink, waste.pollution_load, waste.pollution_sink
    );
    if waste.overflowing() {
        line.push_str(&format!(
            "  unrest {:.2}",
            waste.unrest_factor(unrest_per_overflow)
        ));
    } else if waste.warns(warn_ratio) {
        line.push_str("  WARN");
    }
    line
}

fn intel_conf_cell(
    live: Option<&hoshi_core::IntelFact>,
    plan: Option<&hoshi_core::IntelFact>,
) -> String {
    let a = live
        .and_then(|fact| fact.observations.last())
        .map(|obs| obs.confidence);
    let b = plan
        .and_then(|fact| fact.observations.last())
        .map(|obs| obs.confidence);
    match (a, b) {
        (Some(a), Some(b)) if (a - b).abs() < 1e-9 => format!("{a:.2}"),
        (Some(a), Some(b)) => format!("{a:.2}/{b:.2}"),
        (Some(a), None) => format!("{a:.2}"),
        (None, Some(b)) => format!("{b:.2}"),
        (None, None) => String::new(),
    }
}

fn intel_fact_cell(fact: &hoshi_core::IntelFact) -> String {
    let nums: Vec<f64> = fact
        .observations
        .iter()
        .filter_map(|obs| match &obs.value {
            IntelValue::Count(n) => Some(f64::from(*n)),
            IntelValue::Number(n) => Some(*n),
            IntelValue::Text(_) => None,
        })
        .collect();
    if nums.is_empty() {
        return fact
            .observations
            .last()
            .and_then(|obs| match &obs.value {
                IntelValue::Text(text) => Some(text.clone()),
                _ => None,
            })
            .unwrap_or_else(|| "-".into());
    }
    let min = nums.iter().copied().fold(f64::INFINITY, f64::min);
    let max = nums.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if (max - min).abs() < 1e-9 {
        format!("{min:.0}")
    } else {
        format!("{min:.0}-{max:.0}")
    }
}

fn pad_x(area: Rect, n: u16) -> Rect {
    Rect {
        x: area.x + n.min(area.width),
        width: area.width.saturating_sub(2 * n),
        ..area
    }
}

fn window_area(frame: Rect, body: Rect) -> Rect {
    if frame.width >= 120 && frame.height >= 40 {
        let width = body.width.saturating_sub(16).min(150);
        Rect {
            x: body.x + (body.width - width) / 2,
            y: body.y + 1,
            width,
            height: body.height - 2,
        }
    } else {
        body
    }
}

fn detail_column_width(total: u16) -> u16 {
    (total * 7 / 20).clamp(30, 56).min(total.saturating_sub(10))
}

fn report_overlay_lines(state: &GameState, reports: &[&SpyReport], now: u64) -> Vec<Line<'static>> {
    let mut ordered: Vec<&SpyReport> = reports.to_vec();
    ordered.sort_by_key(|r| std::cmp::Reverse((r.observed_at, r.id)));
    let mut lines = Vec::new();
    for (i, report) in ordered.iter().enumerate() {
        if i > 0 {
            lines.push(Line::from(""));
        }
        lines.push(Line::from(format!(
            "{}  {}  age {}  conf {:.2}  score {:.0}",
            report.source,
            report.tier,
            ui::spans::duration(report.age_secs(now)),
            state.report_confidence(report),
            report.score
        )));
        lines.push(Line::from(format!(
            "observed {}  detected {}  lost {}/{}",
            report.observed_at,
            if report.detected { "yes" } else { "no" },
            report.probes_lost,
            report.probes_sent
        )));
        if let (Some(kind), Some(size)) = (report.kind, report.size) {
            lines.push(Line::from(format!("planet {} size {size}", kind.as_key())));
        }
        if let Some(res) = report.resources {
            lines.push(Line::from(format!("resources {}", stockpile_label(res))));
        }
        if let Some(fleet) = report.fleet {
            lines.push(Line::from(format!("fleet {}", fleet_brief(fleet))));
        }
        if let Some(defences) = report.defences {
            lines.push(Line::from(format!("defences {}", fleet_brief(defences))));
        }
        if let Some(garrison) = report.garrison {
            lines.push(Line::from(format!("garrison {garrison}")));
        }
    }
    lines.push(Line::from("Enter closes"));
    lines
}

fn fleet_brief(fleet: Fleet) -> String {
    let parts: Vec<String> = ShipKind::ALL
        .iter()
        .filter_map(|&kind| {
            let n = fleet.get(kind);
            (n > 0).then(|| format!("{}:{n}", kind.as_key()))
        })
        .collect();
    if parts.is_empty() {
        "none".into()
    } else {
        parts.join(" ")
    }
}

fn stockpile_label(stock: Resources) -> String {
    ResourceKind::ALL
        .iter()
        .map(|&kind| format!("{} {:.0}", kind.abbr(), stock.get(kind)))
        .collect::<Vec<_>>()
        .join("  ")
}

fn missing_label(missing: Resources) -> String {
    let parts: Vec<String> = ResourceKind::ALL
        .iter()
        .copied()
        .filter(|&kind| missing.get(kind) > 0.5)
        .map(|kind| format!("{} {}", kind.abbr(), ui::spans::compact(missing.get(kind))))
        .collect();
    // "short", not "need": the number is what is missing, not the price.
    if parts.is_empty() {
        "short of resources".to_string()
    } else {
        format!("short {}", parts.join(" "))
    }
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        for chunk in wrap_chunks(word, width) {
            if line.is_empty() {
                line = chunk;
            } else if line.chars().count() + 1 + chunk.chars().count() <= width {
                line.push(' ');
                line.push_str(&chunk);
            } else {
                lines.push(std::mem::take(&mut line));
                line = chunk;
            }
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn wrap_chunks(word: &str, width: usize) -> Vec<String> {
    if word.chars().count() <= width {
        return vec![word.to_string()];
    }
    let mut out = Vec::new();
    let mut buf = String::new();
    for c in word.chars() {
        if buf.chars().count() == width {
            out.push(std::mem::take(&mut buf));
        }
        buf.push(c);
    }
    if !buf.is_empty() {
        out.push(buf);
    }
    out
}

/// One `(label, text)` row per defence layer that took part in a raid, in
/// battle order: interception, stations, hangar, dome, batteries, rebuild.
fn defence_layer_lines(d: hoshi_core::RaidDefence) -> Vec<(&'static str, String)> {
    let mut rows = Vec::new();
    if d.intercept_in > 0 {
        rows.push((
            "missiles",
            format!("{} in · {} shot down", d.intercept_in, d.intercept_down),
        ));
    }
    for (name, before, after) in [
        ("stations", d.stations_before, d.stations_after),
        ("hangar", d.hangar_before, d.hangar_after),
    ] {
        if before > 0 {
            rows.push((name, format!("{before} → {after}")));
        }
    }
    if d.dome_absorbed > 0.0 || d.dome_collapsed {
        rows.push((
            "dome",
            format!(
                "absorbed {:.0}{}",
                d.dome_absorbed,
                if d.dome_collapsed {
                    " · collapsed"
                } else {
                    ""
                }
            ),
        ));
    }
    if d.batteries_before > 0 {
        rows.push((
            "battery",
            format!("{} → {}", d.batteries_before, d.batteries_after),
        ));
    }
    if d.rebuilt > 0 {
        rows.push(("rebuilt", d.rebuilt.to_string()));
    }
    rows
}

/// `Fe 2.7k Si 1.2k` for every resource above half a unit.
fn resource_list(amounts: Resources) -> String {
    ResourceKind::ALL
        .iter()
        .filter(|&&kind| amounts.get(kind) >= 0.5)
        .map(|&kind| format!("{} {}", kind.abbr(), ui::spans::compact(amounts.get(kind))))
        .collect::<Vec<_>>()
        .join(" ")
}

/// One away-table row per resource that moved. PRODUCED is what reached
/// storage; CAPPED is production the full store discarded.
fn away_rows(ledger: &hoshi_core::PlanetLedger) -> Vec<String> {
    let cell = |v: f64| {
        if v.abs() < 0.5 {
            "·".to_string()
        } else {
            ui::spans::signed(v)
        }
    };
    ResourceKind::ALL
        .iter()
        .filter_map(|&kind| {
            let produced = ledger.produced.get(kind) - ledger.capped.get(kind);
            let other = ledger.gained.get(kind) + ledger.transfers_in.get(kind)
                - ledger.transfers_out.get(kind);
            let terms = [
                produced,
                ledger.spent.get(kind),
                ledger.plundered.get(kind),
                other,
                ledger.capped.get(kind),
            ];
            if terms.iter().all(|v| v.abs() < 0.5) {
                return None;
            }
            Some(format!(
                "{:<6}{:>10}{:>9}{:>9}{:>9}{:>9}{:>9}",
                kind.abbr(),
                cell(produced),
                cell(-ledger.spent.get(kind)),
                cell(-ledger.plundered.get(kind)),
                cell(other),
                ui::spans::signed(ledger.net(kind)),
                cell(ledger.capped.get(kind))
                    .trim_start_matches('+')
                    .to_string(),
            ))
        })
        .collect()
}

/// Deposits the building's own recipes extract from at `planet`, in recipe
/// order. A building without an extractor recipe reads none.
fn building_deposits(state: &GameState, kind: BuildingKind, planet: Coord) -> Vec<(String, f64)> {
    let deposits = state.deposits_at(planet);
    let mut out: Vec<(String, f64)> = Vec::new();
    for recipe in state
        .config
        .chains
        .recipes
        .iter()
        .filter(|r| r.building == kind)
    {
        if let Some(id) = &recipe.deposit
            && !out.iter().any(|(seen, _)| seen == id)
            && let Some(richness) = deposits.get(id)
        {
            out.push((id.clone(), *richness));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoshi_core::{
        BalanceConfig, Buildings, Clock, Colony, Coord, DiplomacyConfig, FakeClock, Fleet,
        GameState, MarketConfig, Outpost, PlanetType, RaidConfig, Resources, StoryCatalog, Vehicle,
        VictoryPath, VictoryRecord, load_state, open_save, save_state,
    };
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn render_text(app: &App, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();
        buffer_text(terminal.backend())
    }

    fn buffer_text(backend: &TestBackend) -> String {
        let buf = backend.buffer();
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

    /// True when `needle` is on screen and its first cell sits on the dimmed backdrop.
    fn text_is_dimmed(app: &App, width: u16, height: u16, needle: &str) -> bool {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();
        let buf = terminal.backend().buffer();
        let dimmed = app.theme.surface(Surface::Dimmed).bg;
        for y in 0..buf.area.height {
            let cells: Vec<_> = (0..buf.area.width).map(|x| &buf[(x, y)]).collect();
            let row: String = cells.iter().map(|c| c.symbol()).collect();
            if let Some(at) = row.find(needle) {
                let col = row[..at].chars().count();
                return cells[col].bg == dimmed.unwrap_or_default();
            }
        }
        false
    }

    fn line_is_dim(backend: &TestBackend, needle: &str) -> bool {
        let buf = backend.buffer();
        let area = buf.area();
        for y in area.y..area.y + area.height {
            let mut line = String::new();
            let mut dim = false;
            for x in area.x..area.x + area.width {
                let cell = &buf[(x, y)];
                line.push_str(cell.symbol());
                if cell.modifier.contains(Modifier::DIM) {
                    dim = true;
                }
            }
            if line.contains(needle) {
                return dim;
            }
        }
        false
    }

    #[test]
    fn building_inspector_lists_the_deposits_that_building_extracts() {
        let mut app = App::new(GameState::new(1_000));
        app.screen = Screen::Planet;
        let deposits_of = |app: &mut App, kind: BuildingKind| {
            app.selected = BuildingKind::ALL.iter().position(|k| *k == kind).unwrap();
            let (_, rows) = app.inspector_context();
            rows.into_iter()
                .filter(|(label, _)| label == "Deposit")
                .map(|(_, line)| line.to_string())
                .collect::<Vec<_>>()
        };
        let metal = deposits_of(&mut app, BuildingKind::MetalMine);
        assert!(metal[0].starts_with("iron ore "), "{metal:?}");
        assert!(metal.iter().all(|row| !row.contains("helium")), "{metal:?}");
        let synth = deposits_of(&mut app, BuildingKind::DeuteriumSynthesizer);
        assert!(
            synth.iter().any(|row| row.starts_with("water ice ")),
            "{synth:?}"
        );
        assert!(
            synth.iter().any(|row| row.starts_with("helium-3 ")),
            "{synth:?}"
        );
        assert!(deposits_of(&mut app, BuildingKind::SolarPlant).is_empty());
    }

    #[test]
    fn market_amount_steps_by_size_and_the_screen_shows_blockers_before_enter() {
        let mut state = GameState::new(1_000);
        state.config.market.cargo_free = 100.0;
        state.resources.silicon = 4_596.0;
        let mut app = App::new(state);
        app.screen = Screen::Market;
        app.selected = TRADE_PAIRS
            .iter()
            .position(|p| *p == (ResourceKind::Silicon, ResourceKind::Deuterium))
            .unwrap();
        for _ in 0..9 {
            app.handle_key(KeyCode::Char('+'), 1_000);
        }
        assert_eq!(app.trade_amount, 100.0);
        app.handle_key(KeyCode::Char('+'), 1_000);
        assert_eq!(app.trade_amount, 200.0);
        app.handle_key(KeyCode::Char('-'), 1_000);
        assert_eq!(app.trade_amount, 100.0);
        app.handle_key(KeyCode::Char(' '), 1_000);
        assert_eq!(app.trade_amount, 100.0, "max is capped by cargo, not stock");
        app.trade_amount = 800.0;
        let text = render_text(&app, 140, 45);
        assert!(text.contains("need 800 cargo, have 0"), "{text}");
        let detail = app.current_detail().unwrap();
        let silicon = detail
            .deltas
            .iter()
            .find(|d| d.label == "silicon stock")
            .unwrap();
        assert_eq!(
            (silicon.from.as_str(), silicon.to.as_str()),
            ("4596", "3796")
        );
        assert!(
            detail
                .requirements
                .iter()
                .any(|r| !r.met && r.text.contains("cargo"))
        );
    }

    #[test]
    fn market_shows_a_full_store_before_enter_and_refuses_the_trade() {
        let mut state = GameState::new(1_000);
        state.resources.silicon = state.storage_cap(ResourceKind::Silicon);
        state.resources.iron = 1_000.0;
        let mut app = App::new(state);
        app.screen = Screen::Market;
        app.selected = TRADE_PAIRS
            .iter()
            .position(|p| *p == (ResourceKind::Iron, ResourceKind::Silicon))
            .unwrap();
        let text = render_text(&app, 140, 45);
        assert!(text.contains("storage is full"), "{text}");
        let iron = app.state.resources.iron;
        app.handle_key(KeyCode::Enter, 1_000);
        assert_eq!(app.state.resources.iron, iron, "{}", app.status);
        assert!(app.status.contains("storage is full"), "{}", app.status);
    }

    /// A lived-in four-day game for whole-screen text checks.
    fn played_state() -> GameState {
        let mut state = GameState::new_live_galaxy(1_000, BalanceConfig::default(), 7);
        state.skip_tutorial();
        state.buildings.metal_mine = 8;
        state.buildings.crystal_mine = 6;
        state.buildings.deuterium_synthesizer = 2;
        state.buildings.solar_plant = 9;
        state.buildings.research_lab = 1;
        state.catch_up(1_000 + 4 * 86_400);
        state.story_pending = None;
        state
    }

    /// Words a player should never read: snake_case ids, raw unix times and
    /// second counts of a minute or more.
    fn raw_words(text: &str) -> std::collections::BTreeSet<String> {
        text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .filter(|word| {
                let digits = |w: &str| !w.is_empty() && w.chars().all(|c| c.is_ascii_digit());
                let snake = word.contains('_') && word.chars().any(|c| c.is_ascii_lowercase());
                let unix = digits(word.trim_start_matches('t'))
                    && word.trim_start_matches('t').len() == 10;
                let secs = word.len() > 3
                    && word.ends_with('s')
                    && digits(&word[..word.len() - 1])
                    && word[..word.len() - 1].parse::<u64>().is_ok_and(|n| n >= 60);
                snake || unix || secs
            })
            .map(str::to_string)
            .collect()
    }

    /// #295: the bridge timeline mashed history columns into phrases like
    /// "other player request spend" and "build simulation complete".
    #[test]
    fn bridge_timeline_reads_as_sentences() {
        let mut state = GameState::new(0);
        state.next_raid_at = None;
        state.resources = Resources::from_parts(9_000.0, 9_000.0, 1_000.0);
        state.enqueue_build(BuildingKind::MetalMine, 0).unwrap();
        state.catch_up(3_600);
        let mut app = App::new(state);
        app.screen = Screen::Cockpit;
        let text = render_text(&app, 140, 45);
        assert!(text.contains("queued iron mine at 1:1:1"), "{text}");
        assert!(text.contains("iron mine reached lv 2"), "{text}");
        let row = |kind, action: &str, objects: &[&str], outcome: &str| hoshi_core::HistoryEvent {
            kind,
            action: action.into(),
            objects: objects.iter().map(|o| (*o).to_string()).collect(),
            outcome: outcome.into(),
            ..app.state.history.events[0].clone()
        };
        let dismantled = row(
            HistoryKind::Build,
            "dismantle_complete",
            &["metal_mine"],
            "salvage",
        );
        assert_eq!(
            crate::cockpit::timeline_text(&app.state, &dismantled),
            "iron mine dismantled"
        );
        let held = row(
            HistoryKind::Other,
            "request_spend",
            &["spend:12", "industry"],
            "held",
        );
        assert_eq!(
            crate::cockpit::timeline_text(&app.state, &held),
            "industry spending held"
        );
        let assigned = row(
            HistoryKind::Other,
            "assign_leader",
            &["leader:3", "1:1:1"],
            "mission:4",
        );
        assert_eq!(
            crate::cockpit::timeline_text(&app.state, &assigned),
            "assign leader: 1:1:1"
        );
        for token in ["simulation", "other player", "build simulation"] {
            assert!(!text.contains(token), "{token} in {text}");
        }
    }

    #[test]
    fn hub_screens_show_no_raw_ids_or_timestamps() {
        let state = played_state();
        let mut found = Vec::new();
        for screen in [
            Screen::Cockpit,
            Screen::Overview,
            Screen::Planet,
            Screen::Production,
            Screen::Maintenance,
            Screen::Shipyard,
            Screen::Map,
            Screen::Ops,
            Screen::Market,
            Screen::Finance,
            Screen::Kpis,
            Screen::Diplomacy,
            Screen::Laws,
            Screen::Staff,
            Screen::Incidents,
            Screen::Reports,
            Screen::History,
        ] {
            let mut app = App::new(state.clone());
            app.open(screen);
            let panes: &[HistoryPane] = if screen == Screen::History {
                &[HistoryPane::List, HistoryPane::Lineage, HistoryPane::AsOf]
            } else {
                &[HistoryPane::List]
            };
            for &pane in panes {
                app.history_pane = pane;
                for selected in 0..6 {
                    app.selected = selected;
                    let bad = raw_words(&render_text(&app, 140, 45));
                    if !bad.is_empty() {
                        found.push(format!("{screen:?} {pane:?} row {selected}: {bad:?}"));
                    }
                }
            }
        }
        assert!(found.is_empty(), "{}", found.join("\n"));
    }

    #[test]
    fn an_energy_shortfall_is_a_bridge_alert() {
        let mut state = GameState::new(1_000);
        state.buildings.metal_mine = 10;
        state.buildings.solar_plant = 0;
        assert!(state.energy_balance() < 0.0);
        let mut app = App::new(state);
        app.screen = Screen::Cockpit;
        let text = render_text(&app, 140, 45);
        assert!(text.contains("Energy short"), "{text}");
    }

    #[test]
    fn a_lost_raid_reads_as_a_report_from_the_bridge_and_history() {
        let mut state = GameState::new(1_000);
        state.resources = Resources::from_parts(2_000.0, 2_000.0, 0.0);
        state.trigger_raid(1_000);
        let plunder = state.combat_logs.last().unwrap().raid_plunder.unwrap();
        assert!(plunder.iron > 0.0);
        state.catch_up(1_000 + 7_200);
        // A story choice would take the keys; this test is about the raid.
        state.story_pending = None;
        let mut app = App::new(state);
        app.screen = Screen::Cockpit;
        let bridge = render_text(&app, 140, 45);
        assert!(bridge.contains("Raid lost"), "{bridge}");
        assert!(bridge.contains("Home undefended"), "{bridge}");
        app.handle_key(KeyCode::Char('r'), 1_000 + 7_200);
        let report = render_text(&app, 140, 45);
        assert!(report.contains("Combat report"), "{report}");
        assert!(report.contains("2h 00m ago"), "{report}");
        let iron = format!("Fe {}", ui::spans::compact(plunder.iron));
        assert!(report.contains(&iron), "{iron}\n{report}");
        app.replay = None;
        app.screen = Screen::History;
        app.selected = app
            .history_rows()
            .iter()
            .position(|row| row.kind == HistoryKind::Combat)
            .unwrap();
        app.handle_key(KeyCode::Enter, 1_000 + 7_200);
        assert!(app.replay_active(), "Enter on a fight opens its report");
    }

    /// #293: two heavy cannons fought and died, and the report said nothing.
    #[test]
    fn the_combat_report_shows_the_defence_layers() {
        let mut state = GameState::new(1_000);
        state.defences.add(DefenceKind::HeavyCannon, 2);
        state.trigger_raid(1_000);
        let defence = state.combat_logs.last().unwrap().raid_defence.unwrap();
        state.story_pending = None;
        let mut app = App::new(state);
        app.screen = Screen::Cockpit;
        app.handle_key(KeyCode::Char('r'), 1_000);
        let report = render_text(&app, 140, 45);
        let battery = format!("battery  {} → {}", 2, defence.batteries_after);
        assert!(report.contains(&battery), "{battery}\n{report}");
        if defence.rebuilt > 0 {
            assert!(
                report.contains(&format!("rebuilt  {}", defence.rebuilt)),
                "{report}"
            );
        }
    }

    #[test]
    fn a_raid_lost_with_nothing_to_take_still_alerts() {
        let mut state = GameState::new(1_000);
        state.trigger_raid(1_000);
        let log = state.combat_logs.last().unwrap();
        assert_eq!(
            log.raid_plunder,
            Some(Resources::ZERO),
            "stock sits under the floor"
        );
        state.story_pending = None;
        let mut app = App::new(state);
        app.screen = Screen::Cockpit;
        let bridge = render_text(&app, 140, 45);
        assert!(bridge.contains("Raid lost"), "{bridge}");
    }

    #[test]
    fn docked_fighters_count_as_home_defence() {
        let mut state = GameState::new(1_000);
        state.next_raid_at = Some(1_000 + 7 * 3_600);
        state.fleet.add(ShipKind::EspionageProbe, 2);
        let mut app = App::new(state);
        app.screen = Screen::Cockpit;
        let text = render_text(&app, 140, 45);
        assert!(text.contains("2 unarmed ships"), "{text}");
        app.state.fleet.add(ShipKind::LightFighter, 3);
        let text = render_text(&app, 140, 45);
        assert!(!text.contains("Home undefended"), "{text}");
    }

    #[test]
    fn the_header_shows_a_raid_hours_ahead_and_undefended_stays() {
        let mut state = GameState::new(1_000);
        state.next_raid_at = Some(1_000 + 7 * 3_600);
        let mut app = App::new(state);
        app.screen = Screen::Cockpit;
        let text = render_text(&app, 140, 45);
        assert!(text.contains("RAID 7h 00m"), "{text}");
        assert!(text.contains("Home undefended"), "{text}");
        assert!(!text.contains("Incoming raid"), "{text}");
    }

    #[test]
    fn away_overlay_dismisses_and_reopens_from_overview() {
        let mut app = App::new(GameState::new(1_000));
        app.set_away(crate::title::AwaySummary {
            elapsed_secs: 120,
            builds: 1,
            events: vec!["raid: lost".into()],
            ..crate::title::AwaySummary::default()
        });
        let text = render_text(&app, 80, 24);
        assert!(text.contains("While you were away"), "{text}");
        assert!(text.contains("2m 00s"), "{text}");
        assert!(
            !text.contains("Armour Technology"),
            "away overlay left the research list visible:\n{text}"
        );
        app.handle_key(KeyCode::Enter, 1_000);
        let text = render_text(&app, 80, 24);
        assert!(!text.contains("while you were away"), "{text}");
        app.handle_key(KeyCode::Char('w'), 1_000);
        let text = render_text(&app, 80, 24);
        assert!(text.contains("While you were away"), "{text}");
    }

    fn plundered_summary() -> AwaySummary {
        let mut ledger = hoshi_core::CatchUpLedger::default();
        let before = Resources::from_parts(1_000.0, 800.0, 400.0);
        let mut home = hoshi_core::PlanetLedger {
            coord: Coord::HOME,
            before,
            after: before,
            produced: Resources::from_parts(5_630.0, 2_100.0, 12.0),
            capped: Resources::from_parts(0.0, 300.0, 0.0),
            spent: Resources::from_parts(200.0, 0.0, 400.0),
            plundered: Resources::from_parts(2_729.0, 1_000.0, 0.0),
            transfers_in: Resources::ZERO,
            transfers_out: Resources::ZERO,
            gained: Resources::ZERO,
        };
        for kind in ResourceKind::ALL {
            home.after.set(kind, before.get(kind) + home.net(kind));
        }
        ledger.planets.push(home);
        ledger.raids_lost = 1;
        let mut events = vec!["raid: pirates 3 light fighter vs none. winner attacker".to_string()];
        events.extend((1..=5).map(|n| format!("story {n}")));
        AwaySummary {
            elapsed_secs: 10 * 3_600,
            ledger,
            builds: 1,
            events,
            ..AwaySummary::default()
        }
    }

    #[test]
    fn away_dialog_shows_where_resources_went() {
        let mut app = App::new(GameState::new(1_000));
        app.set_away(plundered_summary());
        let text = render_text(&app, 80, 24);
        assert_eq!(
            text,
            include_str!("../tests/fixtures/away/80x24.txt"),
            "{text}"
        );
        assert!(text.contains("Pirates plundered Fe 2.7k Si 1.0k"), "{text}");
        assert!(text.contains("−388"), "{text}");
        assert!(!text.contains("+-"), "{text}");
        assert!(text.contains("raids 1"), "{text}");
        assert!(!text.contains("story 1"), "{text}");
        app.handle_key(KeyCode::Char(' '), 1_000);
        let text = render_text(&app, 80, 24);
        assert!(text.contains("story 1"), "{text}");
        assert!(text.contains("raid: pirates"), "{text}");
        assert!(!text.contains("story 5"), "{text}");
        app.handle_key(KeyCode::Char(' '), 1_000);
        assert!(render_text(&app, 80, 24).contains("story 5"));
    }

    #[test]
    fn running_game_shows_the_away_summary_once_after_a_wake() {
        let mut app = App::new(GameState::new(1_000));
        app.away_on_wake = Some(600);
        app.catch_up_now(1_010);
        app.catch_up_now(1_400);
        assert!(app.away.is_none());
        app.handle_key(KeyCode::Enter, 1_400 + 3_600);
        let away = app.away.as_ref().expect("summary after a wake");
        assert_eq!(away.elapsed_secs, 3_600);
        assert!(!app.away_hidden);
        app.set_notify(NotifyPrefs {
            enabled: true,
            ..NotifyPrefs::default()
        });
        assert!(String::from_utf8_lossy(&app.poll_notify()).contains("while away"));
        assert!(!String::from_utf8_lossy(&app.poll_notify()).contains("while away"));
        app.handle_key(KeyCode::Enter, 5_000);
        app.catch_up_now(5_001);
        app.catch_up_now(5_300);
        assert!(app.away_hidden);
    }

    #[test]
    fn away_overlay_lists_deliveries_stalls_and_completed() {
        let mut app = App::new(GameState::new(1_000));
        app.set_away(AwaySummary {
            elapsed_secs: 60,
            deliveries: vec!["delivered 100 metal to 1:1:2".into()],
            stalls: vec!["stall: energy deficit at 1:1:1".into()],
            completed: vec!["metal mine lv 2".into()],
            ..AwaySummary::default()
        });
        let text = render_text(&app, 80, 24);
        assert!(text.contains("DELIVERIES"), "{text}");
        assert!(text.contains("delivered 100 metal to 1:1:2"), "{text}");
        assert!(text.contains("STALLS"), "{text}");
        assert!(text.contains("stall: energy deficit at 1:1:1"), "{text}");
        assert!(text.contains("COMPLETED"), "{text}");
        assert!(text.contains("metal mine lv 2"), "{text}");
    }

    #[test]
    fn renders_without_panic_from_60x20_upward() {
        let app = App::new(GameState::new(1_000));
        for (w, h) in [(60, 20), (80, 24), (100, 30)] {
            let text = render_text(&app, w, h);
            assert!(text.contains("Fe"), "{w}x{h}: {text}");
            assert!(text.contains("Si"), "{w}x{h}: {text}");
            assert!(text.contains("REE"), "{w}x{h}: {text}");
            assert!(text.contains("Energy"), "{w}x{h}: {text}");
            assert!(text.contains("Research"), "{w}x{h}: {text}");
            assert!(text.contains("research idle"), "{w}x{h}: {text}");
            assert!(
                !text.contains('\u{1b}'),
                "{w}x{h} leaked an escape sequence: {text:?}"
            );
        }
        let mut app = app;
        app.open(Screen::Shipyard);
        let fleet = render_text(&app, 60, 20);
        assert!(fleet.contains("Fleet"), "{fleet}");
        assert!(fleet.contains("RAID"), "{fleet}");
        app.open(Screen::Prestige);
        let records = render_text(&app, 60, 20);
        assert!(records.contains("Legacy"), "{records}");
    }

    #[test]
    fn material_labels_fit_80_120_and_160() {
        let app = App::new(GameState::new(1_000));
        for (w, h) in [(80, 24), (120, 40), (160, 48)] {
            let text = render_text(&app, w, h);
            let needles: &[&str] = if w >= 112 {
                &[
                    "IRON",
                    "SILICON",
                    "RARE EARTHS",
                    "WATER ICE",
                    "HELIUM-3",
                    "DEUTERIUM",
                    "ENERGY",
                    "HEAT",
                    "/h",
                ]
            } else {
                &["Fe", "Si", "REE", "H2O", "He3", "D", "Energy", "heat", "/h"]
            };
            for needle in needles {
                assert!(text.contains(needle), "{w}x{h} missing {needle}: {text}");
            }
            for line in text.lines() {
                let width = line.chars().count();
                assert!(
                    width <= w as usize,
                    "{w}x{h} truncated cost row ({width}): {line}"
                );
            }
        }
    }

    #[test]
    fn keystones_get_their_own_research_group() {
        let mut state = GameState::new(1_000);
        state.buildings.research_lab = 1;
        let app = App::new(state);
        let specs = app.overview_techs();
        assert!(specs.last().is_some_and(|spec| spec.keystone));
        let text = render_text(&app, 160, 48);
        assert!(text.contains("KEYSTONE ★ VICTORY PATH"), "{text}");
    }

    #[test]
    fn tree_view_renders_at_80x24_and_shows_node_states() {
        let mut state = GameState::new(1_000);
        state.buildings.research_lab = 1;
        state.tech_levels.insert("energy".into(), 1);
        state
            .exclusive_picks
            .insert("mining_method".into(), "deep_core".into());
        let mut app = App::new(state);
        let small = render_text(&app, 80, 24);
        assert!(small.contains(" industry "), "{small}");
        assert!(
            small.contains("NEED") || small.contains("READY") || small.contains("EXCLUDED"),
            "{small}"
        );
        let text = render_text(&app, 120, 40);
        assert!(text.contains(" industry "), "{text}");
        assert!(text.contains("military"), "{text}");
        assert!(text.contains("society"), "{text}");
        assert!(text.contains("EXCLUDED"), "{text}");
        assert!(text.contains("★ Dyson ring"), "{text}");
        // Full names next to the wider status column at the target size.
        let wide = render_text(&app, 140, 45);
        assert!(wide.contains("Energy Technology 1"), "{wide}");
        assert!(!text.contains("ancient"), "{text}");
        app.handle_key(KeyCode::Char('l'), 1_000);
        assert_eq!(app.tree_branch, TechBranch::Military);
        let military = render_text(&app, 120, 40);
        assert!(military.contains("military tier"), "{military}");
        app.handle_key(KeyCode::Char('l'), 1_000);
        app.handle_key(KeyCode::Char('l'), 1_000);
        assert_eq!(app.tree_branch, TechBranch::Industry);
    }

    #[test]
    fn tree_view_queues_research_from_the_keyboard() {
        let mut state = GameState::new(1_000);
        state.buildings.research_lab = 1;
        state.resources.iron = 10_000.0;
        state.resources.silicon = 10_000.0;
        state.resources.deuterium = 10_000.0;
        let mut app = App::new(state);
        let energy = app
            .overview_techs()
            .iter()
            .position(|spec| spec.id == "energy")
            .unwrap();
        app.selected = energy;
        app.handle_key(KeyCode::Enter, 1_000);
        assert_eq!(
            app.state.research.as_ref().map(|j| j.tech.as_str()),
            Some("energy")
        );
        let text = render_text(&app, 120, 40);
        assert!(text.contains("IN PROGRESS"), "{text}");
    }

    #[test]
    fn disabled_portraits_use_ascii_and_keep_layout() {
        fn portrait_row(app: &App) -> String {
            let slot = app.current_portrait_slot();
            let mood = portraits::mood_for(slot, app.state.visible_events(Actor::Player));
            portraits::portrait_lines_for(app.portraits, slot, mood, None)
                .iter()
                .map(|line| line.to_string().trim().to_string())
                .find(|row| row.len() > 2)
                .unwrap()
        }
        let mut app = App::new(GameState::new(1_000));
        app.portraits.enabled = false;
        app.open(Screen::Staff);
        let staff = render_text(&app, 80, 24);
        assert!(staff.contains(&portrait_row(&app)), "{staff}");
        assert!(!staff.contains('▀'), "{staff}");
        app.state.events.push(hoshi_core::GameEvent {
            at: 1,
            text: "raid: pirates 1 vs 0. 1 rounds. lost none. pirates lost 1. plunder Fe0 Si0 D0"
                .into(),
        });
        let raid = render_text(&app, 80, 24);
        assert!(raid.contains(&portrait_row(&app)), "{raid}");
        app.open(Screen::Planet);
        let planet = render_text(&app, 80, 24);
        assert!(!planet.contains(&portrait_row(&app)), "{planet}");
    }

    #[test]
    fn overview_shows_raid_countdown() {
        let app = App::new(GameState::new(1_000));
        let text = render_text(&app, 80, 24);
        assert!(text.contains("RAID 1:00:00"), "{text}");
    }

    #[test]
    fn overview_shows_shareable_seed_and_y_copies_it() {
        let mut state = GameState::new(1_000);
        state.galaxy_seed = 42;
        let mut app = App::new(state);
        let code = encode_galaxy_seed(42);
        let text = render_text(&app, 80, 24);
        assert!(text.contains(&code), "{text}");
        assert!(text.contains("gen 1"), "{text}");
        app.handle_key(KeyCode::Char('y'), 1_000);
        let text = render_text(&app, 80, 24);
        assert!(text.contains(&format!("copied {code}")), "{text}");
    }

    #[test]
    fn combat_replay_opens_from_overview_and_away_and_respects_reduced_motion() {
        let mut state = GameState::new(1_000);
        state.fleet.add(ShipKind::LightFighter, 6);
        state.trigger_raid(1_000);
        let mut app = App::new(state);
        app.handle_key(KeyCode::Char('r'), 1_000);
        assert!(app.replay_active());
        let text = render_text(&app, 80, 24);
        assert!(text.contains("Combat report"), "{text}");
        assert!(text.contains("PIRATES"), "{text}");
        assert!(text.contains("YOU"), "{text}");
        app.handle_key(KeyCode::Char('n'), 1_000);
        let text = render_text(&app, 80, 24);
        assert!(text.contains("round"), "{text}");
        app.handle_key(KeyCode::Char('x'), 1_000);
        assert!(!app.replay_active());

        app.reduced_motion = true;
        app.handle_key(KeyCode::Char('r'), 1_000);
        let replay = app.replay.as_ref().expect("replay");
        assert_eq!(
            replay.frame,
            replay.log.report.frames.len().saturating_sub(1)
        );
        assert!(replay.paused);

        app.replay = None;
        app.set_away(AwaySummary {
            elapsed_secs: 10,
            events: vec!["raid: replay".into()],
            ..AwaySummary::default()
        });
        app.handle_key(KeyCode::Char('r'), 1_000);
        assert!(app.replay_active());
        assert!(app.away_hidden);
    }

    #[test]
    fn replay_commentary_is_off_until_enabled() {
        let mut state = GameState::new(1_000);
        state.fleet.add(ShipKind::LightFighter, 6);
        state.trigger_raid(1_000);
        let mut app = App::new(state);
        app.handle_key(KeyCode::Char('r'), 1_000);
        assert!(
            app.replay
                .as_ref()
                .unwrap()
                .commentary
                .iter()
                .all(Option::is_none)
                || app.replay.as_ref().unwrap().commentary.is_empty()
        );
        app.replay = None;
        app.commentary = true;
        app.handle_key(KeyCode::Char('r'), 1_000);
        let (last, line) = {
            let replay = app.replay.as_ref().unwrap();
            assert!(replay.commentary.iter().any(Option::is_some));
            let last = replay.commentary.len().saturating_sub(1);
            let line = replay.commentary[last].clone().expect("closing line");
            (last, line)
        };
        app.replay.as_mut().unwrap().frame = last;
        let text = render_text(&app, 80, 24);
        assert!(text.contains("Combat report"), "{text}");
        assert!(text.contains(&line), "{text}");
    }

    #[test]
    fn overview_shows_combat_losses_on_both_sides() {
        let mut state = GameState::new(1_000);
        state.trigger_raid(1_000);
        let app = App::new(state);
        let text = render_text(&app, 100, 28);
        assert!(text.contains("lost"), "{text}");
        assert!(text.contains("pirates lost"), "{text}");
    }

    #[test]
    fn start_build_and_research_from_keys_and_countdowns_tick() {
        let clock = FakeClock::new(1_000);
        let mut state = GameState::new(clock.now_unix());
        state.resources.iron = 10_000.0;
        state.resources.silicon = 10_000.0;
        state.resources.deuterium = 10_000.0;
        state.buildings.research_lab = 1;
        let mut app = App::new(state);

        app.open(Screen::Planet);
        assert_eq!(
            app.handle_key(KeyCode::Enter, clock.now_unix()),
            Action::None
        );
        assert_eq!(app.state.build_queue.len(), 1);
        let build_left = app.state.build_queue[0]
            .completes_at
            .unwrap()
            .saturating_sub(app.state.last_tick_unix);
        assert!(build_left > 0);

        clock.set(1_010);
        app.state.catch_up(clock.now_unix());
        let build_left_later = app.state.build_queue[0]
            .completes_at
            .unwrap()
            .saturating_sub(app.state.last_tick_unix);
        assert!(
            build_left_later < build_left,
            "build countdown should tick: {build_left} -> {build_left_later}"
        );
        let text = render_text(&app, 80, 24);
        assert!(
            text.contains(&ui::spans::duration(build_left_later)),
            "{text}"
        );

        app.open(Screen::Overview);
        assert_eq!(app.screen, Screen::Overview);
        let energy = app
            .overview_techs()
            .iter()
            .position(|spec| spec.id == "energy")
            .unwrap();
        app.selected = energy;
        app.handle_key(KeyCode::Enter, clock.now_unix());
        let job = app.state.research.as_ref().expect("research started");
        let research_left = job.completes_at.saturating_sub(app.state.last_tick_unix);
        assert!(research_left > 0);

        clock.set(1_020);
        app.state.catch_up(clock.now_unix());
        let research_left_later = app
            .state
            .research
            .as_ref()
            .unwrap()
            .completes_at
            .saturating_sub(app.state.last_tick_unix);
        assert!(
            research_left_later < research_left,
            "research countdown should tick: {research_left} -> {research_left_later}"
        );
        let text = render_text(&app, 80, 24);
        assert!(text.contains("research energy"), "{text}");
        assert!(
            text.contains(&ui::spans::duration(research_left_later)),
            "{text}"
        );
    }

    #[test]
    fn unaffordable_items_are_dimmed_with_missing_amount() {
        let app = App::new(GameState::new(1_000));
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();
        let text = buffer_text(terminal.backend());
        assert!(
            text.contains("NEED"),
            "overview should show missing: {text}"
        );
        assert!(
            line_is_dim(terminal.backend(), "NEED"),
            "unaffordable research should be dim: {text}"
        );

        let mut app = App::new(GameState::new(1_000));
        app.open(Screen::Planet);
        app.selected = BuildingKind::ALL
            .iter()
            .position(|&kind| kind == BuildingKind::ResearchLab)
            .expect("research lab");
        let backend = TestBackend::new(80, 32);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();
        let text = buffer_text(terminal.backend());
        assert!(
            text.contains("SHORT D") || text.contains("SHORT FE"),
            "planet should show missing resources: {text}"
        );
        assert!(
            line_is_dim(terminal.backend(), "Research lab"),
            "unaffordable building should be dim: {text}"
        );
    }

    #[test]
    fn shipyard_queues_a_batch_in_one_slot() {
        let mut state = GameState::new(1_000);
        state.buildings.shipyard = 1;
        state.resources = Resources::from_parts(100_000.0, 100_000.0, 100_000.0);
        let mut app = App::new(state);
        app.open(Screen::Shipyard);
        app.selected = ShipKind::ALL.len()
            + DefenceKind::ALL
                .iter()
                .position(|k| *k == DefenceKind::LightCannon)
                .unwrap();
        for _ in 0..5 {
            app.handle_key(KeyCode::Char('+'), 1_000);
        }
        let unit = app.state.defence_time_secs(DefenceKind::LightCannon);
        let text = render_text(&app, 140, 45);
        assert!(text.contains("×6"), "{text}");
        assert!(text.contains(&ui::spans::duration(unit * 6)), "{text}");
        let before = app.state.resources;
        app.handle_key(KeyCode::Enter, 1_000);
        assert_eq!(app.state.ship_queue.len(), 1, "{}", app.status);
        assert_eq!(app.state.ship_queue[0].remaining, 6);
        let paid = before.iron - app.state.resources.iron;
        let six = app.state.defence_cost(DefenceKind::LightCannon, 6).iron;
        assert!((paid - six).abs() < 1e-6, "paid {paid}, six cost {six}");

        // Space picks the largest batch the stockpile pays for.
        app.state.resources = app.state.defence_cost(DefenceKind::LightCannon, 3);
        app.handle_key(KeyCode::Char(' '), 1_000);
        assert_eq!(app.ship_batch, 3);
        app.handle_key(KeyCode::Char('-'), 1_000);
        assert_eq!(app.ship_batch, 2);
    }

    #[test]
    fn shipyard_lists_ships_requirements_and_queue() {
        let mut state = GameState::new(1_000);
        state.buildings.shipyard = 1;
        state.tech_levels.insert("combustion_drive".into(), 1);
        state.resources.iron = 100_000.0;
        state.resources.silicon = 100_000.0;
        state.resources.deuterium = 100_000.0;
        state.fleet.add(ShipKind::LightFighter, 4);
        state.fleet.add(ShipKind::HeavyFighter, 2);
        state.fleet.add(ShipKind::SmallCargo, 1);
        let mut app = App::new(state);
        app.open(Screen::Shipyard);
        let text = render_text(&app, 100, 32);
        assert!(text.contains("Fleet / Shipyard"), "{text}");
        assert!(text.contains("Light fighter"), "{text}");
        assert!(text.contains("Battleship"), "{text}");
        assert!(text.contains("Light fighter        4"), "{text}");
        assert!(
            text.contains("NEED SHIPYARD") || text.contains("NEED "),
            "{text}"
        );

        app.selected = ShipKind::ALL
            .iter()
            .position(|k| *k == ShipKind::LightFighter)
            .unwrap();
        app.handle_key(KeyCode::Enter, 1_000);
        assert_eq!(app.state.ship_queue.len(), 1);
        assert_eq!(app.state.ship_queue[0].ship, Some(ShipKind::LightFighter));
        let text = render_text(&app, 100, 32);
        assert!(text.contains("1. light fighter ×1"), "{text}");
        assert!(text.contains("QUEUE 1/5"), "{text}");
    }

    #[test]
    fn unaffordable_ships_are_dimmed_with_missing_or_shipyard() {
        let mut app = App::new(GameState::new(1_000));
        app.open(Screen::Shipyard);
        app.selected = ShipKind::ALL
            .iter()
            .position(|k| *k == ShipKind::LightFighter)
            .unwrap();
        let backend = TestBackend::new(100, 32);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();
        let text = buffer_text(terminal.backend());
        assert!(
            text.contains("NEED SHIPYARD"),
            "shipyard should show shipyard requirement: {text}"
        );
        assert!(
            line_is_dim(terminal.backend(), "Light fighter"),
            "locked ships should be dim: {text}"
        );

        let mut state = GameState::new(1_000);
        state.buildings.shipyard = 1;
        state.tech_levels.insert("combustion_drive".into(), 1);
        state.resources = hoshi_core::Resources::ZERO;
        let mut app = App::new(state);
        app.open(Screen::Shipyard);
        app.selected = ShipKind::ALL
            .iter()
            .position(|k| *k == ShipKind::LightFighter)
            .unwrap();
        let backend = TestBackend::new(100, 32);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();
        let text = buffer_text(terminal.backend());
        assert!(
            text.contains("SHORT FE") || text.contains("SHORT SI"),
            "unaffordable ships should show missing amounts: {text}"
        );
        assert!(
            line_is_dim(terminal.backend(), "Light fighter"),
            "unaffordable ships should be dim: {text}"
        );
    }

    #[test]
    fn shipyard_x_cancels_the_ship_queue() {
        let mut state = GameState::new(1_000);
        state.buildings.shipyard = 1;
        state.tech_levels.insert("combustion_drive".into(), 1);
        state.resources.iron = 100_000.0;
        state.resources.silicon = 100_000.0;
        state.resources.deuterium = 100_000.0;
        let mut app = App::new(state);
        app.open(Screen::Planet);
        assert_eq!(app.screen, Screen::Planet);
        app.handle_key(KeyCode::Enter, 1_000);
        assert_eq!(app.state.build_queue.len(), 1);
        app.open(Screen::Shipyard);
        assert_eq!(app.screen, Screen::Shipyard);
        app.selected = ShipKind::ALL
            .iter()
            .position(|k| *k == ShipKind::LightFighter)
            .unwrap();
        app.handle_key(KeyCode::Enter, 1_000);
        assert_eq!(app.state.ship_queue.len(), 1);

        app.handle_key(KeyCode::Char('x'), 1_000);
        assert!(app.state.ship_queue.is_empty());
        assert_eq!(app.state.build_queue.len(), 1);

        app.open(Screen::Planet);
        assert_eq!(app.screen, Screen::Planet);
        app.handle_key(KeyCode::Char('x'), 1_000);
        assert!(app.state.build_queue.is_empty());
    }

    #[test]
    fn shipyard_lists_defences_and_queues_a_cannon() {
        let mut state = GameState::new(1_000);
        state.buildings.shipyard = 1;
        state.resources.iron = 100_000.0;
        state.resources.silicon = 100_000.0;
        state.resources.deuterium = 100_000.0;
        let mut app = App::new(state);
        app.open(Screen::Shipyard);
        let text = render_text(&app, 100, 40);
        assert!(text.contains("Light cannon"), "{text}");
        assert!(text.contains("Orbital station"), "{text}");
        app.selected = ShipKind::ALL.len()
            + DefenceKind::ALL
                .iter()
                .position(|k| *k == DefenceKind::LightCannon)
                .unwrap();
        app.handle_key(KeyCode::Enter, 1_000);
        assert_eq!(app.state.ship_queue.len(), 1);
        assert_eq!(
            app.state.ship_queue[0].defence,
            Some(DefenceKind::LightCannon)
        );
        let text = render_text(&app, 100, 32);
        assert!(text.contains("1. light cannon ×1"), "{text}");
    }

    #[test]
    fn quit_saves_the_game() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "hoshi-tui-quit-{}-{nanos}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let mut conn = open_save(&path).unwrap();
        let mut app = App::new(GameState::new(50));
        app.open(Screen::Planet);
        app.handle_key(KeyCode::Enter, 50);
        assert_eq!(app.handle_key(KeyCode::Char('q'), 50), Action::Quit);
        save_state(&mut conn, &app.state).unwrap();
        drop(conn);
        let conn = open_save(&path).unwrap();
        let loaded = load_state(&conn, BalanceConfig::default()).unwrap();
        assert_eq!(loaded.build_queue.len(), 1);
        assert_eq!(loaded.build_queue[0].building, BuildingKind::MetalMine);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    }

    #[test]
    fn planet_switcher_shows_glance_and_builds_on_the_selected_world() {
        let dest = Coord {
            galaxy: 1,
            system: 1,
            position: 2,
        };
        let mut state = GameState::new(1_000);
        state.resources.iron = 10_000.0;
        state.resources.silicon = 10_000.0;
        state.resources.deuterium = 10_000.0;
        state.colonies.push(Colony {
            coord: dest,
            kind: PlanetType::Rocky,
            size: 120,
            resources: Resources::from_parts(10_000.0, 10_000.0, 10_000.0),
            buildings: Buildings::default(),
            fleet: Fleet::default(),
            defences: hoshi_core::DefenceGrid::default(),
            build_queue: Vec::new(),
            rare: None,
            goods: std::collections::BTreeMap::new(),
        });
        let mut app = App::new(state);
        let text = render_text(&app, 100, 32);
        assert!(text.contains("1:1:1"), "{text}");
        assert!(text.contains("10k"), "{text}");

        app.handle_key(KeyCode::Char(']'), 1_000);
        assert_eq!(app.selected_coord(), dest);
        let text = render_text(&app, 100, 32);
        assert!(text.contains("Colony 1:1:2"), "{text}");
        app.open(Screen::Planet);
        assert_eq!(app.screen, Screen::Planet);
        app.handle_key(KeyCode::Enter, 1_000);
        assert!(app.state.build_queue.is_empty());
        assert_eq!(app.state.colonies[0].build_queue.len(), 1);
        let text = render_text(&app, 100, 32);
        assert!(text.contains("Planet 1:1:2"), "{text}");
        assert!(text.contains("iron mine"), "{text}");
        app.handle_key(KeyCode::Char('['), 1_000);
        assert_eq!(app.selected_coord(), Coord::HOME);
    }

    #[test]
    fn planet_screen_shows_rare_bonus_from_config() {
        let dest = Coord {
            galaxy: 1,
            system: 1,
            position: 2,
        };
        let mut state = GameState::new(1_000);
        state.colonies.push(Colony {
            coord: dest,
            kind: PlanetType::Rocky,
            size: 140,
            resources: Resources::ZERO,
            buildings: Buildings::default(),
            fleet: Fleet::default(),
            defences: hoshi_core::DefenceGrid::default(),
            build_queue: Vec::new(),
            rare: Some("rich_vein".into()),
            goods: std::collections::BTreeMap::new(),
        });
        let mut app = App::new(state);
        app.handle_key(KeyCode::Char(']'), 1_000);
        app.open(Screen::Planet);
        assert_eq!(app.screen, Screen::Planet);
        let text = render_text(&app, 100, 32);
        assert!(text.contains("Planet 1:1:2"), "{text}");
        assert!(text.contains("Rare: Rich vein"), "{text}");
        assert!(text.contains("iron +25%"), "{text}");
    }

    #[test]
    fn planet_screen_shows_deposits_from_type_and_seed() {
        let state = GameState::new(1_000);
        let mut app = App::new(state);
        app.open(Screen::Planet);
        assert_eq!(app.screen, Screen::Planet);
        let text = render_text(&app, 100, 32);
        assert!(text.contains("Deposits"), "{text}");
        assert!(text.contains("iron ore"), "{text}");
        assert!(text.contains("Heat"), "{text}");
        assert!(text.contains("Pollution"), "{text}");
    }

    #[test]
    fn planet_screen_warns_before_waste_overflow() {
        let mut state = GameState::new(1_000);
        state.buildings.metal_mine = 18;
        state.buildings.crystal_mine = 18;
        state.buildings.deuterium_synthesizer = 12;
        state.buildings.solar_plant = 24;
        let waste = state.waste_home();
        assert!(waste.overflowing(), "ratio {}", waste.peak_ratio());
        let mut app = App::new(state);
        let overview = render_text(&app, 100, 32);
        assert!(overview.contains("unrest"), "{overview}");
        app.open(Screen::Planet);
        assert_eq!(app.screen, Screen::Planet);
        let planet = render_text(&app, 100, 32);
        assert!(planet.contains("unrest"), "{planet}");
        assert!(planet.contains("Radiator"), "{planet}");
        assert!(planet.contains("Scrubber"), "{planet}");
    }

    #[test]
    fn wonder_screen_shows_remaining_and_eta_on_small_terminals() {
        let mut state = GameState::new(1_000);
        state.tech_levels.insert("energy".into(), 8);
        state.buildings.metal_mine = 18;
        state.buildings.crystal_mine = 16;
        state.buildings.deuterium_synthesizer = 12;
        state.buildings.solar_plant = 28;
        state.buildings.radiator = 6;
        state.buildings.scrubber = 3;
        state.buildings.metal_storage = 10;
        state.buildings.crystal_storage = 10;
        state.buildings.deuterium_storage = 10;
        state.resources.iron = 1_000.0;
        state.resources.silicon = 500.0;
        state.resources.deuterium = 2_000.0;
        let mut app = App::new(state);
        app.open(Screen::Wonder);
        assert_eq!(app.screen, Screen::Wonder);
        let _ = render_text(&app, 40, 12);
        app.selected = 0;
        app.handle_key(KeyCode::Enter, 1_000);
        let remaining = app.state.wonder_remaining().expect("stage remaining");
        let eta = app.state.wonder_eta_secs().expect("eta");
        let text = render_text(&app, 100, 32);
        assert!(text.contains("Colony / Wonder"), "{text}");
        assert!(text.contains("Dyson ring"), "{text}");
        assert!(
            text.contains(&format!("short Fe {}", ui::spans::compact(remaining.iron))),
            "{text}"
        );
        assert!(
            text.contains(&format!("eta {}", ui::spans::duration(eta))),
            "{text}"
        );
        assert!(text.contains("stage 1/"), "{text}");
    }

    #[test]
    fn prestige_screen_shows_kept_and_lost_before_reset() {
        let mut state = GameState::new(1_000);
        state.completed_wonders.push("observatory".into());
        let mut app = App::new(state);
        app.open(Screen::Prestige);
        assert_eq!(app.screen, Screen::Prestige);
        let text = render_text(&app, 100, 32);
        assert!(text.contains("Records / Legacy"), "{text}");
        assert!(text.contains("Reset"), "{text}");
        app.handle_key(KeyCode::Enter, 1_000);
        assert!(app.confirming_prestige);
        let text = render_text(&app, 100, 32);
        assert!(text.contains("Reset keeps:"), "{text}");
        assert!(text.contains("prestige currency"), "{text}");
        assert!(text.contains("achievements"), "{text}");
        assert!(text.contains("Reset loses:"), "{text}");
        assert!(text.contains("resources"), "{text}");
        app.handle_key(KeyCode::Enter, 1_000);
        assert_eq!(app.state.prestige.current.index, 2);
        assert!(app.state.completed_wonders.is_empty());
    }

    #[test]
    fn achievements_screen_lists_progress_hides_locked_and_toasts() {
        let mut state = GameState::new(1_000);
        state.buildings.metal_mine = 2;
        state.config.chains = hoshi_core::ChainConfig::default();
        let mut app = App::new(state);
        for _ in 0..4 {
            app.handle_key(KeyCode::Char('1'), 1_001);
        }
        assert_eq!(app.screen, Screen::Achievements);
        let text = render_text(&app, 100, 32);
        assert!(text.contains("Bridge / Feats"), "{text}");
        assert!(text.contains("Iron cut"), "{text}");
        assert!(text.contains("EARNED"), "{text}");
        assert!(text.contains("Hidden feat"), "{text}");
        assert!(
            text.contains("1/5") || text.contains("Solar array"),
            "{text}"
        );
        assert!(text.contains("achievement: Iron cut"), "{text}");
    }

    #[test]
    fn daily_screen_lists_progress_and_reset() {
        let mut app = App::new(GameState::new(1_000));
        app.open(Screen::Daily);
        assert_eq!(app.screen, Screen::Daily);
        let text = render_text(&app, 100, 32);
        assert!(text.contains("Bridge / Daily"), "{text}");
        assert!(text.contains("STREAK"), "{text}");
        assert!(text.contains("RESETS IN"), "{text}");
        assert!(
            app.state.daily.slots.iter().any(|s| text.contains(
                app.state
                    .config
                    .daily
                    .get(&s.spec_id)
                    .map(|spec| spec.name.as_str())
                    .unwrap_or(s.spec_id.as_str())
            )),
            "{text}"
        );
    }

    #[test]
    fn goals_screen_lists_three_paths() {
        let mut app = App::new(GameState::new(1_000));
        app.open(Screen::Goals);
        assert_eq!(app.screen, Screen::Goals);
        let text = render_text(&app, 100, 32);
        assert!(text.contains("Bridge / Goals"), "{text}");
        assert!(text.contains("Conquest"), "{text}");
        assert!(text.contains("Wonder"), "{text}");
        assert!(text.contains("Diplomacy"), "{text}");
        assert!(text.contains("PIRATE HOMEWORLD"), "{text}");
        assert!(text.contains("LADDER"), "{text}");
        assert!(
            text.contains("Self-sufficient volatiles") || text.contains("Next"),
            "{text}"
        );
    }

    #[test]
    fn automation_screen_lists_tiers_and_dry_runs() {
        let mut state = GameState::new(1_000);
        state.tech_levels.insert("logistics_rules".into(), 1);
        state
            .add_automation_rule(
                hoshi_core::RuleCondition {
                    kind: hoshi_core::RuleConditionKind::StockBelow,
                    planet: state.home,
                    resource: ResourceKind::Iron,
                    amount: 2_000.0,
                },
                hoshi_core::RuleAction::QueueBuild {
                    planet: state.home,
                    building: BuildingKind::MetalMine,
                },
            )
            .unwrap();
        let mut app = App::new(state);
        app.open(Screen::Automation);
        assert_eq!(app.screen, Screen::Automation);
        let text = render_text(&app, 120, 32);
        assert!(text.contains("Empire / Automation"), "{text}");
        assert!(text.contains("Adaptive routes"), "{text}");
        assert!(text.contains("#1 "), "{text}");
        assert!(text.contains("stock_below"), "{text}");
        app.handle_key(KeyCode::Enter, 1_000);
        assert!(
            app.status.contains("rule #1") || app.status.contains("queue"),
            "{}",
            app.status
        );
        for _ in 0..3 {
            app.handle_key(KeyCode::Down, 1_000);
        }
        app.handle_key(KeyCode::Char(' '), 1_000);
        assert!(app.status.contains("off"), "{}", app.status);
    }

    #[test]
    fn finance_screen_lists_treasury_and_approves_held_spend() {
        let mut state = GameState::new(1_000);
        state.resources.iron = 50_000.0;
        state.resources.silicon = 50_000.0;
        state.resources.deuterium = 50_000.0;
        state
            .set_budget(
                hoshi_core::CostCenter::Department(hoshi_core::Department::Industry),
                Some(1.0),
                hoshi_core::Actor::Player,
                1_000,
            )
            .unwrap();
        let err = state
            .enqueue_build(BuildingKind::MetalMine, 1_000)
            .unwrap_err();
        assert!(matches!(err, hoshi_core::BuildError::NeedsApproval { .. }));
        let mut app = App::new(state);
        app.open(Screen::Finance);
        assert_eq!(app.screen, Screen::Finance);
        let text = render_text(&app, 120, 32);
        assert!(text.contains("Trade / Finance"), "{text}");
        assert!(text.contains("CASH"), "{text}");
        assert!(text.contains("Industry"), "{text}");
        assert!(text.contains("HELD"), "{text}");
        app.handle_key(KeyCode::Enter, 1_000);
        assert!(app.status.contains("approved"), "{}", app.status);
        assert_eq!(app.state.build_queue.len(), 1);
    }

    #[test]
    fn kpi_screen_lists_formulas_and_drills_to_events() {
        let mut state = GameState::new(1_000);
        state
            .set_kpi_target(
                hoshi_core::KpiId::Output,
                hoshi_core::TargetOwner::Empire,
                1_000.0,
                hoshi_core::Actor::Player,
                1_000,
            )
            .unwrap();
        let mut app = App::new(state);
        app.open(Screen::Kpis);
        assert_eq!(app.screen, Screen::Kpis);
        let text = render_text(&app, 120, 32);
        assert!(text.contains("KPIs"), "{text}");
        assert!(text.contains("Output"), "{text}");
        app.handle_key(KeyCode::Enter, 1_000);
        assert!(
            app.status.contains("events") || app.status.contains("no source"),
            "{}",
            app.status
        );
    }

    #[test]
    fn maintenance_screen_lists_assets_and_schedules() {
        let mut state = GameState::new(1_000);
        state.resources.iron = 1_000.0;
        state.resources.silicon = 1_000.0;
        let mut app = App::new(state);
        app.open(Screen::Maintenance);
        assert_eq!(app.screen, Screen::Maintenance);
        let text = render_text(&app, 120, 32);
        assert!(text.contains("Maintenance"), "{text}");
        assert!(text.contains("COND"), "{text}");
        assert!(text.contains("ONLINE"), "{text}");
        app.handle_key(KeyCode::Enter, 1_000);
        assert!(app.status.contains("work order"), "{}", app.status);
    }

    #[test]
    fn incident_screen_lists_open_rows_and_resolves() {
        let mut state = GameState::new(1_000);
        state
            .open_incident(
                hoshi_core::IncidentKind::Manual,
                hoshi_core::Severity::High,
                vec!["home".into()],
                hoshi_core::Actor::Player,
                1_000,
            )
            .unwrap();
        let mut app = App::new(state);
        app.open(Screen::Incidents);
        assert_eq!(app.screen, Screen::Incidents);
        let text = render_text(&app, 120, 32);
        assert!(text.contains("Incidents"), "{text}");
        assert!(text.contains("Manual"), "{text}");
        assert!(text.contains("objects home"), "{text}");
        assert!(text.contains("pirate activity"), "{text}");
        app.handle_key(KeyCode::Enter, 1_000);
        assert!(app.status.contains("resolved"), "{}", app.status);
        assert!(app.state.incident_snapshot().open.is_empty());
    }

    /// #299: a lost raid opened and closed its incident in one tick, and
    /// the screen showed nothing behind its counters.
    #[test]
    fn incidents_keep_a_recent_raid_and_explain_reviews() {
        let mut state = GameState::new(1_000);
        state.resources = Resources::from_parts(2_000.0, 2_000.0, 0.0);
        state.trigger_raid(1_000);
        let mut app = App::new(state);
        app.open(Screen::Incidents);
        let text = render_text(&app, 140, 45);
        assert!(text.contains("RECENTLY RESOLVED"), "{text}");
        assert!(text.contains("#1 Raid"), "{text}");
        assert!(text.contains("pirate activity"), "{text}");
        assert!(text.contains("hoshi incidents review"), "{text}");
        let small = render_text(&app, 80, 24);
        assert!(small.contains("#1 Raid"), "{small}");
        assert!(small.contains("hoshi incidents review ID"), "{small}");
        assert!(small.contains("pirate activity"), "{small}");
    }

    /// #300: a recipe whose output store is full must say so on the
    /// production screen, with its real (zero) rate and a stalled count.
    #[test]
    fn a_full_output_store_shows_as_a_stalled_chain() {
        let mut state = GameState::new_live_galaxy(0, BalanceConfig::default(), 7);
        state.next_raid_at = None;
        state.catch_up(3_600);
        state.story_pending = None;
        state.resources.iron = state.storage_cap(ResourceKind::Iron);
        state.resources.silicon = state.storage_cap(ResourceKind::Silicon);
        let mut app = App::new(state);
        app.open(Screen::Production);
        let text = render_text(&app, 140, 45);
        let frames = text
            .lines()
            .find(|line| line.contains("Forge frames"))
            .unwrap_or_default();
        assert!(
            frames.contains("frames 0.0") && frames.contains("BUFFER"),
            "{frames}"
        );
        assert!(text.contains("2 stalled"), "{text}");
    }

    #[test]
    fn victory_overlay_enter_keeps_playing() {
        let mut app = App::new(GameState::new(1_000));
        app.state.victory = Some(VictoryRecord {
            path: VictoryPath::Wonder,
            at: 1_000,
            prestige: 26,
            elapsed_secs: 1_209_600,
            kept_playing: false,
        });
        let text = render_text(&app, 80, 24);
        assert!(text.contains("Victory"), "{text}");
        assert!(text.contains("wonder"), "{text}");
        assert!(text_is_dimmed(&app, 80, 24, "Colony / Research"), "{text}");
        app.handle_key(KeyCode::Enter, 1_000);
        assert!(app.state.victory.as_ref().unwrap().kept_playing);
        let after = render_text(&app, 80, 24);
        assert!(after.contains("Colony / Research"), "{after}");
    }

    #[test]
    fn victory_overlay_enter_works_from_daily() {
        let mut app = App::new(GameState::new(1_000));
        app.open(Screen::Daily);
        assert_eq!(app.screen, Screen::Daily);
        app.state.victory = Some(VictoryRecord {
            path: VictoryPath::Conquest,
            at: 1_000,
            prestige: 28,
            elapsed_secs: 1_209_600,
            kept_playing: false,
        });
        app.handle_key(KeyCode::Enter, 1_000);
        assert!(app.state.victory.as_ref().unwrap().kept_playing);
    }

    #[test]
    fn production_history_samples_on_idle_ticks() {
        let mut app = App::new(GameState::new(1_000));
        app.catch_up_now(1_000);
        app.catch_up_now(2_000);
        app.catch_up_now(3_000);
        assert_eq!(app.prod_history.len(), 3);
        app.catch_up_now(4_000);
        assert_eq!(app.prod_history.len(), 4);
    }

    #[test]
    fn production_screen_shows_limit_energy_heat_and_fits_80x24() {
        let mut app = App::new(GameState::new(1_000));
        app.open(Screen::Production);
        assert_eq!(app.screen, Screen::Production);
        let text = render_text(&app, 80, 24);
        assert!(text.contains("Colony / Production"), "{text}");
        assert!(text.contains("LIMIT"), "{text}");
        assert!(text.contains("ENERGY"), "{text}");
        assert!(text.contains("HEAT"), "{text}");
        assert!(text.contains("REE"), "{text}");
        assert!(text.contains("H2O"), "{text}");
        assert!(text.contains("He3"), "{text}");
        let wide = render_text(&app, 120, 32);
        assert!(wide.contains("LIMIT"), "{wide}");
        assert!(
            text.chars().any(|c| "▁▂▃▄▅▆▇█".contains(c)) || text.contains("Fe /h"),
            "history missing: {text}"
        );
    }

    #[test]
    fn story_overlay_shows_text_and_number_keys_choose() {
        let mut app = App::new(GameState::new(1_000));
        app.state.story_pending = Some(hoshi_core::PendingStory {
            event_id: "dust_cloud".into(),
            offered_at: 1_000,
            timeout_at: 10_000,
        });
        let text = render_text(&app, 80, 24);
        assert!(text.contains("Dust cloud"), "{text}");
        assert!(text.contains("Burn through"), "{text}");
        assert!(text.contains("Wait it out"), "{text}");
        assert!(
            text_is_dimmed(&app, 80, 24, "Colony / Research"),
            "overlay left the screen chrome undimmed:\n{text}"
        );
        assert!(
            !text.contains("Armour Technology")
                || text_is_dimmed(&app, 80, 24, "Armour Technology"),
            "overlay left the research list undimmed:\n{text}"
        );
        assert!(
            text_is_dimmed(&app, 80, 24, "REE"),
            "overlay left resource lines undimmed:\n{text}"
        );
        app.handle_key(KeyCode::Char('2'), 1_000);
        assert!(app.state.story_pending.is_none());
        assert!(
            app.state
                .events
                .iter()
                .any(|e| e.text.contains("Dust cloud")),
            "{:?}",
            app.state.events
        );
        let text = render_text(&app, 80, 24);
        assert!(!text.contains("Burn through"), "{text}");
        assert!(text.contains("Colony / Research"), "{text}");
    }

    #[test]
    fn story_overlay_erases_inline_graphics() {
        let mut app = App::new(GameState::new(1_000));
        app.portraits.enabled = true;
        app.portraits.protocol = Protocol::Graphics;
        app.state.story_pending = Some(hoshi_core::PendingStory {
            event_id: "dust_cloud".into(),
            offered_at: 1_000,
            timeout_at: 10_000,
        });
        let mut out = Vec::new();
        app.paint_inline_graphics(&mut out, ratatui::layout::Rect::new(0, 0, 80, 24))
            .unwrap();
        assert_eq!(out, b"\x1b_Ga=d,d=A\x1b\\");
    }

    #[test]
    fn market_screen_shows_prices_sparkline_and_trade_preview() {
        let mut state = GameState::new(1_000);
        state.resources.iron = 1_000.0;
        let mut app = App::new(state);
        app.open(Screen::Market);
        assert_eq!(app.screen, Screen::Market);
        let text = render_text(&app, 100, 32);
        assert!(text.contains("Trade / Market"), "{text}");
        assert!(text.contains("Fe → Si"), "{text}");
        assert!(text.contains("YOU GET"), "{text}");
        assert!(
            text.chars().any(|c| "▁▂▃▄▅▆▇█".contains(c)),
            "sparkline missing: {text}"
        );
        app.handle_key(KeyCode::Char('+'), 1_000);
        let text = render_text(&app, 100, 32);
        assert!(text.contains(" 20 "), "{text}");
        let before = app.state.resources.iron;
        app.handle_key(KeyCode::Enter, 1_000);
        assert!(
            app.state.resources.iron < before,
            "trade should spend metal"
        );
    }

    #[test]
    fn diplomacy_screen_shows_standing_next_tier_and_pays_tribute() {
        let mut state = GameState::new(1_000);
        state.resources.iron = 1_000.0;
        let mut app = App::new(state);
        app.open(Screen::Diplomacy);
        assert_eq!(app.screen, Screen::Diplomacy);
        let text = render_text(&app, 100, 32);
        assert!(text.contains("Empire / Diplomacy"), "{text}");
        assert!(text.contains("Traders' guild"), "{text}");
        assert!(text.contains("Pirate clans"), "{text}");
        assert!(text.contains("Ancient empire"), "{text}");
        assert!(
            text.contains("NEUTRAL") || text.contains("DORMANT"),
            "{text}"
        );
        assert!(text.contains("NEXT"), "{text}");
        assert!(text.contains("RECENT"), "{text}");
        app.handle_key(KeyCode::Char('j'), 1_000);
        app.handle_key(KeyCode::Enter, 1_000);
        close_standing(&app, "pirates", 12.0);
        let text = render_text(&app, 100, 32);
        assert!(text.contains("tribute"), "{text}");
        assert!(text.contains("TRUCE") || text.contains("NEUTRAL"), "{text}");
        assert!(text.contains("RECENT"), "{text}");
        assert!(text.contains("pirates"), "{text}");
    }

    #[test]
    fn laws_screen_proposes_and_shows_expected_versus_actual() {
        let cfg = BalanceConfig {
            story: StoryCatalog::empty(),
            raid: RaidConfig::inactive(),
            market: MarketConfig::inactive(),
            diplomacy: DiplomacyConfig::inactive(),
            ..BalanceConfig::default()
        };
        let mut app = App::new(GameState::with_config(1_000, cfg));
        app.open(Screen::Laws);
        assert_eq!(app.screen, Screen::Laws);
        let text = render_text(&app, 100, 32);
        assert!(text.contains("Empire / Laws"), "{text}");
        assert!(text.contains("Emission cap"), "{text}");
        assert!(text.contains("Iron levy"), "{text}");
        app.handle_key(KeyCode::Enter, 1_000);
        assert!(
            app.state
                .laws
                .laws
                .iter()
                .any(|l| l.template_id == "emission_cap"),
            "propose should create a law"
        );
        let text = render_text(&app, 100, 32);
        assert!(text.contains("proposed"), "{text}");
        assert!(text.contains("#0"), "{text}");
        for _ in 0..5 {
            app.handle_key(KeyCode::Char('j'), 1_000);
        }
        app.handle_key(KeyCode::Enter, 1_000);
        assert_eq!(
            app.state.laws.laws[0].status,
            hoshi_core::LawStatus::Enacted
        );
        app.handle_key(KeyCode::Char('j'), 4_600);
        let text = render_text(&app, 100, 32);
        assert!(text.contains("exp"), "{text}");
        assert!(text.contains("act"), "{text}");
    }

    #[test]
    fn laws_screen_proposes_planet_cap_on_selected_world() {
        let mut state = GameState::new(1_000);
        let coord = Coord {
            galaxy: 1,
            system: 1,
            position: 4,
        };
        state.colonies.push(Colony {
            coord,
            kind: PlanetType::Rocky,
            size: 120,
            resources: Resources::ZERO,
            buildings: Buildings::default(),
            fleet: Fleet::default(),
            defences: hoshi_core::DefenceGrid::default(),
            build_queue: Vec::new(),
            rare: None,
            goods: Default::default(),
        });
        let mut app = App::new(state);
        app.handle_key(KeyCode::Char(']'), 1_000);
        app.open(Screen::Laws);
        assert_eq!(app.screen, Screen::Laws);
        app.handle_key(KeyCode::Enter, 1_000);
        let law = app
            .state
            .laws
            .laws
            .iter()
            .find(|l| l.template_id == "emission_cap")
            .expect("proposed emission cap");
        assert_eq!(law.jurisdiction, Jurisdiction::Planet(coord));
    }

    #[test]
    fn reports_screen_lists_age_and_opens_the_latest() {
        let mut state = GameState::new(1_000);
        let dest = state.outposts[0].coord;
        let garrison = state.outposts[0].garrison;
        let mut fleet = Fleet::default();
        fleet.add(ShipKind::LightFighter, 2);
        state.spy_reports.push(hoshi_core::SpyReport {
            id: 1,
            target: dest,
            created_at: 900,
            probes_sent: 3,
            probes_lost: 1,
            detected: true,
            score: 4.0,
            tier: "garrison".into(),
            resources: Some(Resources::ZERO),
            fleet: Some(fleet),
            defences: Some(fleet),
            garrison: Some(garrison),
            kind: Some(state.outposts[0].kind),
            size: Some(state.outposts[0].size),
            formation: Some("neutral".into()),
            stance: Some("neutral".into()),
            source: "probe".into(),
            observed_at: 900,
            arrives_at: 0,
            confidence: 0.8,
            deceived: false,
        });
        state.spy_reports.push(hoshi_core::SpyReport {
            id: 0,
            target: dest,
            created_at: 800,
            probes_sent: 1,
            probes_lost: 0,
            detected: false,
            score: 1.0,
            tier: "activity".into(),
            resources: None,
            fleet: None,
            defences: None,
            garrison: Some(1),
            kind: None,
            size: None,
            formation: None,
            stance: None,
            source: "probe".into(),
            observed_at: 800,
            arrives_at: 0,
            confidence: 0.6,
            deceived: false,
        });
        let mut app = App::new(state);
        app.open(Screen::Reports);
        assert_eq!(app.screen, Screen::Reports);
        let text = render_text(&app, 100, 32);
        assert!(text.contains("Fleet / Intel"), "{text}");
        assert!(text.contains(&dest.to_string()), "{text}");
        assert!(text.contains("1m 40s"), "{text}");
        assert!(text.contains("garrison"), "{text}");
        app.handle_key(KeyCode::Enter, 1_000);
        let text = render_text(&app, 100, 32);
        assert!(text.contains(&format!("spy {dest}")), "{text}");
        assert!(
            text_is_dimmed(&app, 100, 32, "Fleet / Intel"),
            "spy overlay left the reports chrome undimmed:\n{text}"
        );
        assert!(text.contains("probe"), "{text}");
        assert!(text.contains("conf "), "{text}");
        assert!(text.contains("observed 900"), "{text}");
        assert!(text.contains("observed 800"), "{text}");
        let newer = text.find("observed 900").expect(&text);
        let older = text.find("observed 800").expect(&text);
        assert!(newer < older, "{text}");
        assert!(text.contains("detected yes"), "{text}");
        assert!(text.contains("lost 1/3"), "{text}");
        assert!(text.contains(&format!("garrison {garrison}")), "{text}");
        app.handle_key(KeyCode::Enter, 1_000);
        let text = render_text(&app, 100, 32);
        assert!(text.contains("Fleet / Intel"), "{text}");
        assert!(!text.contains("detected yes"), "{text}");
    }

    #[test]
    fn history_screen_lists_queued_build_and_opens_lineage() {
        let mut state = GameState::with_seed(0, BalanceConfig::default(), 1);
        state.enqueue_build(BuildingKind::MetalMine, 0).unwrap();
        let mut app = App::new(state);
        app.open(Screen::History);
        assert_eq!(app.screen, Screen::History);
        let text = render_text(&app, 100, 32);
        assert!(text.contains("Records / History"), "{text}");
        assert!(text.contains("queue"), "{text}");
        app.handle_key(KeyCode::Enter, 0);
        let overlay = render_text(&app, 100, 32);
        assert!(overlay.contains("lineage"), "{overlay}");
    }

    #[test]
    fn staff_screen_lists_home_leaders() {
        let mut app = App::new(GameState::new(1_000));
        app.open(Screen::Staff);
        assert_eq!(app.screen, Screen::Staff);
        let text = render_text(&app, 100, 32);
        assert!(text.contains("Empire / Staff"), "{text}");
        assert!(text.contains("Home fleet"), "{text}");
        assert!(text.contains("GOVERNOR"), "{text}");
    }

    #[test]
    fn ops_screen_lists_schedule_alerts() {
        let mut app = App::new(GameState::new(1_000));
        app.open(Screen::Ops);
        assert_eq!(app.screen, Screen::Ops);
        let text = render_text(&app, 100, 32);
        assert!(text.contains("Fleet / Ops"), "{text}");
        assert!(text.contains("vehicles"), "{text}");
    }

    #[test]
    fn ops_registers_a_vehicle_and_plans_a_delivery_without_the_cli() {
        let mut state = GameState::new(1_000);
        state.fleet.add(ShipKind::SmallCargo, 1);
        state.resources.deuterium = 5_000.0;
        let mut app = App::new(state);
        app.open(Screen::Ops);
        let empty = render_text(&app, 120, 40);
        assert!(empty.contains("Press + to register"), "{empty}");
        assert!(!empty.contains("hoshi vehicle register"), "{empty}");
        app.handle_key(KeyCode::Char('+'), 1_000);
        assert_eq!(app.state.vehicles.len(), 1, "{}", app.status);
        assert!(
            app.state.fleet.is_empty(),
            "the parked ship joined the vehicle"
        );
        app.map_position = 2;
        app.selected = 0;
        app.handle_key(KeyCode::Enter, 1_000);
        let form = render_text(&app, 120, 40);
        assert!(form.contains("Plan mission"), "{form}");
        assert!(form.contains("1:1:2"), "{form}");
        app.handle_key(KeyCode::Enter, 1_000);
        assert!(app.mission_form.is_none(), "{}", app.status);
        assert_eq!(app.state.planned.len(), 1, "{}", app.status);
    }

    #[test]
    fn ops_schedule_scrolls_to_the_selected_row() {
        let mut state = GameState::new(1_000);
        for id in 0..24 {
            state.vehicles.push(Vehicle {
                id,
                name: format!("vehicle {id}"),
                ships: Fleet::default(),
                coord: Coord::HOME,
                free_at: 1_000,
            });
        }
        let mut app = App::new(state);
        app.open(Screen::Ops);
        assert_eq!(app.screen, Screen::Ops);
        for _ in 0..23 {
            app.handle_key(KeyCode::Char('j'), 1_000);
        }
        let text = render_text(&app, 100, 32);
        assert!(text.contains("#23"), "{text}");
        assert!(!text.contains("#0 vehicle 0"), "{text}");
    }

    #[test]
    fn map_shows_frontier_bonuses_before_colonizing() {
        let mut state = GameState::new(1_000);
        let drive = state.config.frontier.drive_id.clone();
        let astro = state.config.frontier.astro_id.clone();
        state.tech_levels.insert(drive, 1);
        state.tech_levels.insert(astro, 1);
        state.catch_up(1_001);
        let world = state
            .frontier
            .visible_worlds()
            .next()
            .expect("revealed world")
            .clone();
        let mut app = App::new(state);
        app.open(Screen::Map);
        app.handle_key(KeyCode::Char(' '), 1_000);
        app.map_system = world.coord.system;
        app.map_position = world.coord.position;
        let text = render_text(&app, 100, 32);
        assert!(text.contains("galaxy 2"), "{text}");
        assert!(text.contains("frontier"), "{text}");
        assert!(text.contains(&world.coord.to_string()), "{text}");
        assert!(
            text.contains("prod +") || text.contains("fields +") || world.rare.is_some(),
            "{text}"
        );
    }

    #[test]
    fn map_renders_at_80x24_and_scales_up() {
        let mut app = App::new(GameState::new(1_000));
        app.open(Screen::Map);
        let small = render_text(&app, 80, 24);
        assert!(small.contains("Star map"), "{small}");
        assert!(small.contains("[H]") || small.contains("home"), "{small}");
        let large = render_text(&app, 120, 40);
        assert!(large.contains("Fleet / Star map"), "{large}");
        assert!(large.contains("[H]"), "{large}");
        assert!(large.contains("home"), "{large}");
        assert!(large.contains("hjkl"), "{large}");
    }

    #[test]
    fn map_and_planet_switcher_label_moons() {
        let mut state = GameState::new(1_000);
        state.moons.push(hoshi_core::Moon::new(Coord::HOME));
        let mut app = App::new(state);
        app.open(Screen::Map);
        let map = render_text(&app, 100, 32);
        assert!(map.contains("moon"), "{map}");
        app.screen = Screen::Planet;
        let planet = render_text(&app, 100, 32);
        assert!(planet.contains("Moon"), "{planet}");
        assert!(planet.contains("phalanx"), "{planet}");
    }

    #[test]
    fn map_shows_fleets_in_flight_with_eta() {
        let dest = Coord {
            galaxy: 1,
            system: 1,
            position: 2,
        };
        let mut ships = Fleet::default();
        ships.add(ShipKind::SmallCargo, 1);
        let mut state = GameState::new(1_000);
        state.fleet.add(ShipKind::SmallCargo, 1);
        state.resources.deuterium = 10_000.0;
        state.launch_fleet(ships, dest, 100, 0, 1_000).unwrap();
        let eta = state.missions[0]
            .phase_ends_at
            .saturating_sub(state.last_tick_unix);
        let id = state.missions[0].id;
        let mut app = App::new(state);
        app.open(Screen::Map);
        let text = render_text(&app, 100, 32);
        assert!(text.contains("Fleet / Star map"), "{text}");
        assert!(text.contains("[H] f"), "{text}");
        assert!(
            text.contains(&format!(
                "fleet #{id} outbound {}",
                ui::spans::duration(eta)
            )),
            "{text}"
        );
    }

    #[test]
    fn map_enter_opens_dispatch_and_launches() {
        let mut state = GameState::new(1_000);
        state.fleet.add(ShipKind::SmallCargo, 2);
        state.resources.deuterium = 10_000.0;
        let mut app = App::new(state);
        app.open(Screen::Map);
        app.handle_key(KeyCode::Char('l'), 1_000);
        app.handle_key(KeyCode::Enter, 1_000);
        let overlay = render_text(&app, 100, 32);
        assert!(overlay.contains("dispatch"), "{overlay}");
        assert!(overlay.contains("to 1:1:2"), "{overlay}");
        assert!(overlay.contains("pick ships"), "{overlay}");
        assert!(overlay.contains("Small cargo"), "{overlay}");
        assert!(
            text_is_dimmed(&app, 100, 32, "Fleet / Star map"),
            "dispatch overlay left the map chrome undimmed:\n{overlay}"
        );
        app.handle_key(KeyCode::Char('+'), 1_000);
        let preview = render_text(&app, 100, 32);
        assert!(preview.contains("ETA"), "{preview}");
        assert!(preview.contains("colonize"), "{preview}");
        // An empty slot means colonize: cargo ships alone are refused, not flown.
        app.handle_key(KeyCode::Enter, 1_000);
        assert!(app.dispatch.is_some());
        assert!(app.state.missions.is_empty());
        assert!(app.status.contains("colony ship"), "{}", app.status);
    }

    fn outpost_at(state: &mut GameState, coord: Coord, defences: Fleet) {
        state.outposts.push(Outpost {
            coord,
            kind: PlanetType::Rocky,
            size: 8,
            defences,
            garrison: 2,
            rare: None,
            last_rebuild_at: 0,
            homeworld: false,
        });
    }

    /// Open dispatch to `dest` with its default mission and pick `count` of `kind`.
    fn open_dispatch(app: &mut App, dest: Coord, kind: ShipKind, count: u32) {
        app.open(Screen::Map);
        app.dispatch = Some(DispatchForm {
            dest,
            ships: Fleet::default(),
            selected: ShipKind::ALL.iter().position(|k| *k == kind).unwrap(),
            mission: app.dispatch_missions(dest)[0],
        });
        for _ in 0..count {
            app.handle_key(KeyCode::Char('+'), 1_000);
        }
    }

    #[test]
    fn dispatch_attacks_an_outpost_and_space_switches_to_spy() {
        let dest = Coord {
            galaxy: 1,
            system: 1,
            position: 2,
        };
        let mut state = GameState::new(1_000);
        state.resources.deuterium = 10_000.0;
        state.fleet.add(ShipKind::LightFighter, 3);
        state.fleet.add(ShipKind::EspionageProbe, 1);
        let mut defences = Fleet::default();
        defences.add(ShipKind::LightFighter, 1);
        outpost_at(&mut state, dest, defences);
        let spare = state.clone();
        let mut app = App::new(state);
        open_dispatch(&mut app, dest, ShipKind::LightFighter, 3);
        assert_eq!(app.dispatch.unwrap().mission, Mission::Attack);
        app.handle_key(KeyCode::Enter, 1_000);
        assert!(app.state.missions[0].attack, "{}", app.status);
        assert!(
            app.status.contains("attack launched to 1:1:2"),
            "{}",
            app.status
        );

        let mut app = App::new(spare);
        open_dispatch(&mut app, dest, ShipKind::EspionageProbe, 1);
        app.handle_key(KeyCode::Char(' '), 1_000);
        assert_eq!(app.dispatch.unwrap().mission, Mission::Spy);
        app.handle_key(KeyCode::Enter, 1_000);
        assert!(app.state.missions[0].spy, "{}", app.status);
    }

    #[test]
    fn dispatch_to_the_expedition_slot_runs_an_expedition() {
        let mut state = GameState::new(1_000);
        state.resources.deuterium = 10_000.0;
        state.fleet.add(ShipKind::SmallCargo, 1);
        let dest = state.expedition_dest();
        let mut app = App::new(state);
        open_dispatch(&mut app, dest, ShipKind::SmallCargo, 1);
        app.handle_key(KeyCode::Enter, 1_000);
        assert!(
            app.state.missions[0].expedition_seed.is_some(),
            "{}",
            app.status
        );
    }

    #[test]
    fn wide_dispatch_docks_beside_the_map() {
        let mut state = GameState::new(1_000);
        state.fleet.add(ShipKind::SmallCargo, 2);
        state.resources.deuterium = 10_000.0;
        let mut app = App::new(state);
        app.open(Screen::Map);
        app.handle_key(KeyCode::Char('l'), 1_000);
        app.handle_key(KeyCode::Enter, 1_000);
        app.handle_key(KeyCode::Char('+'), 1_000);
        let text = render_text(&app, 140, 44);
        assert!(text.contains("Fleet / Star map"), "{text}");
        assert!(text.contains("home"), "{text}");
        assert!(text.contains("to 1:1:2"), "{text}");
        assert!(text.contains("ETA"), "{text}");
        assert!(text.contains("launch"), "{text}");
    }

    #[test]
    fn governance_prompts_for_invasion_and_deny_leaves_the_fleet() {
        let dest = Coord {
            galaxy: 1,
            system: 1,
            position: 2,
        };
        let mut state = GameState::new(1_000);
        state.resources.deuterium = 10_000.0;
        state.tech_levels.insert("colonization".into(), 1);
        state.fleet.add(ShipKind::TroopTransport, 2);
        outpost_at(&mut state, dest, Fleet::default());
        let mut app = App::new(state);
        app.governance = true;
        app.open(Screen::Map);
        app.handle_key(KeyCode::Char('l'), 1_000);
        app.handle_key(KeyCode::Enter, 1_000);
        for _ in 0..10 {
            app.handle_key(KeyCode::Char('j'), 1_000);
        }
        app.handle_key(KeyCode::Char('+'), 1_000);
        app.handle_key(KeyCode::Char('+'), 1_000);
        app.handle_key(KeyCode::Enter, 1_000);
        let overlay = render_text(&app, 100, 32);
        assert!(overlay.contains("Invasion approval"), "{overlay}");
        assert!(overlay.contains("invasion-approval"), "{overlay}");
        assert_eq!(app.state.missions.len(), 0);
        app.handle_key(KeyCode::Char('x'), 1_000);
        assert!(app.pending_invasion.is_none());
        assert_eq!(app.state.missions.len(), 0);
        assert_eq!(app.state.fleet.get(ShipKind::TroopTransport), 2);
        assert!(app.status.contains("denied"), "{}", app.status);
    }

    #[test]
    fn tutorial_opening_fits_80x24_and_skip_clears_it() {
        let state = GameState::new_live_galaxy(1_000, BalanceConfig::default(), 1);
        let mut app = App::new(state);
        let text = render_text(&app, 80, 24);
        assert!(text.contains("Hint:"), "{text}");
        assert!(text.contains("Welcome"), "{text}");
        assert!(text.contains("Skip"), "{text}");
        assert!(text.contains("dismiss"), "{text}");
        // "Show me" opens the buildings, where the first upgrade is possible.
        app.handle_key(KeyCode::Char('g'), 1_000);
        assert_eq!(app.screen, Screen::Planet);
        let after = render_text(&app, 80, 24);
        assert!(!after.contains("Welcome"), "{after}");
        app.handle_key(KeyCode::Char('s'), 1_000);
        let skipped = render_text(&app, 80, 24);
        assert!(!skipped.contains("Hint:"), "{skipped}");
    }

    #[test]
    fn tutorial_keys_wait_while_an_overlay_is_open() {
        let state = GameState::new_live_galaxy(1_000, BalanceConfig::default(), 1);
        let mut app = App::new(state);
        app.set_away(AwaySummary {
            elapsed_secs: 10,
            events: vec!["away".into()],
            ..AwaySummary::default()
        });
        app.handle_key(KeyCode::Char('x'), 1_000);
        assert!(app.away_hidden);
        assert_eq!(app.state.pending_hint().unwrap().id, "opening");
        app.handle_key(KeyCode::Char('x'), 1_000);
        assert!(app.state.pending_hint().is_none_or(|h| h.id != "opening"));
    }

    #[test]
    fn energy_metric_exposes_colony_deficits() {
        let mut state = GameState::new(100);
        state.colonies.push(hoshi_core::Colony {
            coord: Coord {
                galaxy: 1,
                system: 1,
                position: 8,
            },
            kind: hoshi_core::PlanetType::Rocky,
            size: 160,
            resources: Resources::ZERO,
            buildings: hoshi_core::Buildings {
                metal_mine: 5,
                crystal_mine: 5,
                ..hoshi_core::Buildings::default()
            },
            fleet: Fleet::default(),
            defences: hoshi_core::DefenceGrid::default(),
            build_queue: Vec::new(),
            rare: None,
            goods: std::collections::BTreeMap::new(),
        });
        let mut app = App::new(state);
        let text = render_text(&app, 160, 40);
        assert!(text.contains("1 planet short"), "{text}");

        // Home short too (the deficit #294 found uncounted) makes two.
        app.state.buildings.deuterium_synthesizer = 6;
        let text = render_text(&app, 160, 40);
        assert!(text.contains("2 planets short"), "{text}");

        // Nobody short: demand over supply, with every digit inside the cell.
        app.state.colonies.clear();
        app.state.buildings.deuterium_synthesizer = 0;
        app.state.buildings.solar_plant = 30;
        let report = app.state.production_report();
        let text = render_text(&app, 160, 40);
        let used = format!(
            "{}/{} used",
            ui::spans::compact(report.energy_demand),
            ui::spans::compact(report.energy_supply)
        );
        assert!(text.contains(&used), "{used} in {text}");
    }

    #[test]
    fn cheat_console_is_hidden_when_disabled_and_fits_small_terminal() {
        let mut app = App::new(GameState::new(100));
        app.handle_key(KeyCode::Char('`'), 100);
        assert!(app.cheat_prompt.is_none());
        app.state.set_cheats_enabled(true, Actor::Player, 100);
        app.handle_key(KeyCode::Char('`'), 100);
        let text = render_text(&app, 80, 24);
        assert!(text.contains("Cheat console"), "{text}");
        assert!(text.contains("permanently"), "{text}");
        app.handle_key(KeyCode::Esc, 100);
        assert!(app.cheat_prompt.is_none());
        assert!(!app.state.is_modified());
    }

    #[test]
    fn cheat_console_opens_from_the_bridge_and_other_play_screens() {
        let mut app = App::new(GameState::new(100));
        app.screen = Screen::Cockpit;
        app.handle_key(KeyCode::Char('`'), 100);
        assert!(app.cheat_prompt.is_none());
        assert_eq!(app.status, "cheats are disabled");
        app.state.set_cheats_enabled(true, Actor::Player, 100);
        for screen in [Screen::Cockpit, Screen::Map, Screen::Market] {
            app.screen = screen;
            app.handle_key(KeyCode::Char('`'), 100);
            assert!(app.cheat_prompt.is_some(), "{screen:?}");
            app.handle_key(KeyCode::Esc, 100);
        }
        // A console key remapped onto a hub digit or a tutorial key leaves
        // that key's own action alone.
        app.keymap = Keymap::from_overrides([("cheat_console", "2")]).unwrap();
        app.screen = Screen::Cockpit;
        app.handle_key(KeyCode::Char('2'), 100);
        assert!(app.cheat_prompt.is_none());
        assert_ne!(app.screen, Screen::Cockpit);
    }

    #[test]
    fn modified_save_is_marked_and_its_feats_read_blocked() {
        let mut state = GameState::new(100);
        let clean = render_text(&App::new(state.clone()), 140, 45);
        assert!(!clean.contains("MODIFIED"), "{clean}");
        state.set_cheats_enabled(true, Actor::Player, 100);
        let intent = hoshi_core::Intent::Cheat {
            code: "grant iron 10".into(),
            actor: Actor::Player,
        };
        let proposal = hoshi_core::propose(&state, &intent, 100).unwrap();
        hoshi_core::apply(&mut state, &proposal, 100).unwrap();
        let mut app = App::new(state);
        let bridge = render_text(&app, 140, 45);
        assert!(bridge.contains("MODIFIED"), "{bridge}");
        app.screen = Screen::Achievements;
        let feats = render_text(&app, 140, 45);
        assert!(
            feats.contains("Feats are off: cheats modified this save."),
            "{feats}"
        );
        assert!(feats.contains("BLOCKED"), "{feats}");
        assert!(!feats.contains("OPEN"), "{feats}");
    }

    #[test]
    fn support_preview_owns_input_above_update_shortcuts() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "hoshi-support-overlay-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&dir).unwrap();
        let mut app = App::new(GameState::new(1_000));
        app.updates = Some(crate::update_ui::UpdateUi::start(
            dir.clone(),
            &crate::config::UserConfig::default(),
            &["hoshi".into(), "--offline".into()],
            1_000,
        ));
        app.help_open = true;
        app.open_support(&dir.join("missing.sqlite"));
        assert!(app.support.is_some());
        app.handle_key(KeyCode::Char('c'), 1_000);
        assert!(!app.updates.as_ref().unwrap().overlay_open());
        app.handle_key(KeyCode::Esc, 1_000);
        assert!(app.support.is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn help_overlay_snapshot_lists_current_screen_keys() {
        let mut app = App::new(GameState::new(1_000));
        assert_eq!(app.handle_key(KeyCode::Char('?'), 1_000), Action::None);
        let overview = render_text(&app, 80, 24);
        assert!(overview.contains("help"), "{overview}");
        assert!(overview.contains("keys for this screen"), "{overview}");
        assert!(overview.contains("quit"), "{overview}");
        assert!(overview.contains("copy galaxy seed"), "{overview}");
        assert!(overview.contains("?"), "{overview}");
        assert!(overview.contains("Fe iron"), "{overview}");
        assert!(overview.contains("REE rare earths"), "{overview}");
        assert!(overview.contains("He3 helium-3"), "{overview}");
        app.handle_key(KeyCode::Char('?'), 1_000);
        app.open(Screen::Market);
        app.handle_key(KeyCode::Char('?'), 1_000);
        let market = render_text(&app, 80, 24);
        assert!(market.contains("increase amount"), "{market}");
        assert!(!market.contains("send spy probe"), "{market}");
        app.handle_key(KeyCode::Tab, 1_000);
        let commands = render_text(&app, 80, 24);
        assert!(commands.contains("hoshi market region"), "{commands}");
        assert!(commands.contains("hoshi finance"), "{commands}");
        assert!(commands.contains("hoshi kpis"), "{commands}");
        app.handle_key(KeyCode::BackTab, 1_000);
        assert!(render_text(&app, 80, 24).contains("increase amount"));
        app.handle_key(KeyCode::Esc, 1_000);
        app.open(Screen::Planet);
        app.handle_key(KeyCode::Char('?'), 1_000);
        app.handle_key(KeyCode::Tab, 1_000);
        let colony = render_text(&app, 80, 24);
        for command in [
            "hoshi state",
            "hoshi build home solar_plant",
            "hoshi research energy",
        ] {
            assert!(colony.contains(command), "{colony}");
        }
    }

    #[test]
    fn remapped_quit_is_used_and_conflicts_stay_on_defaults() {
        let km = Keymap::from_overrides([("quit", "p")]).unwrap();
        let mut app = App::with_keymap(GameState::new(50), km);
        assert_eq!(app.handle_key(KeyCode::Char('q'), 50), Action::None);
        assert_eq!(app.handle_key(KeyCode::Char('p'), 50), Action::Quit);
        assert!(Keymap::from_overrides([("quit", "j")]).is_err());
    }

    #[test]
    fn bridge_status_bar_is_off_until_enabled() {
        let mut app = App::new(GameState::new(1_000));
        let off = render_text(&app, 80, 24);
        assert!(!off.contains("bridge idle"), "{off}");
        app.handle_key(KeyCode::Char('?'), 1_000);
        let help = render_text(&app, 80, 24);
        assert!(!help.contains("activity log"), "{help}");
        app.handle_key(KeyCode::Char('?'), 1_000);
        app.set_bridge_ui(true);
        let idle = render_text(&app, 80, 24);
        assert!(idle.contains("bridge idle"), "{idle}");
    }

    #[test]
    fn advisor_is_off_until_enabled() {
        let mut app = App::new(GameState::new(1_000));
        app.show_tutorial = false;
        app.handle_key(KeyCode::Char('i'), 1_000);
        let still = render_text(&app, 80, 24);
        assert!(!still.contains("suggestions only"), "{still}");
        app.handle_key(KeyCode::Char('?'), 1_000);
        let help = render_text(&app, 80, 24);
        assert!(!help.contains("advisor"), "{help}");
        app.handle_key(KeyCode::Char('?'), 1_000);
        app.ai_advisor = true;
        app.screen = Screen::Daily;
        let footer = render_text(&app, 80, 24);
        assert!(footer.contains("advisor"), "{footer}");
        app.handle_key(KeyCode::Char('?'), 1_000);
        let help = render_text(&app, 80, 24);
        assert!(help.contains("advisor"), "{help}");
        app.handle_key(KeyCode::Char('?'), 1_000);
        let before = app.state.clone();
        app.handle_key(KeyCode::Char('i'), 1_000);
        let panel = render_text(&app, 80, 24);
        assert!(panel.contains("suggestions only"), "{panel}");
        assert!(
            text_is_dimmed(&app, 80, 24, "Bridge / Daily"),
            "advisor overlay left the chrome undimmed:\n{panel}"
        );
        assert_eq!(app.state, before);
    }

    #[test]
    fn bridge_status_shows_latest_event_boosts_and_opens_the_panel() {
        let mut app = App::new(GameState::new(1_000));
        app.set_bridge_ui(true);
        app.state.activity.prod_boost_until = Some(1_540);
        app.record_bridge("github", "activity: production +20% for 540s", false);
        let text = render_text(&app, 100, 32);
        assert!(text.contains("github"), "{text}");
        assert!(text.contains("mines 540s"), "{text}");
        app.handle_key(KeyCode::Char('?'), 1_000);
        let help = render_text(&app, 80, 24);
        assert!(help.contains("activity log"), "{help}");
        app.handle_key(KeyCode::Char('?'), 1_000);
        app.handle_key(KeyCode::Char('a'), 1_000);
        let panel = render_text(&app, 80, 24);
        assert!(panel.contains("activity"), "{panel}");
        assert!(panel.contains("github"), "{panel}");
        assert!(
            text_is_dimmed(&app, 80, 24, "Colony / Research"),
            "activity overlay left the chrome undimmed:\n{panel}"
        );
    }

    #[test]
    fn bridge_panel_keeps_the_last_hundred_and_highlights_raids() {
        let mut app = App::new(GameState::new(1_000));
        app.set_bridge_ui(true);
        for i in 0..120 {
            app.record_bridge("shell", &format!("event {i}"), false);
        }
        app.record_bridge("github", "activity: raid incoming", true);
        app.handle_key(KeyCode::Char('a'), 1_000);
        let panel = render_text(&app, 80, 24);
        assert!(panel.contains("raid incoming"), "{panel}");
        assert!(panel.contains("!github"), "{panel}");
        assert!(!panel.contains("event 0"), "{panel}");
        assert!(!panel.contains("event 19"), "{panel}");
        let bar = render_text(&app, 100, 24);
        assert!(bar.contains("!github"), "{bar}");
    }

    #[test]
    fn activity_key_does_not_preempt_a_story_choice() {
        let mut app = App::new(GameState::new(1_000));
        app.set_bridge_ui(true);
        app.state.story_pending = Some(hoshi_core::PendingStory {
            event_id: "dust_cloud".into(),
            offered_at: 1_000,
            timeout_at: 10_000,
        });
        app.handle_key(KeyCode::Char('a'), 1_000);
        let text = render_text(&app, 80, 24);
        assert!(text.contains("Dust cloud"), "{text}");
        assert!(text.contains("Burn through"), "{text}");
        assert!(!text.contains("no events yet"), "{text}");
        app.handle_key(KeyCode::Char('2'), 1_000);
        assert!(app.state.story_pending.is_none());
    }

    #[test]
    fn cockpit_renders_all_panels_at_three_sizes() {
        let mut app = App::new(GameState::new(1_000));
        app.handle_key(KeyCode::Char('c'), 1_000);
        assert_eq!(app.screen, Screen::Cockpit);
        let full = render_text(&app, 160, 48);
        assert!(full.contains("Bridge / Dashboard"), "{full}");
        assert!(full.contains("ALERTS"), "{full}");
        for tile in ["STAR MAP", "UPCOMING", "QUEUES", "FLEETS", "TIMELINE"] {
            assert!(full.contains(tile), "missing {tile}: {full}");
        }
        let compact = render_text(&app, 120, 40);
        assert!(compact.contains("ALERTS"), "{compact}");
        assert!(
            compact.contains("STAR MAP") && compact.contains("QUEUES"),
            "{compact}"
        );
        let tabbed = render_text(&app, 80, 24);
        assert!(tabbed.contains("alerts"), "{tabbed}");
        assert!(
            tabbed.contains("MAP") || tabbed.contains("GAUGES"),
            "{tabbed}"
        );
        let again = render_text(&app, 80, 24);
        assert_eq!(tabbed, again);
    }

    #[test]
    fn cockpit_layout_cycles_and_alerts_jump() {
        let mut app = App::new(GameState::new(1_000));
        app.handle_key(KeyCode::Char('c'), 1_000);
        assert_eq!(app.cockpit.layout, crate::cockpit::CockpitLayout::Overview);
        app.handle_key(KeyCode::Char(' '), 1_000);
        assert_eq!(app.cockpit.layout, crate::cockpit::CockpitLayout::Economy);
        let econ = render_text(&app, 160, 48);
        assert!(econ.contains("GAUGES") || econ.contains("MARKET"), "{econ}");
        app.handle_key(KeyCode::Char(' '), 1_000);
        assert_eq!(app.cockpit.layout, crate::cockpit::CockpitLayout::War);
        app.handle_key(KeyCode::Enter, 1_000);
        let detail = render_text(&app, 160, 48);
        assert!(
            detail.contains("detail") || detail.contains("planet"),
            "{detail}"
        );
        app.handle_key(KeyCode::Char('x'), 1_000);
        let after = render_text(&app, 160, 48);
        assert!(after.contains("ALERTS"), "{after}");
    }

    #[test]
    fn cockpit_help_lists_layout_and_drill_keys() {
        let mut app = App::new(GameState::new(1_000));
        app.handle_key(KeyCode::Char('c'), 1_000);
        app.handle_key(KeyCode::Char('?'), 1_000);
        let text = render_text(&app, 80, 24);
        assert!(
            text.contains("cockpit dashboard") || text.contains("toggle"),
            "{text}"
        );
        assert!(text.contains("confirm") || text.contains("drill"), "{text}");
    }

    /// CPU time spent by the calling thread so far.
    #[cfg(unix)]
    fn thread_cpu_time() -> std::time::Duration {
        let mut ts = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: `ts` is a valid out-pointer for the duration of the call.
        unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
        std::time::Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
    }

    #[test]
    #[cfg(unix)]
    fn cockpit_late_game_render_stays_under_budget() {
        let mut state = GameState::new(1_000);
        state.resources.iron = 80_000.0;
        state.resources.silicon = 40_000.0;
        state.buildings.metal_mine = 20;
        for i in 0..8u16 {
            state.colonies.push(hoshi_core::Colony {
                coord: Coord {
                    galaxy: 1,
                    system: 2,
                    position: i + 1,
                },
                kind: hoshi_core::PlanetType::Rocky,
                size: 8,
                resources: hoshi_core::Resources::ZERO,
                buildings: hoshi_core::Buildings::default(),
                fleet: Fleet::default(),
                defences: hoshi_core::DefenceGrid::default(),
                build_queue: Vec::new(),
                rare: None,
                goods: Default::default(),
            });
        }
        for i in 0..32 {
            state.events.push(hoshi_core::GameEvent {
                at: 1_000 + i,
                text: format!("event {i}"),
            });
        }
        let mut app = App::new(state);
        app.handle_key(KeyCode::Char('c'), 1_000);
        // Thread CPU time, so another agent's compile on the same machine
        // cannot turn into a render regression. Every sample still renders
        // the complete frame.
        let elapsed = (0..3)
            .map(|_| {
                let started = thread_cpu_time();
                let _ = render_text(&app, 160, 48);
                thread_cpu_time() - started
            })
            .min()
            .unwrap();
        let budget = if cfg!(debug_assertions) { 0.050 } else { 0.016 };
        assert!(
            elapsed.as_secs_f64() < budget,
            "cockpit render took {elapsed:?}"
        );
    }

    fn close_standing(app: &App, id: &str, expected: f64) {
        let got = app.state.diplomacy.standing(id);
        assert!(
            (got - expected).abs() < 1e-6,
            "{id} standing {got} != {expected}"
        );
    }

    fn detail_app() -> App {
        let mut app = App::new(GameState::new(1_000));
        app.portraits.enabled = false;
        app.show_tutorial = false;
        app
    }

    fn assert_detail_on(app: &App, width: u16, height: u16, needles: &[&str]) {
        let text = render_text(app, width, height);
        for needle in needles {
            assert!(
                text.contains(needle),
                "{width}x{height} missing {needle}:\n{text}"
            );
        }
    }

    #[test]
    fn detail_panel_docks_on_wide_list_screens() {
        let mut app = detail_app();
        app.screen = Screen::Planet;
        assert_detail_on(
            &app,
            160,
            48,
            &["LV 1 → 2", "iron mine", "COST", "iron", "energy draw"],
        );
        assert_detail_on(&app, 120, 40, &["LV 1 → 2", "iron mine", "READY"]);
        app.screen = Screen::Overview;
        app.selected = app
            .overview_techs()
            .iter()
            .position(|spec| spec.id == "energy")
            .unwrap();
        assert_detail_on(&app, 160, 48, &["TECH", "Energy Technology", "REQUIRES"]);
        assert_detail_on(&app, 120, 40, &["Energy Technology"]);
        app.screen = Screen::Shipyard;
        app.selected = 0;
        assert_detail_on(&app, 160, 48, &["SHIP", "speed", "fuel"]);
        assert_detail_on(&app, 120, 40, &["speed"]);
        app.screen = Screen::Laws;
        app.selected = 0;
        let laws = render_text(&app, 160, 48);
        assert!(laws.contains("LAW"), "{laws}");
        app.screen = Screen::Map;
        let map = render_text(&app, 160, 48);
        assert!(map.contains("LOCATION"), "{map}");
        assert!(map.contains("1:1:1") || map.contains("size"), "{map}");
        let map_mid = render_text(&app, 120, 40);
        assert!(map_mid.contains("LOCATION"), "{map_mid}");
        assert!(!map_mid.contains("LV 0 → 1"), "{map_mid}");
    }

    #[test]
    fn shipyard_row_and_inspector_agree_on_the_shipyard_level() {
        let mut app = detail_app();
        // Shipyard 2 is enough for small cargo, but a dismantle takes the
        // shipyard offline for the queue gate.
        app.state.buildings.shipyard = 2;
        app.state.resources = hoshi_core::Resources::from_parts(1e6, 1e6, 1e6);
        app.state.next_raid_at = None;
        let home = app.state.home;
        app.state
            .enqueue_dismantle_as(
                home,
                BuildingKind::Shipyard,
                1_000,
                hoshi_core::Actor::Player,
            )
            .unwrap();
        app.screen = Screen::Shipyard;
        app.selected = ShipKind::ALL
            .iter()
            .position(|kind| *kind == ShipKind::SmallCargo)
            .unwrap();
        let text = render_text(&app, 160, 48);
        assert!(text.contains("NEED SHIPYARD 2"), "{text}");
        assert!(text.contains("Status   BLOCKED"), "{text}");
        assert!(!text.contains("Status   READY"), "{text}");
        assert!(text.contains("have 0"), "{text}");
    }

    #[test]
    fn market_max_names_the_cargo_cap_and_help_keys_do_not_clash() {
        let mut app = detail_app();
        app.screen = Screen::Market;
        let text = render_text(&app, 140, 45);
        assert!(text.contains("max · cargo 50"), "{text}");
        app.help_open = true;
        let help = render_text(&app, 140, 45);
        assert!(help.contains("v Changelog"), "{help}");
        assert!(help.contains("p Support report"), "{help}");
        assert!(!help.contains("c Changelog"), "{help}");
    }

    #[test]
    fn feats_and_legacy_say_what_items_need_and_give() {
        let mut app = detail_app();
        app.screen = Screen::Achievements;
        let feats = render_text(&app, 160, 48);
        assert!(feats.contains("Raise an iron mine to level 2."), "{feats}");
        assert!(feats.contains("starting buildings"), "{feats}");
        app.screen = Screen::Prestige;
        let legacy = render_text(&app, 160, 48);
        assert!(legacy.contains("needs score 80"), "{legacy}");
        assert!(legacy.contains("+8% production"), "{legacy}");
        assert!(legacy.contains("legacy points"), "{legacy}");
        assert!(!legacy.contains(" locked "), "{legacy}");
    }

    #[test]
    fn maintenance_and_staff_give_units_and_roles() {
        let mut app = detail_app();
        app.state.catch_up(1_000 + 3_600);
        app.open(Screen::Maintenance);
        let maint = render_text(&app, 160, 48);
        assert!(maint.contains("NEXT READING IN"), "{maint}");
        assert!(maint.contains("% of sink"), "{maint}");
        assert!(maint.contains("failure chance"), "{maint}");
        assert!(!maint.contains("fail p"), "{maint}");
        app.open(Screen::Staff);
        let staff = render_text(&app, 160, 48);
        assert!(staff.contains("−5% output"), "{staff}");
        assert!(staff.contains("governor: scales"), "{staff}");
    }

    #[test]
    fn finance_and_kpis_define_their_numbers() {
        let mut app = detail_app();
        app.open(Screen::Finance);
        let finance = render_text(&app, 160, 48);
        assert!(finance.contains("production"), "{finance}");
        assert!(
            finance.contains("A spend over its budget limit"),
            "{finance}"
        );
        assert!(!finance.contains("approve"), "{finance}");
        app.open(Screen::Kpis);
        let kpis = render_text(&app, 160, 48);
        assert!(kpis.contains("EVENTS"), "{kpis}");
        assert!(!kpis.contains("-0.0"), "{kpis}");
    }

    #[test]
    fn automation_tiers_name_the_research_that_unlocks_them() {
        let mut app = detail_app();
        app.open(Screen::Automation);
        let text = render_text(&app, 160, 48);
        assert!(
            text.contains("research Adaptive routes 1 (have 0)"),
            "{text}"
        );
        assert!(!text.contains("auto routes"), "{text}");
    }

    #[test]
    fn detail_overlay_opens_at_80x24() {
        let mut app = detail_app();
        app.screen = Screen::Planet;
        let hidden = render_text(&app, 80, 24);
        assert!(!hidden.contains("energy draw"), "{hidden}");
        app.handle_key(KeyCode::Char('d'), 1_000);
        let text = render_text(&app, 80, 24);
        assert!(text.contains("Detail"), "{text}");
        assert!(text.contains("iron mine"), "{text}");
        app.handle_key(KeyCode::Char('d'), 1_000);
        let closed = render_text(&app, 80, 24);
        assert!(!closed.contains("energy draw"), "{closed}");
    }

    #[test]
    fn detail_time_until_affordable_shows_never_at_zero_income() {
        let mut cfg = BalanceConfig::default();
        cfg.raid.enabled = false;
        cfg.market.enabled = false;
        cfg.starting_buildings.metal_mine = 0;
        cfg.starting_buildings.crystal_mine = 0;
        cfg.starting_buildings.deuterium_synthesizer = 0;
        cfg.starting_buildings.solar_plant = 0;
        cfg.starting_resources.iron = 0.0;
        cfg.starting_resources.silicon = 0.0;
        cfg.starting_resources.deuterium = 0.0;
        // The lab meets energy's only requirement, so the ETA is what shows.
        cfg.starting_buildings.research_lab = 1;
        let mut app = App::new(GameState::with_config(0, cfg));
        app.portraits.enabled = false;
        app.show_tutorial = false;
        app.screen = Screen::Overview;
        app.selected = app
            .overview_techs()
            .iter()
            .position(|spec| spec.id == "energy")
            .unwrap();
        let text = render_text(&app, 160, 48);
        assert!(text.contains("NEVER at current income"), "{text}");
    }

    #[test]
    fn detail_exclusive_warning_lists_locked_siblings() {
        let mut app = detail_app();
        app.selected = app
            .overview_techs()
            .iter()
            .position(|spec| spec.id == "orbital_harvest")
            .unwrap();
        let text = render_text(&app, 160, 48);
        assert!(text.contains("locks"), "{text}");
        assert!(text.contains("Deep-core mining"), "{text}");
    }

    #[test]
    fn detail_ship_travel_matches_dispatch_maths() {
        let mut app = detail_app();
        app.screen = Screen::Shipyard;
        app.selected = 0;
        app.map_galaxy = 1;
        app.map_system = 2;
        app.map_position = 4;
        let dest = Coord {
            galaxy: 1,
            system: 2,
            position: 4,
        };
        let travel = app
            .state
            .describe(DetailSubject::Ship(ShipKind::SmallCargo), Some(dest))
            .unwrap()
            .ship
            .expect("ship travel");
        let secs = travel.outbound_secs.expect("outbound");
        let fuel = travel.fuel_cost.expect("fuel");
        let text = render_text(&app, 160, 48);
        assert!(text.contains("TRAVEL"), "{text}");
        assert!(text.contains(&ui::spans::duration(secs)), "{text}");
        let fuel_bit = format!("fuel {fuel:.1}");
        assert!(text.contains(&fuel_bit), "missing {fuel_bit} in\n{text}");
    }

    #[test]
    fn detail_toggle_persists_and_help_lists_the_key() {
        let mut app = detail_app();
        let _ = render_text(&app, 160, 48);
        assert!(app.take_detail_panel().is_none());
        app.handle_key(KeyCode::Char('d'), 1_000);
        assert_eq!(app.take_detail_panel(), Some(false));
        app.handle_key(KeyCode::Char('?'), 1_000);
        let help = render_text(&app, 160, 48);
        assert!(help.contains("toggle detail panel"), "{help}");
        let user = crate::config::UserConfig {
            detail_panel: false,
            ..Default::default()
        };
        app.apply_cockpit(&user);
        app.screen = Screen::Planet;
        let hidden = render_text(&app, 160, 48);
        assert!(!hidden.contains("energy draw"), "{hidden}");
    }

    #[test]
    fn play_starts_on_the_bridge() {
        let app = App::with_keymap(GameState::new(1_000), Keymap::defaults());
        assert_eq!(app.screen, Screen::Cockpit);
        let text = render_text(&app, 120, 40);
        assert!(text.contains("Bridge / Dashboard"), "{text}");
        assert!(text.contains("ALERTS") || text.contains("alerts"), "{text}");
    }

    #[test]
    fn dismantle_dialog_shows_costs_and_blockers_and_honors_remapping() {
        let mut app = App::new(GameState::new(1_000));
        app.state.config.raid = hoshi_core::RaidConfig::inactive();
        app.screen = Screen::Planet;
        app.selected = BuildingKind::ALL
            .iter()
            .position(|kind| *kind == BuildingKind::MetalMine)
            .unwrap();
        app.handle_key(KeyCode::Char('D'), 1_000);
        assert!(app.dismantle_confirm.is_some());
        app.handle_key(KeyCode::Esc, 1_000);
        app.keymap = app
            .keymap
            .rebind(Binding::Dismantle, KeyCode::Char(';'))
            .unwrap();
        app.handle_key(KeyCode::Char('D'), 1_000);
        assert!(app.dismantle_confirm.is_none());
        app.handle_key(KeyCode::Char(';'), 1_000);
        let text = render_text(&app, 80, 24);
        for label in [
            "Dismantle one level?",
            "Time",
            "Power during work",
            "Power freed",
            "Heat freed",
            "Salvage",
        ] {
            assert!(text.contains(label), "missing {label}:\n{text}");
        }
        app.handle_key(KeyCode::Esc, 1_000);
        assert!(app.state.build_queue.is_empty());
        app.state.buildings.robotics_factory = 1;
        app.state.buildings.shipyard = 1;
        app.selected = BuildingKind::ALL
            .iter()
            .position(|kind| *kind == BuildingKind::RoboticsFactory)
            .unwrap();
        app.handle_key(KeyCode::Char(';'), 1_000);
        let text = render_text(&app, 80, 24);
        assert!(text.contains("Cannot dismantle"), "{text}");
        assert!(text.contains("shipyard"), "{text}");
        app.handle_key(KeyCode::Enter, 1_000);
        assert!(app.state.build_queue.is_empty());
    }

    #[test]
    fn hub_digits_open_hubs_and_cycle_their_tabs() {
        let mut app = App::new(GameState::new(1_000));
        app.handle_key(KeyCode::Char('3'), 1_000);
        assert_eq!(app.screen, Screen::Shipyard);
        app.handle_key(KeyCode::Char('3'), 1_000);
        assert_eq!(app.screen, Screen::Map);
        app.handle_key(KeyCode::Char('4'), 1_000);
        assert_eq!(app.screen, Screen::Market);
        app.handle_key(KeyCode::Char('4'), 1_000);
        assert_eq!(app.screen, Screen::Finance);
        app.handle_key(KeyCode::Char('4'), 1_000);
        assert_eq!(app.screen, Screen::Kpis);
        app.handle_key(KeyCode::Tab, 1_000);
        assert_eq!(app.screen, Screen::Diplomacy);
        app.handle_key(KeyCode::BackTab, 1_000);
        assert_eq!(app.screen, Screen::Kpis);
        app.handle_key(KeyCode::Char('5'), 1_000);
        assert_eq!(app.screen, Screen::Diplomacy);
        app.handle_key(KeyCode::Char('5'), 1_000);
        assert_eq!(app.screen, Screen::Laws);
        app.handle_key(KeyCode::Char('5'), 1_000);
        assert_eq!(app.screen, Screen::Staff);
        app.handle_key(KeyCode::Char('5'), 1_000);
        assert_eq!(app.screen, Screen::Automation);
        app.handle_key(KeyCode::Char('5'), 1_000);
        assert_eq!(app.screen, Screen::Incidents);
        app.handle_key(KeyCode::Char('1'), 1_000);
        assert_eq!(app.screen, Screen::Cockpit);
        let text = render_text(&app, 80, 24);
        assert!(text.contains("Bridge / Dashboard"), "{text}");
    }

    #[test]
    fn esc_closes_the_window_but_x_still_cancels() {
        let mut state = GameState::new(1_000);
        state.resources.iron = 100_000.0;
        state.resources.silicon = 100_000.0;
        let mut app = App::new(state);
        app.open(Screen::Planet);
        app.handle_key(KeyCode::Enter, 1_000);
        assert_eq!(app.state.build_queue.len(), 1);
        app.handle_key(KeyCode::Esc, 1_000);
        assert_eq!(app.screen, Screen::Cockpit);
        assert_eq!(app.state.build_queue.len(), 1);
        app.open(Screen::Planet);
        app.handle_key(KeyCode::Char('x'), 1_000);
        assert!(app.state.build_queue.is_empty());
        assert_eq!(app.screen, Screen::Planet);
    }

    #[test]
    fn hub_window_floats_over_the_dimmed_bridge_on_large_terminals() {
        let mut app = App::new(GameState::new(1_000));
        app.open(Screen::Planet);
        let backend = TestBackend::new(160, 48);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();
        let text = buffer_text(terminal.backend());
        assert!(text.contains("Colony"), "{text}");
        assert!(text.contains("Buildings"), "{text}");
        assert!(text.contains("BUILDING"), "{text}");
        let theme = Theme::default();
        let edge = &terminal.backend().buffer()[(1, 10)];
        assert_eq!(
            edge.bg,
            theme.surface(Surface::Dimmed).bg.unwrap(),
            "{text}"
        );
        let small = render_text(&app, 80, 24);
        assert!(small.contains("✕ esc"), "{small}");
    }

    #[test]
    fn hub_window_floats_from_120x40_and_fills_below() {
        let frame = Rect::new(0, 0, 120, 40);
        let body = Rect::new(0, 4, 120, 35);
        assert_ne!(window_area(frame, body), body);
        let short = Rect::new(0, 0, 120, 39);
        let short_body = Rect::new(0, 4, 120, 34);
        assert_eq!(window_area(short, short_body), short_body);
        let narrow = Rect::new(0, 0, 119, 48);
        let narrow_body = Rect::new(0, 4, 119, 43);
        assert_eq!(window_area(narrow, narrow_body), narrow_body);
    }

    #[test]
    fn scenario_screen_compares_without_writing_live_until_promote() {
        let mut app = App::new(GameState::new(1_000));
        let iron = app.state.resources.iron;
        app.open(Screen::Scenarios);
        assert_eq!(app.screen, Screen::Scenarios);
        app.handle_key(KeyCode::Char('+'), 1_000);
        app.handle_key(KeyCode::Char('+'), 1_000);
        app.handle_key(KeyCode::Char(' '), 1_000);
        assert_eq!(app.state.resources.iron, iron);
        assert!(app.state.build_queue.is_empty());
        let sandbox_iron = app
            .scenario
            .as_ref()
            .expect("sandbox")
            .outcome()
            .resources
            .iron;
        assert_ne!(sandbox_iron, iron);
        let text = render_text(&app, 120, 32);
        assert!(text.contains("Records / Scenarios"), "{text}");
        assert!(text.contains("Iron"), "{text}");
        assert!(text.contains("SANDBOX"), "{text}");
        assert!(text.contains(&format!("{iron:.0}")), "{text}");
        assert!(text.contains(&format!("{sandbox_iron:.0}")), "{text}");
        app.handle_key(KeyCode::Enter, 1_000);
        assert_eq!(app.state.build_queue.len(), 1);
        assert_eq!(app.state.build_queue[0].building, BuildingKind::MetalMine);
        assert!(app.scenario.is_none());
    }

    #[test]
    fn scenario_promote_refuses_a_stale_plan() {
        let mut app = App::new(GameState::new(1_000));
        app.open(Screen::Scenarios);
        app.handle_key(KeyCode::Char('+'), 1_000);
        app.handle_key(KeyCode::Char('+'), 1_000);
        app.state.resources = Resources::ZERO;
        app.handle_key(KeyCode::Enter, 1_000);
        assert!(
            app.status.contains("afford") || !app.status.is_empty(),
            "{}",
            app.status
        );
        assert!(app.state.build_queue.is_empty());
        assert!(app.scenario.is_some());
    }

    #[test]
    fn scenario_compare_shows_intel_confidence_ranges() {
        let mut state = GameState::new(1_000);
        let dest = state.outposts[0].coord;
        state.spy_reports.push(hoshi_core::SpyReport {
            id: 1,
            target: dest,
            created_at: 900,
            probes_sent: 1,
            probes_lost: 0,
            detected: false,
            score: 4.0,
            tier: "garrison".into(),
            resources: Some(Resources {
                iron: 12.0,
                ..Resources::ZERO
            }),
            fleet: None,
            defences: None,
            garrison: Some(2),
            kind: Some(state.outposts[0].kind),
            size: Some(state.outposts[0].size),
            formation: None,
            stance: None,
            source: "probe".into(),
            observed_at: 900,
            arrives_at: 0,
            confidence: 0.8,
            deceived: false,
        });
        state.spy_reports.push(hoshi_core::SpyReport {
            id: 2,
            target: dest,
            created_at: 950,
            probes_sent: 1,
            probes_lost: 0,
            detected: false,
            score: 4.0,
            tier: "garrison".into(),
            resources: Some(Resources {
                iron: 20.0,
                ..Resources::ZERO
            }),
            fleet: None,
            defences: None,
            garrison: Some(2),
            kind: Some(state.outposts[0].kind),
            size: Some(state.outposts[0].size),
            formation: None,
            stance: None,
            source: "probe".into(),
            observed_at: 950,
            arrives_at: 0,
            confidence: 0.6,
            deceived: false,
        });
        let mut app = App::new(state);
        app.open(Screen::Scenarios);
        app.handle_key(KeyCode::Char('+'), 1_000);
        let text = render_text(&app, 120, 36);
        assert!(text.contains("CONF"), "{text}");
        assert!(text.contains("12-20"), "{text}");
        assert!(text.contains("0.60"), "{text}");
        assert!(text.contains("Rare earths"), "{text}");
        assert!(text.contains("Helium 3"), "{text}");
        assert!(!text.contains("deceived"), "{text}");
        assert!(!text.contains("next_raid_at"), "{text}");
    }

    #[test]
    fn scenario_compare_joins_intel_by_object_and_field() {
        let mut state = GameState::new(1_000);
        let dest = state.outposts[0].coord;
        state.spy_reports.push(hoshi_core::SpyReport {
            id: 1,
            target: dest,
            created_at: 900,
            probes_sent: 1,
            probes_lost: 0,
            detected: false,
            score: 4.0,
            tier: "garrison".into(),
            resources: Some(Resources {
                iron: 12.0,
                ..Resources::ZERO
            }),
            fleet: None,
            defences: None,
            garrison: Some(2),
            kind: Some(state.outposts[0].kind),
            size: Some(state.outposts[0].size),
            formation: None,
            stance: None,
            source: "probe".into(),
            observed_at: 900,
            arrives_at: 0,
            confidence: 0.8,
            deceived: false,
        });
        let mut inbound = Fleet::default();
        inbound.add(ShipKind::LightFighter, 7);
        state.spy_reports.push(hoshi_core::SpyReport {
            id: 2,
            target: dest,
            created_at: 950,
            probes_sent: 1,
            probes_lost: 0,
            detected: false,
            score: 4.0,
            tier: "fleet".into(),
            resources: None,
            fleet: Some(inbound),
            defences: None,
            garrison: None,
            kind: None,
            size: None,
            formation: None,
            stance: None,
            source: "probe".into(),
            observed_at: 950,
            arrives_at: 1_500,
            confidence: 0.9,
            deceived: false,
        });
        let mut app = App::new(state);
        app.open(Screen::Scenarios);
        app.handle_key(KeyCode::Char('+'), 1_000);
        app.handle_key(KeyCode::Char(' '), 1_000);
        let rows = app.scenario_compare_rows();
        let iron = rows
            .iter()
            .find(|(metric, _, _, _)| metric.contains("resources.iron"))
            .expect("iron fact");
        assert_eq!(iron.1, "12");
        assert_eq!(iron.2, "12");
        let fleet = rows
            .iter()
            .find(|(metric, _, _, _)| metric.contains("fleet.light_fighter"))
            .expect("fleet fact");
        assert_eq!(fleet.1, "-");
        assert_eq!(fleet.2, "7");
        let sandbox_conf = app
            .scenario
            .as_ref()
            .expect("sandbox")
            .outcome()
            .intel
            .iter()
            .find(|fact| fact.field == "fleet.light_fighter")
            .and_then(|fact| fact.observations.last())
            .map(|obs| format!("{:.2}", obs.confidence))
            .expect("sandbox fleet confidence");
        assert_eq!(fleet.3, sandbox_conf);
        assert!(
            iron.3.contains('/'),
            "aged sandbox confidence should differ from live: {}",
            iron.3
        );
        app.selected = rows
            .iter()
            .position(|(metric, _, _, _)| metric.contains("resources.iron"))
            .expect("iron fact index");
        let text = render_text(&app, 120, 36);
        assert!(text.contains(&iron.3), "{text}");
        assert!(app.state.build_queue.is_empty());
    }
}
