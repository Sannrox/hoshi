//! Overview, planet, shipyard, wonder, prestige, achievements, daily, goals, production, market, diplomacy, laws, reports, ops and map screens.
//! Core owns the rules; this module only renders and turns keys into core commands.

use crate::clip::copy_to_clipboard;
use crate::detail_panel::{self, DOCK_MIN_WIDTH};
use crate::hubs::{self, Hub};
use crate::keymap::{Binding, HelpContext, Keymap};
use crate::notify::{self, Prefs as NotifyPrefs, Watch as NotifyWatch};
use crate::portraits::{self, PortraitView, Protocol, Slot};
use crate::title::AwaySummary;
use crate::ui::chrome::{Dialog, Navbar, StatusBar, Tabs, Tile};
use crate::ui::panels::{Metric, MetricStrip};
use crate::ui::table::{Column, DataTable};
use crate::ui::{self, Depth, Intent, Surface, Theme};
use hoshi_core::{
    AssignmentTarget, BuildingKind, CombatLog, CombatWinner, CommentaryCatalog, Coord, DefenceKind,
    Detail, DetailSubject, Fleet, GameState, HintScreen, HistoryKind, HistoryQuery, Jurisdiction,
    JurisdictionKind, LawStatus, Permission, ResourceKind, Resources, RoleKind, ShipKind,
    SpyReport, StoryEvent, TechBranch, TechNodeState, WonderError, advise, encode_galaxy_seed,
    polish_all_named, rephrase_lines,
};
use ratatui::Frame;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};
use std::cell::Cell;
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
    Map,
    Cockpit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HistoryPane {
    List,
    Lineage,
    AsOf,
}

const MAP_SYSTEM_MAX: u16 = 81;
const MAP_SLOT_COUNT: u16 = 16;

#[derive(Debug, Clone, Copy, PartialEq)]
struct DispatchForm {
    dest: Coord,
    ships: Fleet,
    selected: usize,
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
    None,
    Quit,
}

pub struct App {
    pub state: GameState,
    pub portraits: PortraitView,
    keymap: Keymap,
    help_open: bool,
    screen: Screen,
    selected: usize,
    planet_index: usize,
    status: String,
    confirming_prestige: bool,
    trade_amount: f64,
    away: Option<AwaySummary>,
    away_hidden: bool,
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
            state,
            portraits: PortraitView::default(),
            keymap,
            help_open: false,
            screen: Screen::Cockpit,
            selected: 0,
            planet_index: 0,
            status: String::new(),
            confirming_prestige: false,
            trade_amount: 10.0,
            away: None,
            away_hidden: false,
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
            Screen::Market => TRADE_PAIRS
                .get(self.selected)
                .map(|&(pay, recv)| DetailSubject::Trade { pay, recv }),
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
        let Some(log) = self.state.combat_logs.last().cloned() else {
            self.status = "no combat log".into();
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
    }

    pub fn set_notify(&mut self, prefs: NotifyPrefs) {
        self.notify = prefs;
        self.watch = NotifyWatch::from_state(&self.state);
    }

    pub fn poll_notify(&mut self) -> Vec<u8> {
        let now = self.state.last_tick_unix;
        let notices = self.watch.observe(&self.state, now);
        notify::encode(self.notify, now, &notices)
    }

    pub fn notify_away(&self, summary: &AwaySummary) -> Vec<u8> {
        let Some(notice) = notify::summarize_away(summary, self.notify) else {
            return Vec::new();
        };
        notify::encode(self.notify, self.state.last_tick_unix, &[notice])
    }

    pub fn catch_up_now(&mut self, now_unix: u64) {
        self.state.interactive_tactics = true;
        self.state.catch_up(now_unix);
        self.record_production_sample();
        crate::cockpit::record_gauge(&mut self.gauge_history, self.state.resources);
    }

    pub fn handle_key(&mut self, code: KeyCode, now_unix: u64) -> Action {
        self.state.interactive_tactics = true;
        self.state.catch_up(now_unix);
        self.clamp_planet();
        if self.help_open {
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
            return Action::None;
        }
        if self.replay.is_some() {
            return self.handle_replay_key(code);
        }
        if self.away.is_some() && !self.away_hidden {
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
            Some(Binding::Increase) if self.screen == Screen::Market => {
                self.trade_amount = (self.trade_amount + 10.0).min(1_000_000.0);
            }
            Some(Binding::Increase) if self.screen == Screen::Automation => {
                self.add_default_automation_rule();
            }
            Some(Binding::Toggle) if self.screen == Screen::Automation => {
                self.toggle_selected_automation_rule();
            }
            Some(Binding::Decrease) if self.screen == Screen::Market => {
                self.trade_amount = (self.trade_amount - 10.0).max(10.0);
            }
            Some(Binding::SendProbe) if self.screen == Screen::Reports && !self.opened_report => {
                self.send_probe(now_unix);
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
            Some(Binding::Toggle) if self.screen == Screen::Map && self.dispatch.is_none() => {
                self.cycle_map_galaxy();
            }
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
            Screen::Map => HelpContext::Map,
            Screen::Cockpit => HelpContext::Cockpit,
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
        let [nav, metrics, body, status] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(MetricStrip::HEIGHT),
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
                metrics: self.metrics(),
            },
            metrics,
        );
        frame.render_widget(
            StatusBar {
                theme: t,
                mode: hub.title().to_uppercase(),
                hints: self.global_hints(area.width),
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
        } else {
            self.state
                .events
                .last()
                .map(|event| Line::styled(event.text.clone(), t.muted()))
        };
        let dialog = Dialog {
            theme: t,
            title: hub.title().into(),
            subtitle: format!("{}  ·  Planet {}", hub.digit(), self.selected_coord()),
            close_hint: "esc bridge".into(),
            footer_note: note,
            buttons: self.window_buttons(),
        };
        let inner = pad_x(dialog.inner(area), 1);
        frame.render_widget(dialog, area);
        let screens = hub.screens();
        let glance = self.glance_lines(hub);
        let footer = self.footer_lines(inner.width as usize);
        let [tabs, glance_area, body, footer_area] = Layout::vertical([
            Constraint::Length(Tabs::HEIGHT),
            Constraint::Length(glance.len() as u16),
            Constraint::Min(1),
            Constraint::Length(footer.len() as u16),
        ])
        .areas(inner);
        let (list_area, portrait) = portraits::split_area(body);
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
        frame.render_widget(Paragraph::new(glance).style(t.muted()), glance_area);
        if self.shows_docked_detail() {
            let [list, card] = Layout::horizontal([
                Constraint::Min(24),
                Constraint::Length(detail_column_width(list_area.width)),
            ])
            .areas(list_area);
            self.draw_list(frame, list);
            frame.render_widget(
                Tile {
                    theme: t,
                    title: "detail".into(),
                    right: None,
                    focused: false,
                },
                card,
            );
            if let Some(detail) = self.current_detail() {
                frame.render_widget(
                    Paragraph::new(detail_panel::inspector(&detail, t)).wrap(Wrap { trim: false }),
                    pad_x(Tile::inner(card), 1),
                );
            }
        } else {
            self.draw_list(frame, list_area);
        }
        frame.render_widget(Paragraph::new(footer).style(t.muted()), footer_area);
        if let Some(area) = portrait {
            let art = Layout::vertical([
                Constraint::Length(portraits::ASCII_ROWS as u16),
                Constraint::Min(0),
            ])
            .split(area);
            let slot = self.current_portrait_slot();
            let mood = portraits::mood_for(slot, &self.state.events);
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

    fn navbar_right(&self) -> Vec<Span<'static>> {
        let t = &self.theme;
        let mut spans = Vec::new();
        if let Some(secs) = self.state.raid_countdown_secs() {
            spans.push(ui::spans::tag(
                t,
                &format!("RAID {}", ui::spans::countdown(secs)),
                Intent::Danger,
                false,
            ));
            spans.push(Span::raw("  "));
        }
        if let Some(run) = self.state.species.as_ref() {
            spans.push(Span::styled(run.name.clone(), t.muted()));
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(self.selected_coord().to_string(), t.text()));
        spans
    }

    fn metrics(&self) -> Vec<Metric> {
        let t = &self.theme;
        let s = &self.state;
        let mut out: Vec<Metric> = ResourceKind::ALL
            .iter()
            .map(|&kind| {
                let stock = s.resources.get(kind);
                let cap = s.storage_cap(kind);
                let rate = s.production_per_hour(kind);
                let full = cap > 0.0 && stock / cap >= 0.95;
                let value_style = if full {
                    t.intent(Intent::Warning)
                } else {
                    t.strong()
                };
                let rate_style = if rate < 0.0 {
                    t.intent(Intent::Danger)
                } else if rate > 0.0 {
                    t.intent(Intent::Success)
                } else {
                    t.label()
                };
                Metric {
                    label: kind.abbr().into(),
                    value: Line::styled(ui::spans::compact(stock), value_style),
                    foot: Line::styled(
                        format!(
                            "{}{}/h",
                            if rate > 0.0 { "+" } else { "" },
                            ui::spans::compact(rate)
                        ),
                        rate_style,
                    ),
                }
            })
            .collect();
        let waste = s.waste_home();
        let balance = s.energy_balance();
        out.push(Metric {
            label: "Energy".into(),
            value: Line::styled(
                format!("{balance:+.0}"),
                if balance < 0.0 {
                    t.intent(Intent::Danger)
                } else {
                    t.strong()
                },
            ),
            foot: Line::styled(
                format!("heat {:.0}/{:.0}", waste.heat_load, waste.heat_sink),
                if waste.overflowing() {
                    t.intent(Intent::Danger)
                } else {
                    t.label()
                },
            ),
        });
        out
    }

    fn global_hints(&self, width: u16) -> Vec<(String, String)> {
        let k = &self.keymap;
        let mut hints = vec![("1-6".to_string(), "hubs".to_string())];
        if width >= 100 {
            hints.push((k.primary(Binding::NextScreen), "next".into()));
        }
        if self.screen != Screen::Cockpit {
            hints.push(("esc".into(), "bridge".into()));
        }
        hints.push((k.primary(Binding::Help), "help".into()));
        hints.push((k.primary(Binding::Quit), "quit".into()));
        hints
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
            Screen::Finance => "approve",
            Screen::Kpis => "events",
            Screen::Cockpit
            | Screen::Achievements
            | Screen::Daily
            | Screen::Goals
            | Screen::Production
            | Screen::Ops => return Vec::new(),
        };
        let key = self.keymap.primary(Binding::Confirm);
        vec![ui::spans::button(
            &self.theme,
            &format!("{key} {action}"),
            true,
        )]
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

    fn glance_lines(&self, hub: Hub) -> Vec<Line<'static>> {
        let mut lines = match hub {
            Hub::Colony => vec![
                Line::from(self.build_status()),
                Line::from(self.research_status()),
                Line::from(self.wonder_status()),
            ],
            Hub::Fleet => vec![
                Line::from(self.ship_status()),
                Line::from(self.raid_status()),
            ],
            Hub::Records => vec![Line::from(self.prestige_status())],
            Hub::Bridge | Hub::Trade | Hub::Empire => Vec::new(),
        };
        if hub == Hub::Colony {
            let waste = self.state.waste_home();
            if self.screen != Screen::Planet
                && (waste.overflowing() || waste.warns(self.state.config.waste.warn_ratio))
            {
                lines.push(Line::from(waste_header_line(&self.state)));
            }
            if self.state.owned_coords().len() > 1 || !self.state.moons.is_empty() {
                lines.extend(self.planet_glance_lines());
            }
        }
        if !lines.is_empty() {
            lines.push(Line::default());
        }
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
            self.draw_dispatch(frame, form);
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
            render_overlay(frame, " detail ", detail_panel::paragraph(&detail));
        }
        if self.help_open {
            self.draw_help(frame);
        }
    }

    fn current_portrait_slot(&self) -> Slot {
        let coord = self.selected_coord();
        portraits::slot_for_screen(
            self.screen == Screen::Planet,
            self.screen == Screen::Shipyard || self.screen == Screen::Staff,
            self.state.planet_kind(coord),
            self.state.rare_bonus_at(coord).is_some(),
            self.state.events.last().map(|e| e.text.as_str()),
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
            || self.screen == Screen::Cockpit
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
        let mood = portraits::mood_for(slot, &self.state.events);
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
        let mut lines = vec![
            Line::from(format!(
                "round {}/{}  winner {winner}",
                snap.round,
                report.rounds.max(snap.round)
            )),
            Line::from(format!(
                "pirates {}  {}",
                snap.attacker.total(),
                fleet_bar(snap.attacker.total())
            )),
            Line::from(format!(
                "you     {}  {}",
                snap.defender.total(),
                fleet_bar(snap.defender.total())
            )),
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
        lines.push(Line::from(""));
        lines.push(Line::from(format!(
            "{} pause  {} step  {} skip",
            self.keymap.primary(Binding::Toggle),
            self.keymap.primary(Binding::ReplayStep),
            self.keymap.primary(Binding::Cancel)
        )));
        render_overlay(frame, " combat replay ", lines);
    }

    fn draw_away(&self, frame: &mut Frame<'_>, away: &AwaySummary) {
        let mut lines = vec![
            Line::from(format!("elapsed {}s", away.elapsed_secs)),
            Line::from(format!(
                "resources +{:.0} metal +{:.0} crystal +{:.0} deuterium",
                away.iron, away.silicon, away.deuterium
            )),
            Line::from(format!(
                "builds {}  research {}  fleets returned {}  expeditions {}  raids {}",
                away.builds, away.research, away.fleets_returned, away.expeditions, away.raids
            )),
        ];
        push_away_section(&mut lines, "deliveries", &away.deliveries);
        push_away_section(&mut lines, "stalls", &away.stalls);
        push_away_section(&mut lines, "completed", &away.completed);
        for event in away.events.iter().take(4) {
            lines.push(Line::from(event.clone()));
        }
        lines.push(Line::from(format!(
            "{} dismisses. {} replay. {} on overview reopens.",
            self.keymap.primary(Binding::Confirm),
            self.keymap.primary(Binding::Replay),
            self.keymap.primary(Binding::ShowAway)
        )));
        render_overlay(frame, " while you were away ", lines);
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
        lines.push(Line::from(""));
        lines.push(Line::from("enter hold   x retreat   space commit"));
        render_overlay(frame, " tactical ", lines);
    }

    fn draw_story(&self, frame: &mut Frame<'_>, event: &StoryEvent) {
        let mut lines = vec![Line::from(event.text.clone()), Line::from("")];
        for (i, choice) in event.choices.iter().enumerate() {
            let n = i + 1;
            let mark = if choice.id == event.timeout_choice {
                " (default if ignored)"
            } else {
                ""
            };
            lines.push(Line::from(format!(
                "{n} {label}{mark}",
                label = choice.label
            )));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(
            "1-9 chooses. Ignored too long uses the default.",
        ));
        render_overlay(frame, format!(" {} ", event.title), lines);
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
                    match self.state.enqueue_ships(kind, 1, now_unix) {
                        Ok(()) => self.status.clear(),
                        Err(e) => self.status = e.to_string(),
                    }
                    return;
                }
                let di = self.selected.saturating_sub(ShipKind::ALL.len());
                let Some(kind) = DefenceKind::ALL.get(di).copied() else {
                    return;
                };
                match self.state.enqueue_defence(kind, 1, now_unix) {
                    Ok(()) => self.status.clear(),
                    Err(e) => self.status = e.to_string(),
                }
            }
            Screen::Overview => {
                let Some(id) = self.tech_id_at(self.selected) else {
                    return;
                };
                match self.state.start_research(&id, now_unix) {
                    Ok(()) => self.status.clear(),
                    Err(e) => self.status = e.to_string(),
                }
            }
            Screen::Wonder => self.activate_wonder(now_unix),
            Screen::Prestige => self.activate_prestige(now_unix),
            Screen::Achievements
            | Screen::Daily
            | Screen::Goals
            | Screen::Production
            | Screen::Ops => {}
            Screen::Automation => self.activate_automation(),
            Screen::Finance => self.activate_finance(now_unix),
            Screen::Kpis => self.activate_kpi(),
            Screen::Market => self.activate_market(now_unix),
            Screen::Diplomacy => self.activate_diplomacy(now_unix),
            Screen::Laws => self.activate_law(now_unix),
            Screen::History => self.toggle_history_lineage(),
            Screen::Staff => self.activate_staff(now_unix),
            Screen::Map => self.activate_map(now_unix),
            Screen::Cockpit => self.cockpit_confirm(),
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
        self.dispatch = Some(DispatchForm {
            dest,
            ships: Fleet::default(),
            selected: 0,
        });
        self.status.clear();
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
        match self
            .state
            .launch_fleet(form.ships, form.dest, 100, 0, now_unix)
        {
            Ok(()) => {
                self.status = format!("launched to {}", form.dest);
                self.dispatch = None;
            }
            Err(e) => self.status = e.to_string(),
        }
    }

    fn is_invasion_dispatch(&self, form: &DispatchForm) -> bool {
        form.ships.get(ShipKind::TroopTransport) > 0
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
        let lines = vec![
            Line::from(format!("Approve invasion of {dest}?")),
            Line::from("policy invasion-approval"),
            Line::from("enter grant  x deny"),
        ];
        render_overlay(frame, " invasion approval ", lines);
    }

    fn draw_dispatch(&self, frame: &mut Frame<'_>, form: &DispatchForm) {
        let mut lines = vec![Line::from(format!("to {}", form.dest))];
        if !form.ships.is_empty() {
            match self.state.flight_time_and_fuel(form.ships, form.dest, 100) {
                Ok((eta, fuel)) => {
                    lines.push(Line::from(format!("ETA {eta}s  fuel {fuel:.1}")));
                }
                Err(e) => lines.push(Line::from(e.to_string())),
            }
        } else {
            lines.push(Line::from("pick ships with +/-"));
        }
        for (i, kind) in ShipKind::ALL.iter().enumerate() {
            let mark = if i == form.selected { '>' } else { ' ' };
            lines.push(Line::from(format!(
                "{mark} {} {}/{}",
                kind.as_key(),
                form.ships.get(*kind),
                self.state.fleet.get(*kind)
            )));
        }
        lines.push(Line::from("enter launch  +/- count  x close"));
        render_overlay(frame, " dispatch ", lines);
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
            | Screen::Kpis => {}
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
        }
    }

    fn tech_id_at(&self, index: usize) -> Option<String> {
        self.overview_techs().get(index).map(|spec| spec.id.clone())
    }

    fn overview_techs(&self) -> Vec<&hoshi_core::TechSpec> {
        self.state
            .config
            .techs
            .branch_techs(self.tree_branch)
            .into_iter()
            .filter(|spec| self.state.tech_node_state(&spec.id) != TechNodeState::Hidden)
            .collect()
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
        let mut rows = Vec::new();
        if self.show_tutorial
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
        rows.push(Line::from(format!("{}{activity}{advisor}", match self.screen {
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
            Screen::Production => format!(
                "{planets}  {select}  {} detail",
                k.primary(Binding::Detail)
            ),
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
            Screen::Finance => {
                format!("{select}  {confirm} approve")
            }
            Screen::Kpis => {
                format!("{select}  {confirm} events")
            }
            Screen::Ops => {
                format!("{select}  schedule")
            }
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
        })));
        rows
    }

    fn draw_help(&self, frame: &mut Frame<'_>) {
        let mut lines = self.keymap.help_lines(self.help_context());
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
        render_overlay(frame, " help ", lines.into_iter().map(Line::from).collect());
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
        let mut lines = vec![
            Line::from("advisor (suggestions only; nothing is queued)"),
            Line::from(""),
        ];
        if self.advisor_lines.is_empty() {
            lines.push(Line::from("no suggestions"));
        } else {
            for (i, text) in self.advisor_lines.iter().enumerate() {
                lines.push(Line::from(format!("{}. {text}", i + 1)));
            }
        }
        lines.push(Line::from(""));
        lines.push(Line::from(format!(
            "{} closes",
            self.keymap.primary(Binding::Cancel)
        )));
        render_overlay(frame, " advisor ", lines);
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
        lines.push(Line::from(""));
        lines.push(Line::from(format!(
            "{} closes   {}/{} scroll",
            self.keymap.primary(Binding::Cancel),
            self.keymap.primary(Binding::SelectNext),
            self.keymap.primary(Binding::SelectPrev)
        )));
        render_overlay(frame, " activity ", lines);
    }

    fn list_lines(&self, height: usize) -> Vec<Line<'static>> {
        match self.screen {
            Screen::Planet => self.planet_info_lines(),
            Screen::Shipyard => self.shipyard_lines(height),
            Screen::Overview => self.research_lines(height),
            Screen::Wonder => self.wonder_lines(height),
            Screen::Prestige => self.prestige_lines(height),
            Screen::Achievements => self.achievement_lines(height),
            Screen::Daily => self.daily_lines(height),
            Screen::Goals => self.goals_lines(height),
            Screen::Production => self.production_lines(height),
            Screen::Market => self.market_lines(height),
            Screen::Diplomacy => self.diplomacy_lines(height),
            Screen::Laws => self.law_lines(height),
            Screen::Reports => self.report_lines(height),
            Screen::History => self.history_lines(height),
            Screen::Staff => self.staff_lines(height),
            Screen::Ops => self.ops_lines(height),
            Screen::Automation => self.automation_lines(height),
            Screen::Finance => self.finance_lines(height),
            Screen::Kpis => self.kpi_lines(height),
            Screen::Map => self.map_lines(height),
            Screen::Cockpit => Vec::new(),
        }
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
                    moon.jump_ready_at.saturating_sub(self.state.last_tick_unix)
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
                                "Phalanx  fleet #{} {} {} -> {}  {}s  ships {}",
                                contact.id,
                                contact.phase,
                                contact.origin,
                                contact.dest,
                                contact.eta_secs,
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
        if !deposits.is_empty() {
            let parts: Vec<String> = deposits
                .iter()
                .map(|(id, richness)| format!("{id} {richness:.2}"))
                .collect();
            rows.push(Line::from(format!("Deposits  {}", parts.join("  "))));
        }
        for (i, job) in queue.iter().enumerate() {
            let left = job
                .completes_at
                .map(|t| t.saturating_sub(self.state.last_tick_unix));
            let timer = match left {
                Some(s) => format!("{s}s"),
                None => "wait".to_string(),
            };
            rows.push(Line::from(format!(
                "  {}. {} -> lv{}  {timer}",
                i + 1,
                job.building,
                job.from_level + 1
            )));
        }
        rows
    }

    fn buildings_table(&self) -> DataTable<'_> {
        let t = &self.theme;
        let coord = self.selected_coord();
        let levels = self
            .state
            .buildings_at(coord)
            .unwrap_or(&self.state.buildings);
        let rows = BuildingKind::ALL
            .iter()
            .map(|&kind| {
                let reason = self.building_block_reason(kind);
                let base = if reason.is_some() {
                    t.label().add_modifier(Modifier::DIM)
                } else {
                    t.text()
                };
                let status = match reason {
                    Some(reason) => Line::styled(
                        reason,
                        t.intent(Intent::Warning).add_modifier(Modifier::DIM),
                    ),
                    None => Line::styled("ready", t.intent(Intent::Success)),
                };
                vec![
                    Line::styled(kind.to_string(), base),
                    Line::styled(levels.get(kind).to_string(), base),
                    Line::styled(cost_label(self.state.build_cost_on(coord, kind)), base),
                    Line::styled(
                        ui::spans::duration(self.state.build_time_secs_on(coord, kind)),
                        base,
                    ),
                    status,
                ]
            })
            .collect();
        DataTable {
            theme: t,
            columns: vec![
                Column::left("building", 22),
                Column::right("lv", 3),
                Column::left("cost", 18),
                Column::right("time", 7),
                Column::fill("status"),
            ],
            rows,
            selected: Some(self.selected),
            striped: true,
            surface: Surface::Raised,
        }
    }

    fn draw_list(&self, frame: &mut Frame<'_>, area: Rect) {
        let t = &self.theme;
        if self.screen == Screen::Planet {
            let info = self.planet_info_lines();
            let top_h = (info.len() as u16 + 1).min(area.height / 2);
            let [top, table] =
                Layout::vertical([Constraint::Length(top_h), Constraint::Min(2)]).areas(area);
            frame.render_widget(Paragraph::new(info).style(t.text()), top);
            frame.render_widget(self.buildings_table(), table);
        } else {
            frame.render_widget(
                Paragraph::new(self.list_lines(area.height as usize)).style(t.text()),
                area,
            );
        }
    }

    fn shipyard_lines(&self, height: usize) -> Vec<Line<'static>> {
        let mut rows: Vec<Line<'static>> = Vec::new();
        rows.push(Line::from(format!(
            "Queue {}/{}",
            self.state.ship_queue.len(),
            self.state.config.max_ship_queue
        )));
        for (i, job) in self.state.ship_queue.iter().enumerate() {
            let left = job
                .completes_at
                .map(|t| t.saturating_sub(self.state.last_tick_unix));
            let timer = match left {
                Some(s) => format!("{s}s"),
                None => "wait".to_string(),
            };
            rows.push(Line::from(format!(
                "  {}. {} x{}  {timer}",
                i + 1,
                job.label(),
                job.remaining
            )));
        }
        rows.push(Line::from("Ships and defences"));
        let mut entries: Vec<Line<'static>> = ShipKind::ALL
            .iter()
            .enumerate()
            .map(|(i, &kind)| self.ship_row(i, kind))
            .collect();
        entries.extend(
            DefenceKind::ALL
                .iter()
                .enumerate()
                .map(|(i, &kind)| self.defence_row(ShipKind::ALL.len() + i, kind)),
        );
        append_windowed(&mut rows, &entries, self.selected, height)
    }

    fn research_lines(&self, height: usize) -> Vec<Line<'static>> {
        let tabs = self
            .visible_tree_branches()
            .into_iter()
            .map(|branch| {
                if branch == self.tree_branch {
                    format!("[{}]", branch.as_key())
                } else {
                    branch.as_key().to_string()
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        let mut rows = vec![Line::from(format!("Tree  {tabs}  h/l branch"))];
        let specs = self.overview_techs();
        let entries: Vec<Line<'static>> = specs
            .iter()
            .enumerate()
            .map(|(i, spec)| self.tech_row(i, spec))
            .collect();
        append_windowed(&mut rows, &entries, self.selected, height)
    }

    fn wonder_lines(&self, height: usize) -> Vec<Line<'static>> {
        let mut rows = Vec::new();
        match &self.state.wonder {
            Some(job) => {
                let spec = self.state.config.wonders.iter().find(|s| s.id == job.id);
                let name = spec.map(|s| s.name.as_str()).unwrap_or(job.id.as_str());
                let total = spec.map(|s| s.stages.len()).unwrap_or(0);
                let remaining = self.state.wonder_remaining().unwrap_or(Resources::ZERO);
                let eta = match self.state.wonder_eta_secs() {
                    Some(secs) => format!("{secs}s"),
                    None => "?".into(),
                };
                rows.push(Line::from(format!(
                    "Active {name}  stage {}/{total}  {}  eta {eta}",
                    job.stage + 1,
                    missing_label(remaining)
                )));
            }
            None => rows.push(Line::from("Active none")),
        }
        rows.push(Line::from("Wonders"));
        let entries: Vec<Line<'static>> = self
            .state
            .config
            .wonders
            .iter()
            .enumerate()
            .map(|(i, spec)| self.wonder_row(i, spec))
            .collect();
        append_windowed(&mut rows, &entries, self.selected, height)
    }

    fn wonder_row(&self, index: usize, spec: &hoshi_core::WonderSpec) -> Line<'static> {
        let mark = if index == self.selected { '>' } else { ' ' };
        let text = format!("{mark} {}  {} stages", spec.name, spec.stages.len());
        styled_row(text, self.wonder_block_reason(spec))
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
                return Some(format!("need {} {}", pre.id, pre.level));
            }
        }
        None
    }

    fn ship_row(&self, index: usize, kind: ShipKind) -> Line<'static> {
        let spec = self.state.config.ships.get(kind);
        let cost = spec.unit_cost();
        let secs = spec.duration_secs();
        let owned = self.state.fleet.get(kind);
        let mark = if index == self.selected { '>' } else { ' ' };
        let stats = self.state.ship_stats(kind);
        let text = format!(
            "{mark} {kind}  x{owned}  {}  {secs}s  atk {:.0}",
            cost_label(cost),
            stats.attack
        );
        styled_row(text, self.ship_block_reason(kind, cost))
    }

    fn defence_row(&self, index: usize, kind: DefenceKind) -> Line<'static> {
        let spec = self.state.config.defences.get(kind);
        let cost = spec.unit_cost();
        let secs = spec.duration_secs();
        let owned = self.state.defences.get(kind);
        let mark = if index == self.selected { '>' } else { ' ' };
        let text = format!(
            "{mark} {kind}  x{owned}  {}  {secs}s  atk {:.0}",
            cost_label(cost),
            spec.attack
        );
        styled_row(text, self.defence_block_reason(kind, cost))
    }

    fn defence_block_reason(&self, kind: DefenceKind, cost: Resources) -> Option<String> {
        let spec = self.state.config.defences.get(kind);
        if self.state.buildings.shipyard < spec.shipyard {
            return Some(format!("need shipyard {}", spec.shipyard));
        }
        if !self.state.resources.can_afford(cost) {
            return Some(missing_label(self.state.resources.missing(cost)));
        }
        None
    }

    fn ship_block_reason(&self, kind: ShipKind, cost: Resources) -> Option<String> {
        let spec = self.state.config.ships.get(kind);
        if self.state.buildings.shipyard < spec.shipyard {
            return Some(format!("need shipyard {}", spec.shipyard));
        }
        if let Some(gap) = self.state.missing_ship_prereqs(kind).first() {
            return Some(format!("need {gap}"));
        }
        if !self.state.resources.can_afford(cost) {
            return Some(missing_label(self.state.resources.missing(cost)));
        }
        None
    }

    fn tech_row(&self, index: usize, spec: &hoshi_core::TechSpec) -> Line<'static> {
        let level = self.state.tech_level(&spec.id);
        let cost = spec.cost(level);
        let secs = spec.duration_secs(level);
        let mark = if index == self.selected { '>' } else { ' ' };
        let indent = "  ".repeat(spec.tier.min(4) as usize);
        let state = self.state.tech_node_state(&spec.id).as_key();
        let extra = if spec.keystone { " keystone" } else { "" };
        let text = format!(
            "{mark}{indent}{}  lv{level}  {state}{extra}  {}  {secs}s",
            spec.name,
            cost_label(cost)
        );
        styled_row(text, self.tech_block_reason(&spec.id, cost))
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
            return Some(format!("need {gap}"));
        }
        if !self.state.resources.can_afford(cost) {
            return Some(missing_label(self.state.resources.missing(cost)));
        }
        None
    }

    fn build_status(&self) -> String {
        match self.state.build_queue.first() {
            Some(job) => {
                let left = job
                    .completes_at
                    .unwrap_or(self.state.last_tick_unix)
                    .saturating_sub(self.state.last_tick_unix);
                format!(
                    "Build: {} -> lv{}  {left}s  ({}/{} queued)",
                    job.building,
                    job.from_level + 1,
                    self.state.build_queue.len(),
                    self.state.config.max_build_queue
                )
            }
            None => "Build: idle".to_string(),
        }
    }

    fn research_status(&self) -> String {
        match &self.state.research {
            Some(job) => {
                let left = job.completes_at.saturating_sub(self.state.last_tick_unix);
                format!(
                    "Research: {} -> lv{}  {left}s",
                    job.tech,
                    job.from_level + 1
                )
            }
            None => "Research: idle".to_string(),
        }
    }

    fn ship_status(&self) -> String {
        match self.state.ship_queue.first() {
            Some(job) => {
                let left = job
                    .completes_at
                    .unwrap_or(self.state.last_tick_unix)
                    .saturating_sub(self.state.last_tick_unix);
                format!(
                    "Ships      {}  {} x{}  {left}s  ({}/{} queued)",
                    self.state.fleet.total(),
                    job.label(),
                    job.remaining,
                    self.state.ship_queue.len(),
                    self.state.config.max_ship_queue
                )
            }
            None => format!(
                "Ships      {}  idle  (0/{} queued)",
                self.state.fleet.total(),
                self.state.config.max_ship_queue
            ),
        }
    }

    fn raid_status(&self) -> String {
        match self.state.raid_countdown_secs() {
            Some(secs) => format!("Raid: {secs}s"),
            None => "Raid: none".to_string(),
        }
    }

    fn prestige_status(&self) -> String {
        format!(
            "Prestige: {} unspent  run {}",
            self.state.prestige.currency, self.state.prestige.current.index
        )
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

    fn prestige_lines(&self, height: usize) -> Vec<Line<'static>> {
        let preview = self.state.prestige_preview();
        let mut rows = vec![Line::from(format!(
            "Run {}  score {}  earned {}  {}",
            preview.run.index,
            preview.score,
            preview.earned,
            if preview.eligible { "ready" } else { "locked" }
        ))];
        if self.confirming_prestige {
            rows.push(Line::from(format!(
                "Reset keeps: {}",
                preview.kept.join(", ")
            )));
            rows.push(Line::from(format!(
                "Reset loses: {}",
                preview.lost.join(", ")
            )));
            rows.push(Line::from("Enter again to confirm. x cancels."));
        }
        let mut entries = vec![Line::from(if self.selected == 0 {
            "> Reset".to_string()
        } else {
            "  Reset".to_string()
        })];
        for (i, spec) in self.state.config.prestige.bonuses.iter().enumerate() {
            let mark = if self.selected == i + 1 { '>' } else { ' ' };
            let owned = self.state.prestige.owned_count(&spec.id);
            entries.push(Line::from(format!(
                "{mark} {}  cost {}  owned {owned}",
                spec.name, spec.cost
            )));
        }
        append_windowed(&mut rows, &entries, self.selected, height)
    }

    fn achievement_lines(&self, height: usize) -> Vec<Line<'static>> {
        let earned = self.state.achievements.len();
        let total = self.state.config.achievements.len();
        let mut rows = vec![Line::from(format!("Earned {earned}/{total}"))];
        let entries: Vec<Line<'static>> = self
            .state
            .config
            .achievements
            .iter()
            .enumerate()
            .map(|(i, spec)| {
                let mark = if i == self.selected { '>' } else { ' ' };
                let unlocked = self.state.achievements.contains_key(&spec.id);
                let title = spec.listed_title(unlocked);
                if unlocked {
                    let at = self.state.achievements[&spec.id];
                    Line::from(format!("{mark} {title}  earned t{at}"))
                } else if spec.hidden {
                    Line::from(format!("{mark} ???"))
                } else {
                    let current = spec.condition.current(&self.state);
                    let target = spec.condition.target();
                    Line::from(format!("{mark} {title}  {current:.0}/{target:.0}"))
                }
            })
            .collect();
        append_windowed(&mut rows, &entries, self.selected, height)
    }

    fn daily_lines(&self, height: usize) -> Vec<Line<'static>> {
        let resets = self.state.daily_resets_in();
        let streak = self.state.daily.streak;
        let cap = self.state.config.daily.streak_cap;
        let mut rows = vec![Line::from(format!(
            "Streak {streak}/{cap}  resets in {resets}s"
        ))];
        let entries: Vec<Line<'static>> = self
            .state
            .daily
            .slots
            .iter()
            .enumerate()
            .map(|(i, slot)| {
                let mark = if i == self.selected { '>' } else { ' ' };
                let name = self
                    .state
                    .config
                    .daily
                    .get(&slot.spec_id)
                    .map(|s| s.name.as_str())
                    .unwrap_or(slot.spec_id.as_str());
                let done = if slot.done() { " done" } else { "" };
                Line::from(format!(
                    "{mark} {name}  {}/{}{done}",
                    slot.progress, slot.target
                ))
            })
            .collect();
        append_windowed(&mut rows, &entries, self.selected, height)
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
        let lines = vec![
            Line::from(format!("Galaxy complete: {}", record.path.label())),
            Line::from(format!(
                "+{} prestige in {}s",
                record.prestige, record.elapsed_secs
            )),
            Line::from("Enter keep playing. Prestige screen starts a new galaxy."),
        ];
        render_overlay(frame, " victory ", lines);
    }

    fn goals_lines(&self, height: usize) -> Vec<Line<'static>> {
        let ladder = self.state.ladder_progress();
        let progress = self.state.victory_progress();
        let mut rows = Vec::new();
        let current = ladder.current.as_deref().unwrap_or("none");
        rows.push(Line::from(format!("Ladder  {current}")));
        if let Some(goal) = &ladder.next {
            rows.push(Line::from(format!(
                "Next  {}  {:.0}%  {}",
                goal.name,
                goal.ratio * 100.0,
                goal.next_step
            )));
        } else {
            rows.push(Line::from("Next  ladder complete"));
        }
        if let Some(record) = &self.state.victory {
            rows.push(Line::from(format!(
                "Won via {}  +{} prestige",
                record.path.label(),
                record.prestige
            )));
        } else {
            rows.push(Line::from("Galaxy victory: first path wins"));
        }
        if let Some(coord) = self.state.pirate_homeworld() {
            rows.push(Line::from(format!("Pirate homeworld {coord}")));
        }
        let paths = [
            ("Conquest", &progress.conquest),
            ("Wonder", &progress.wonder),
            ("Diplomacy", &progress.diplomacy),
        ];
        let entries: Vec<Line<'static>> = paths
            .iter()
            .enumerate()
            .map(|(i, (name, path))| {
                let mark = if i == self.selected { '>' } else { ' ' };
                let ready = if path.ready { " ready" } else { "" };
                Line::from(format!(
                    "{mark} {name}  {:.0}%  {}{ready}",
                    path.ratio * 100.0,
                    path.next_step
                ))
            })
            .collect();
        append_windowed(&mut rows, &entries, self.selected, height)
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

    fn production_lines(&self, height: usize) -> Vec<Line<'static>> {
        let report = self.state.production_report();
        let coord = self.selected_coord();
        let mut rows = vec![
            Line::from(format!(
                "limit {}  {}",
                report.limited_by.as_key(),
                report.limited_by.suggestion()
            )),
            Line::from(format!(
                "energy {:.0}/{:.0}  heat {:.0}/{:.0}  Fe {:.0} Si {:.0} REE {:.0} H2O {:.0} He3 {:.0} D {:.0}/h",
                report.energy_supply,
                report.energy_demand,
                report.heat_load,
                report.heat_sink,
                report.metal_per_hour,
                report.crystal_per_hour,
                report.rare_earths_per_hour,
                report.water_ice_per_hour,
                report.helium_3_per_hour,
                report.deuterium_per_hour
            )),
            Line::from(format!("rate {}", sparkline(&self.prod_history, 24))),
        ];
        let entries: Vec<Line<'static>> = report
            .steps
            .iter()
            .filter(|step| step.planet == coord)
            .enumerate()
            .map(|(i, step)| {
                let mark = if i == self.selected { '>' } else { ' ' };
                let out = step
                    .outputs_per_hour
                    .iter()
                    .map(|(id, n)| format!("{id} {n:.1}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                let buf = step
                    .buffers
                    .iter()
                    .map(|(id, n)| format!("{id} {n:.0}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                Line::from(format!(
                    "{mark} {}  {out}  buf {buf}  {}",
                    step.id,
                    step.limited_by.as_key()
                ))
            })
            .collect();
        append_windowed(&mut rows, &entries, self.selected, height)
    }

    fn market_lines(&self, height: usize) -> Vec<Line<'static>> {
        let market = &self.state.market;
        let fee_pct = self.state.effective_market_fee() * 100.0;
        let mut rows = vec![
            Line::from(format!(
                "Prices  metal {:.3}  crystal {:.3}  deuterium {:.3}",
                market.iron, market.silicon, market.deuterium
            )),
            Line::from(format!(
                "metal     {}",
                sparkline(market.history(ResourceKind::Iron), 24)
            )),
            Line::from(format!(
                "crystal   {}",
                sparkline(market.history(ResourceKind::Silicon), 24)
            )),
            Line::from(format!(
                "deuterium {}",
                sparkline(market.history(ResourceKind::Deuterium), 24)
            )),
            Line::from(format!(
                "amount {:.0}  +/-  fee {fee_pct:.0}%",
                self.trade_amount
            )),
        ];
        let entries: Vec<Line<'static>> = TRADE_PAIRS
            .iter()
            .enumerate()
            .map(|(i, &(pay, recv))| {
                let mark = if i == self.selected { '>' } else { ' ' };
                match self.state.quote_market(pay, recv, self.trade_amount) {
                    Ok(quote) => Line::from(format!(
                        "{mark} {:.0} {} -> {:.1} {}  preview",
                        quote.pay_amount,
                        pay.label(),
                        quote.recv_amount,
                        recv.label()
                    )),
                    Err(e) => {
                        Line::from(format!("{mark} {} -> {}  {e}", pay.label(), recv.label()))
                    }
                }
            })
            .collect();
        let mut rows = append_windowed(&mut rows, &entries, self.selected, height);
        rows.push(Line::from(format!(
            "hub {}  orders {}  contracts {}  shipments {}",
            self.state.config.market.hub(),
            self.state.board.orders.len(),
            self.state.board.contracts.len(),
            self.state
                .board
                .shipments
                .values()
                .filter(|s| !s.delivered)
                .count()
        )));
        if let Some(region) = self.state.board.regions.get(&self.state.home) {
            rows.push(Line::from(format!(
                "home book  crystal {}  deut {}",
                sparkline(region.history(ResourceKind::Silicon), 16),
                sparkline(region.history(ResourceKind::Deuterium), 16)
            )));
        }
        for contract in self.state.board.contracts.values().take(3) {
            rows.push(Line::from(format!(
                "contract #{} {} {:.0} {} {} due {}",
                contract.id,
                contract.kind.as_key(),
                contract.remaining,
                contract.good.label(),
                contract.status.as_key(),
                contract.deadline
            )));
        }
        rows
    }

    fn law_lines(&self, height: usize) -> Vec<Line<'static>> {
        let mut rows = vec![Line::from(
            "Laws  enter propose / approve / repeal  expected vs measured",
        )];
        let mut entries: Vec<Line<'static>> = Vec::new();
        let mut i = 0;
        for spec in self.state.config.laws.iter() {
            let mark = if i == self.selected { '>' } else { ' ' };
            entries.push(Line::from(format!(
                "{mark} propose {}  {}={:.2}  {}",
                spec.title,
                spec.param_name,
                spec.default,
                match spec.jurisdiction {
                    JurisdictionKind::Empire => "empire",
                    JurisdictionKind::Planet => "planet",
                }
            )));
            i += 1;
        }
        for law in &self.state.laws.laws {
            let mark = if i == self.selected { '>' } else { ' ' };
            let mut line = format!(
                "{mark} #{} {} {} {}={:.2} {}",
                law.id,
                law.template_id,
                law.status.as_key(),
                "param",
                law.param,
                law.jurisdiction.as_key()
            );
            for sample in self.state.laws.latest_samples(law.id) {
                line.push_str(&format!(
                    "  {}: exp {:.1} act {:.1}",
                    sample.metric.as_key(),
                    sample.expected,
                    sample.actual
                ));
            }
            entries.push(Line::from(line));
            i += 1;
        }
        let _ = i;
        append_windowed(&mut rows, &entries, self.selected, height)
    }

    fn map_lines(&self, height: usize) -> Vec<Line<'static>> {
        let cursor = self.map_cursor();
        let mut rows = vec![Line::from(format!(
            "Map  galaxy {}  cursor {cursor}  hjkl move  enter dispatch  space other galaxy",
            cursor.galaxy
        ))];
        let owned = self.state.owned_coords();
        let mut entries: Vec<Line<'static>> = Vec::new();
        for system in 1..=MAP_SYSTEM_MAX {
            let mark = if system == self.map_system { '>' } else { ' ' };
            let mut glyphs = String::with_capacity(MAP_SLOT_COUNT as usize);
            let mut labels = Vec::new();
            for pos in 1..=MAP_SLOT_COUNT {
                let coord = Coord {
                    galaxy: cursor.galaxy,
                    system,
                    position: pos,
                };
                let ch = if coord == self.state.home {
                    'H'
                } else if owned.contains(&coord) {
                    'C'
                } else if self.state.outposts.iter().any(|o| o.coord == coord) {
                    'P'
                } else if self
                    .state
                    .frontier
                    .visible_worlds()
                    .any(|w| w.coord == coord)
                {
                    'F'
                } else if self.state.missions.iter().any(|m| m.dest == coord) {
                    'f'
                } else {
                    '.'
                };
                if pos == self.map_position && system == self.map_system {
                    glyphs.push('[');
                    glyphs.push(ch);
                    glyphs.push(']');
                } else {
                    glyphs.push(ch);
                }
                if coord == self.state.home {
                    labels.push("home".into());
                } else if owned.contains(&coord) {
                    labels.push(format!("colony {coord}"));
                } else if let Some(out) = self.state.outposts.iter().find(|o| o.coord == coord) {
                    labels.push(format!("pirate {coord} {}", out.kind.as_key()));
                } else if let Some(world) = self
                    .state
                    .frontier
                    .visible_worlds()
                    .find(|w| w.coord == coord)
                {
                    let bonus = world
                        .rare
                        .as_deref()
                        .and_then(|id| {
                            self.state
                                .config
                                .outposts
                                .rare
                                .iter()
                                .find(|row| row.id == id)
                                .map(|row| row.summary())
                        })
                        .unwrap_or_else(|| {
                            format!(
                                "prod +{:.0}% fields +{}",
                                self.state.config.frontier.production_bonus * 100.0,
                                self.state.config.frontier.fields_bonus
                            )
                        });
                    labels.push(format!("frontier {coord} {} {bonus}", world.kind.as_key()));
                }
                if self.state.moon_at(coord).is_some() {
                    labels.push("moon".into());
                }
            }
            for mission in &self.state.missions {
                if mission.dest.galaxy == cursor.galaxy && mission.dest.system == system {
                    let eta = mission
                        .phase_ends_at
                        .saturating_sub(self.state.last_tick_unix);
                    labels.push(format!(
                        "fleet #{} {} {eta}s",
                        mission.id,
                        mission.phase.as_key()
                    ));
                }
            }
            let extra = if labels.is_empty() {
                String::new()
            } else {
                format!("  {}", labels.join("  "))
            };
            entries.push(Line::from(format!("{mark} {system:>3} {glyphs}{extra}")));
        }
        let selected = self.map_system.saturating_sub(1) as usize;
        append_windowed(&mut rows, &entries, selected, height)
    }

    fn diplomacy_lines(&self, height: usize) -> Vec<Line<'static>> {
        let cfg = &self.state.config.diplomacy;
        let mut rows = vec![Line::from(if cfg.enabled {
            format!(
                "Decay {:.0}% / {}s  standing {}..{}",
                cfg.decay * 100.0,
                cfg.tick_secs,
                cfg.min_standing,
                cfg.max_standing
            )
        } else {
            "Diplomacy off".into()
        })];
        if self.state.diplomacy.log.is_empty() {
            rows.push(Line::from("Recent: none"));
        } else {
            rows.push(Line::from("Recent:"));
            for change in self.state.diplomacy.log.iter().rev().take(4) {
                rows.push(Line::from(format!(
                    "  {} {:+.0} {} t{}",
                    change.faction, change.delta, change.reason, change.at
                )));
            }
        }
        let entries: Vec<Line<'static>> = cfg
            .iter()
            .enumerate()
            .map(|(i, spec)| {
                let mark = if i == self.selected { '>' } else { ' ' };
                let standing = self.state.diplomacy.standing(&spec.id);
                let tier = spec.tier(standing).map(|t| t.name.as_str()).unwrap_or("-");
                let next = match spec.next_tier(standing) {
                    Some(t) => format!("next {} {:.0}", t.name, t.min),
                    None => "max".into(),
                };
                let tribute = if spec.tribute_metal > 0.0 {
                    format!(
                        "  tribute {:.0}M {:+.0}",
                        spec.tribute_metal, spec.tribute_delta
                    )
                } else {
                    String::new()
                };
                Line::from(format!(
                    "{mark} {}  {standing:.0}  {tier}  {next}{tribute}",
                    spec.name
                ))
            })
            .collect();
        if self.state.rivals.enabled {
            rows.push(Line::from("Rivals:"));
            for row in self.state.rival_intel() {
                let stock = match row.resources {
                    Some(r) => format!("  {:.0}M {:.0}C {:.0}D", r.iron, r.silicon, r.deuterium),
                    None => String::new(),
                };
                let fleet = match row.fleet {
                    Some(f) => format!("  fleet {}", f.get(ShipKind::LightFighter)),
                    None => String::new(),
                };
                rows.push(Line::from(format!(
                    "  {} {} {} last {}{stock}{fleet}",
                    row.name, row.home, row.personality, row.last_action
                )));
            }
        }
        append_windowed(&mut rows, &entries, self.selected, height)
    }

    fn report_lines(&self, height: usize) -> Vec<Line<'static>> {
        let now = self.state.last_tick_unix;
        let mut rows = vec![Line::from("[reports]")];
        if self.state.outposts.is_empty() {
            rows.push(Line::from("No pirate outposts"));
            return rows;
        }
        let entries: Vec<Line<'static>> = self
            .state
            .outposts
            .iter()
            .enumerate()
            .map(|(i, outpost)| {
                let mark = if i == self.selected { '>' } else { ' ' };
                match self.state.latest_report(outpost.coord) {
                    Some(report) => Line::from(format!(
                        "{mark} {}  {}  age {}s  lost {}/{}{}",
                        outpost.coord,
                        report.tier,
                        report.age_secs(now),
                        report.probes_lost,
                        report.probes_sent,
                        if report.detected { "  detected" } else { "" }
                    )),
                    None => Line::from(format!("{mark} {}  no report", outpost.coord)),
                }
            })
            .collect();
        append_windowed(&mut rows, &entries, self.selected, height)
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

    fn history_lines(&self, height: usize) -> Vec<Line<'static>> {
        let kind = self
            .history_kind
            .map(|k| k.as_key().to_string())
            .unwrap_or_else(|| "all".into());
        let actor = self.history_actor.clone().unwrap_or_else(|| "all".into());
        let mut rows = vec![Line::from(format!(
            "[log] type:{kind} actor:{actor}  {} retained",
            self.state.history.events.len()
        ))];
        let events = self.history_rows();
        if events.is_empty() {
            rows.push(Line::from("no history yet"));
            return rows;
        }
        let entries: Vec<Line<'static>> = events
            .iter()
            .enumerate()
            .map(|(i, event)| {
                let mark = if i == self.selected { '>' } else { ' ' };
                Line::from(format!(
                    "{mark} #{} t{} {} {} {}  {}",
                    event.id,
                    event.at,
                    event.kind.as_key(),
                    event.actor,
                    event.action,
                    event.objects.join(",")
                ))
            })
            .collect();
        append_windowed(&mut rows, &entries, self.selected, height)
    }

    fn ops_lines(&self, height: usize) -> Vec<Line<'static>> {
        let schedule = self.state.mission_schedule(self.state.last_tick_unix);
        let mut rows = vec![Line::from(format!(
            "[ops] {} vehicles  {} missions  {} alerts",
            schedule.vehicles.len(),
            schedule.missions.len(),
            schedule.alerts.len()
        ))];
        if schedule.vehicles.is_empty() && schedule.missions.is_empty() {
            rows.push(Line::from(
                "Register a vehicle with hoshi vehicle register, then mission plan.",
            ));
            return rows;
        }
        let mut entries = Vec::new();
        let mut index = 0usize;
        for vehicle in &schedule.vehicles {
            let mark = if index == self.selected { '>' } else { ' ' };
            index += 1;
            let booked = if vehicle.free_at > self.state.last_tick_unix {
                format!("booked until {}", vehicle.free_at)
            } else {
                "idle".into()
            };
            entries.push(Line::from(format!(
                "{mark} #{} {} @ {}  {booked}",
                vehicle.id, vehicle.name, vehicle.coord
            )));
        }
        for mission in &schedule.missions {
            let mark = if index == self.selected { '>' } else { ' ' };
            index += 1;
            entries.push(Line::from(format!(
                "{mark} mission {} {} -> {}  {}  eta {}",
                mission.id,
                mission.route.origin,
                mission.route.dest,
                mission.transfer.as_key(),
                mission.arrives_at.saturating_sub(self.state.last_tick_unix)
            )));
        }
        for alert in &schedule.alerts {
            let mark = if index == self.selected { '>' } else { ' ' };
            index += 1;
            let kind = match alert.kind {
                hoshi_core::ScheduleAlertKind::DoubleBook => "double-book",
                hoshi_core::ScheduleAlertKind::MissedWindow => "missed window",
            };
            entries.push(Line::from(format!(
                "{mark} ! {kind} vehicle #{}  {}",
                alert.vehicle_id, alert.detail
            )));
        }
        append_windowed(&mut rows, &entries, self.selected, height)
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

    fn automation_lines(&self, height: usize) -> Vec<Line<'static>> {
        let catalog = self.state.automation_catalog();
        let mut rows = vec![Line::from(format!(
            "[auto] control energy {:.0}  interval {}s",
            self.state.automation_control_energy(),
            catalog.interval_secs
        ))];
        let mut entries = Vec::new();
        let mut index = 0usize;
        for tier in &catalog.tier {
            let mark = if index == self.selected { '>' } else { ' ' };
            index += 1;
            let lock = if self.state.automation_tier_unlocked(&tier.id) {
                "unlocked"
            } else {
                "locked"
            };
            entries.push(Line::from(format!(
                "{mark} {lock} {} ({})",
                tier.name, tier.research
            )));
        }
        let preview = self.state.preview_automation();
        for rule in &self.state.automation_rules {
            let mark = if index == self.selected { '>' } else { ' ' };
            index += 1;
            let pre = preview
                .iter()
                .find(|p| p.rule_id == rule.id)
                .map(|p| p.detail.as_str())
                .unwrap_or("");
            let on = if rule.enabled { "on" } else { "off" };
            entries.push(Line::from(format!(
                "{mark} rule #{} {on}  {} → {}  {pre}",
                rule.id,
                rule.condition.describe(),
                rule.action.describe()
            )));
        }
        if self.state.automation_rules.is_empty() {
            entries.push(Line::from(
                "Unlock logistics_rules, then hoshi automation add from the CLI. Enter dry-runs.",
            ));
        }
        append_windowed(&mut rows, &entries, self.selected, height)
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

    fn kpi_lines(&self, height: usize) -> Vec<Line<'static>> {
        let snap = self.state.kpi_snapshot();
        let due = snap
            .next_report_at
            .saturating_sub(self.state.last_tick_unix);
        let mut rows = vec![Line::from(format!(
            "[kpi] next report {due}s  {} targets  {} archived",
            snap.targets.len(),
            snap.reports.len()
        ))];
        if let Some(report) = snap.reports.last() {
            rows.push(Line::from(format!(
                "period #{} at {}  pending {}  {}",
                report.id,
                report.at,
                report.pending_decisions,
                report.highlights.first().cloned().unwrap_or_default()
            )));
        }
        let mut entries = Vec::new();
        for (i, row) in snap.measurements.iter().enumerate() {
            let mark = if i == self.selected { '>' } else { ' ' };
            let target = row
                .target
                .map(|t| format!("{t:.1}"))
                .unwrap_or_else(|| "-".into());
            let var = row
                .variance()
                .map(|v| format!("{v:+.1}"))
                .unwrap_or_else(|| "-".into());
            let progress = row
                .progress()
                .map(|p| format!("{:.0}%", p * 100.0))
                .unwrap_or_else(|| "-".into());
            entries.push(Line::from(format!(
                "{mark} {} [{}] {:.1}  target {target}  var {var}  {progress}  events {}",
                row.kpi.as_key(),
                row.owner.as_key(),
                row.value,
                row.event_ids.len()
            )));
        }
        append_windowed(&mut rows, &entries, self.selected, height)
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

    fn finance_lines(&self, height: usize) -> Vec<Line<'static>> {
        let report = self.state.finance_report();
        let cfg = self.state.finance_config();
        let mut rows = vec![Line::from(format!(
            "[fin] cash {:.0}  debt {:.0}  reserve {:.0} (gap {:.0})  period {}s",
            report.cash, report.debt, report.reserve_target, report.reserve_gap, cfg.period_secs
        ))];
        rows.push(Line::from(format!(
            "income {:.0} (prod {:.0} tax {:.0} trade {:.0} ctr {:.0})  expense {:.0} (ind {:.0} rs {:.0} mil {:.0} log {:.0} int {:.0})",
            report.income.total(),
            report.income.production,
            report.income.tax,
            report.income.trade,
            report.income.contract,
            report.expense.total(),
            report.expense.industry,
            report.expense.research,
            report.expense.military,
            report.expense.logistics,
            report.expense.interest
        )));
        let mut entries = Vec::new();
        let mut index = 0usize;
        for pending in &report.pending {
            let mark = if index == self.selected { '>' } else { ' ' };
            index += 1;
            entries.push(Line::from(format!(
                "{mark} hold #{} {:.0} {}  {}",
                pending.id,
                pending.amount,
                pending.center.as_key(),
                pending.kind.summary()
            )));
        }
        for budget in &report.budgets {
            let mark = if index == self.selected { '>' } else { ' ' };
            index += 1;
            let limit = budget
                .limit
                .map(|v| format!("{v:.0}"))
                .unwrap_or_else(|| "∞".into());
            let run = budget.run_rate(report.elapsed_secs, cfg.period_secs);
            entries.push(Line::from(format!(
                "{mark} {} limit {limit}  actual {:.0}  forecast {:.0}  var {:+.0}  run {:.0}",
                budget.center.as_key(),
                budget.actual,
                budget.forecast,
                budget.variance(),
                run
            )));
        }
        append_windowed(&mut rows, &entries, self.selected, height)
    }

    fn staff_lines(&self, height: usize) -> Vec<Line<'static>> {
        let mut rows = vec![Line::from(format!(
            "[stf] {} leaders  empty gov {:.0}%",
            self.state.leaders.len(),
            self.state.config.leaders.empty_governor * 100.0
        ))];
        if self.state.leaders.is_empty() {
            rows.push(Line::from("no leaders"));
            return rows;
        }
        let entries: Vec<Line<'static>> = self
            .state
            .leaders
            .iter()
            .enumerate()
            .map(|(i, leader)| {
                let mark = if i == self.selected { '>' } else { ' ' };
                let assignment = self.state.assignment_of(leader.id);
                let role = assignment.map(|row| row.role.as_key()).unwrap_or("idle");
                let target = assignment
                    .map(|row| row.target.as_key())
                    .unwrap_or_else(|| "-".into());
                let ready = assignment
                    .map(|row| {
                        if row.ready(self.state.last_tick_unix) {
                            "ready".into()
                        } else {
                            format!(
                                "eta {}",
                                row.ready_at.saturating_sub(self.state.last_tick_unix)
                            )
                        }
                    })
                    .unwrap_or_else(|| "unassigned".into());
                let retired = if leader.retired { " retired" } else { "" };
                Line::from(format!(
                    "{mark} #{id} {name}  {trait_id}  {role} {target}  {ready}{retired}",
                    id = leader.id,
                    name = leader.name,
                    trait_id = leader.trait_id
                ))
            })
            .collect();
        append_windowed(&mut rows, &entries, self.selected, height)
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
            render_overlay(frame, " history ", vec![Line::from("no event selected")]);
            return;
        };
        match self.history_pane {
            HistoryPane::List => {}
            HistoryPane::Lineage => {
                let mut lines = vec![Line::from(format!("lineage of #{}", event.id))];
                for row in self.state.history.lineage(event.id) {
                    lines.push(Line::from(format!(
                        "#{} t{} {} {} → {}",
                        row.id,
                        row.at,
                        row.kind.as_key(),
                        row.action,
                        row.outcome
                    )));
                }
                lines.push(Line::from(""));
                lines.push(Line::from(format!(
                    "{} closes",
                    self.keymap.primary(Binding::Cancel)
                )));
                render_overlay(frame, " lineage ", lines);
            }
            HistoryPane::AsOf => {
                let snap = self.state.history.as_of(event.at);
                let mut lines = vec![Line::from(format!("empire at t{}", event.at))];
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
                render_overlay(frame, " time travel ", lines);
            }
        }
    }

    fn draw_report(&self, frame: &mut Frame<'_>) {
        let dest = self.selected_outpost();
        let title = dest
            .map(|c| format!(" spy {c} "))
            .unwrap_or_else(|| " spy ".into());
        let lines = match dest.and_then(|c| self.state.latest_report(c)) {
            Some(report) => report_overlay_lines(report, self.state.last_tick_unix),
            None => vec![Line::from("no spy report for this outpost")],
        };
        render_overlay(frame, title, lines);
    }

    fn wonder_status(&self) -> String {
        match &self.state.wonder {
            Some(job) => {
                let spec = self.state.config.wonders.iter().find(|s| s.id == job.id);
                let name = spec.map(|s| s.name.as_str()).unwrap_or(job.id.as_str());
                let total = spec.map(|s| s.stages.len()).unwrap_or(0);
                let eta = match self.state.wonder_eta_secs() {
                    Some(secs) => format!("{secs}s"),
                    None => "?".into(),
                };
                format!("Wonder: {name}  stage {}/{total}  {eta}", job.stage + 1)
            }
            None => "Wonder: idle".to_string(),
        }
    }

    fn planet_glance_lines(&self) -> Vec<Line<'static>> {
        let selected = self.selected_coord();
        self.state
            .owned_coords()
            .into_iter()
            .map(|coord| {
                let mark = if coord == selected { '>' } else { ' ' };
                let res = self
                    .state
                    .resources_at(coord)
                    .copied()
                    .unwrap_or(Resources::ZERO);
                let active = match self.state.queue_at(coord).and_then(|q| q.first()) {
                    Some(job) => {
                        let left = job
                            .completes_at
                            .unwrap_or(self.state.last_tick_unix)
                            .saturating_sub(self.state.last_tick_unix);
                        format!("{} {left}s", job.building)
                    }
                    None => "idle".to_string(),
                };
                let moon = if self.state.moon_at(coord).is_some() {
                    "  moon"
                } else {
                    ""
                };
                Line::from(format!(
                    "{mark} {coord}  {}  {active}{moon}",
                    stockpile_label(res)
                ))
            })
            .collect()
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

fn fleet_bar(n: u32) -> String {
    "*".repeat(n.min(40) as usize)
}

fn push_away_section(lines: &mut Vec<Line<'static>>, title: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    lines.push(Line::from(format!("{title}:")));
    for item in items.iter().take(3) {
        lines.push(Line::from(item.clone()));
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
        Rect {
            x: body.x + 4,
            y: body.y + 1,
            width: body.width - 8,
            height: body.height - 2,
        }
    } else {
        body
    }
}

fn detail_column_width(total: u16) -> u16 {
    total
        .saturating_sub(24)
        .clamp(28, 48)
        .min(total.saturating_sub(10))
}

fn render_overlay(frame: &mut Frame<'_>, title: impl Into<String>, lines: Vec<Line<'static>>) {
    let area = frame.area();
    frame.render_widget(Clear, area);
    let block = Block::bordered().title(title.into());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(lines), inner);
}

fn report_overlay_lines(report: &SpyReport, now: u64) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(format!(
            "{}  age {}s  score {:.0}",
            report.tier,
            report.age_secs(now),
            report.score
        )),
        Line::from(format!(
            "detected {}  lost {}/{}",
            if report.detected { "yes" } else { "no" },
            report.probes_lost,
            report.probes_sent
        )),
    ];
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

fn cost_label(cost: Resources) -> String {
    ResourceKind::ALL
        .iter()
        .copied()
        .filter(|&kind| cost.get(kind) > 0.5 || ResourceKind::QUOTED.contains(&kind))
        .map(|kind| format!("{}{:.0}", kind.abbr(), cost.get(kind)))
        .collect::<Vec<_>>()
        .join(" ")
}

fn missing_label(missing: Resources) -> String {
    let parts: Vec<String> = ResourceKind::ALL
        .iter()
        .copied()
        .filter(|&kind| missing.get(kind) > 0.5)
        .map(|kind| format!("{}{:.0}", kind.abbr(), missing.get(kind)))
        .collect();
    if parts.is_empty() {
        "need resources".to_string()
    } else {
        format!("need {}", parts.join(" "))
    }
}

fn styled_row(text: String, reason: Option<String>) -> Line<'static> {
    match reason {
        None => Line::from(text),
        Some(reason) => Line::from(Span::styled(
            format!("{text}  {reason}"),
            Style::default().add_modifier(Modifier::DIM),
        )),
    }
}

fn append_windowed(
    rows: &mut Vec<Line<'static>>,
    entries: &[Line<'static>],
    selected: usize,
    height: usize,
) -> Vec<Line<'static>> {
    let used = rows.len();
    let avail = height.saturating_sub(used);
    if avail == 0 {
        return rows.clone();
    }
    let len = entries.len();
    if len == 0 {
        return std::mem::take(rows);
    }
    let start = if len <= avail {
        0
    } else {
        let mut start = selected.saturating_sub(avail / 2);
        if start + avail > len {
            start = len - avail;
        }
        start
    };
    rows.extend(entries.iter().skip(start).take(avail).cloned());
    std::mem::take(rows)
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

fn sparkline(values: &[f64], width: usize) -> String {
    crate::cockpit::sparkline(values, width)
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
    fn away_overlay_dismisses_and_reopens_from_overview() {
        let mut app = App::new(GameState::new(1_000));
        app.set_away(crate::title::AwaySummary {
            elapsed_secs: 120,
            iron: 10.0,
            builds: 1,
            events: vec!["raid: lost".into()],
            ..crate::title::AwaySummary::default()
        });
        let text = render_text(&app, 80, 24);
        assert!(text.contains("while you were away"), "{text}");
        assert!(text.contains("elapsed 120s"), "{text}");
        assert!(
            !text.contains("Armour Technology"),
            "away overlay left the research list visible:\n{text}"
        );
        app.handle_key(KeyCode::Enter, 1_000);
        let text = render_text(&app, 80, 24);
        assert!(!text.contains("while you were away"), "{text}");
        app.handle_key(KeyCode::Char('w'), 1_000);
        let text = render_text(&app, 80, 24);
        assert!(text.contains("while you were away"), "{text}");
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
        assert!(text.contains("deliveries:"), "{text}");
        assert!(text.contains("delivered 100 metal to 1:1:2"), "{text}");
        assert!(text.contains("stalls:"), "{text}");
        assert!(text.contains("stall: energy deficit at 1:1:1"), "{text}");
        assert!(text.contains("completed:"), "{text}");
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
            assert!(text.contains("Build:"), "{w}x{h}: {text}");
            assert!(text.contains("Research:"), "{w}x{h}: {text}");
            assert!(text.contains("Wonder:"), "{w}x{h}: {text}");
            assert!(
                !text.contains('\u{1b}'),
                "{w}x{h} leaked an escape sequence: {text:?}"
            );
        }
        let mut app = app;
        app.open(Screen::Shipyard);
        let fleet = render_text(&app, 60, 20);
        assert!(fleet.contains("Ships"), "{fleet}");
        assert!(fleet.contains("Raid:"), "{fleet}");
        app.open(Screen::Prestige);
        let records = render_text(&app, 60, 20);
        assert!(records.contains("Prestige:"), "{records}");
    }

    #[test]
    fn material_labels_fit_80_120_and_160() {
        let app = App::new(GameState::new(1_000));
        for (w, h) in [(80, 24), (120, 40), (160, 48)] {
            let text = render_text(&app, w, h);
            for needle in ["Fe", "Si", "REE", "H2O", "He3", "D", "Energy", "heat", "/h"] {
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
    fn tree_view_renders_at_80x24_and_shows_node_states() {
        let mut state = GameState::new(1_000);
        state.buildings.research_lab = 1;
        state.tech_levels.insert("energy".into(), 1);
        state
            .exclusive_picks
            .insert("mining_method".into(), "deep_core".into());
        let mut app = App::new(state);
        let small = render_text(&app, 80, 24);
        assert!(small.contains("[industry]"), "{small}");
        assert!(
            small.contains("locked") || small.contains("available") || small.contains("researched"),
            "{small}"
        );
        let text = render_text(&app, 120, 40);
        assert!(text.contains("[industry]"), "{text}");
        assert!(text.contains("military"), "{text}");
        assert!(text.contains("society"), "{text}");
        assert!(text.contains("researched"), "{text}");
        assert!(text.contains("Energy Technology"), "{text}");
        assert!(text.contains("excluded"), "{text}");
        assert!(text.contains("keystone"), "{text}");
        assert!(!text.contains("ancient"), "{text}");
        app.handle_key(KeyCode::Char('l'), 1_000);
        assert_eq!(app.tree_branch, TechBranch::Military);
        let military = render_text(&app, 120, 40);
        assert!(military.contains("[military]"), "{military}");
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
        assert!(text.contains("in_progress"), "{text}");
    }

    #[test]
    fn disabled_portraits_use_ascii_and_keep_layout() {
        let mut app = App::new(GameState::new(1_000));
        app.portraits.enabled = false;
        let text = render_text(&app, 80, 24);
        assert!(text.contains("Fe"), "{text}");
        assert!(text.contains(".==."), "{text}");
        assert!(!text.contains('▀'), "{text}");
        app.open(Screen::Planet);
        let planet = render_text(&app, 80, 24);
        assert!(
            planet.contains("(::.") || planet.contains("rocky"),
            "{planet}"
        );
        app.state.events.push(hoshi_core::GameEvent {
            at: 1,
            text: "raid: pirates 1 vs 0. 1 rounds. lost none. pirates lost 1. plunder Fe0 Si0 D0"
                .into(),
        });
        app.handle_key(KeyCode::Tab, 1_000);
        app.screen = Screen::Overview;
        let raid = render_text(&app, 80, 24);
        assert!(raid.contains("|o-|") || raid.contains("|O-|"), "{raid}");
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
        assert!(text.contains("combat replay"), "{text}");
        assert!(text.contains("pirates"), "{text}");
        assert!(text.contains("you"), "{text}");
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
            raids: 1,
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
        assert!(text.contains("combat replay"), "{text}");
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
        assert!(text.contains(&format!("{build_left_later}s")), "{text}");

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
        assert!(text.contains("Research: energy"), "{text}");
        assert!(text.contains(&format!("{research_left_later}s")), "{text}");
    }

    #[test]
    fn unaffordable_items_are_dimmed_with_missing_amount() {
        let app = App::new(GameState::new(1_000));
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();
        let text = buffer_text(terminal.backend());
        assert!(
            text.contains("need"),
            "overview should show missing: {text}"
        );
        assert!(
            line_is_dim(terminal.backend(), "need"),
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
            text.contains("need D") || text.contains("need Fe"),
            "planet should show missing resources: {text}"
        );
        assert!(
            line_is_dim(terminal.backend(), "research lab")
                || line_is_dim(terminal.backend(), "need"),
            "unaffordable building should be dim: {text}"
        );
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
        assert!(text.contains("light fighter"), "{text}");
        assert!(text.contains("battleship"), "{text}");
        assert!(text.contains("x4"), "{text}");
        assert!(
            text.contains("need shipyard") || text.contains("need "),
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
        assert!(text.contains("1. light fighter x1"), "{text}");
        assert!(text.contains("Queue 1/5"), "{text}");
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
            text.contains("need shipyard"),
            "shipyard should show shipyard requirement: {text}"
        );
        assert!(
            line_is_dim(terminal.backend(), "light fighter"),
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
            text.contains("need Fe") || text.contains("need Si"),
            "unaffordable ships should show missing amounts: {text}"
        );
        assert!(
            line_is_dim(terminal.backend(), "light fighter"),
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
        assert!(text.contains("light cannon"), "{text}");
        assert!(text.contains("orbital station"), "{text}");
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
        assert!(text.contains("1. light cannon x1"), "{text}");
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
        assert!(text.contains("1:1:2"), "{text}");
        assert!(text.contains("Fe 10000"), "{text}");

        app.handle_key(KeyCode::Char(']'), 1_000);
        assert_eq!(app.selected_coord(), dest);
        app.open(Screen::Planet);
        assert_eq!(app.screen, Screen::Planet);
        app.handle_key(KeyCode::Enter, 1_000);
        assert!(app.state.build_queue.is_empty());
        assert_eq!(app.state.colonies[0].build_queue.len(), 1);
        let text = render_text(&app, 100, 32);
        assert!(text.contains("Planet 1:1:2"), "{text}");
        assert!(text.contains("metal mine"), "{text}");
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
        assert!(text.contains("iron_ore"), "{text}");
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
        assert!(planet.contains("radiator"), "{planet}");
        assert!(planet.contains("scrubber"), "{planet}");
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
            text.contains(&format!("need Fe{:.0}", remaining.iron)),
            "{text}"
        );
        assert!(text.contains(&format!("eta {eta}s")), "{text}");
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
        let mut app = App::new(state);
        for _ in 0..4 {
            app.handle_key(KeyCode::Char('1'), 1_001);
        }
        assert_eq!(app.screen, Screen::Achievements);
        let text = render_text(&app, 100, 32);
        assert!(text.contains("Bridge / Feats"), "{text}");
        assert!(text.contains("First cut"), "{text}");
        assert!(text.contains("earned"), "{text}");
        assert!(text.contains("???"), "{text}");
        assert!(
            text.contains("1/5") || text.contains("Solar array"),
            "{text}"
        );
        assert!(text.contains("achievement: First cut"), "{text}");
    }

    #[test]
    fn daily_screen_lists_progress_and_reset() {
        let mut app = App::new(GameState::new(1_000));
        app.open(Screen::Daily);
        assert_eq!(app.screen, Screen::Daily);
        let text = render_text(&app, 100, 32);
        assert!(text.contains("Bridge / Daily"), "{text}");
        assert!(text.contains("Streak"), "{text}");
        assert!(text.contains("resets in"), "{text}");
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
        assert!(text.contains("Pirate homeworld"), "{text}");
        assert!(text.contains("Ladder"), "{text}");
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
        assert!(text.contains("rule #1"), "{text}");
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
        assert!(text.contains("[fin]"), "{text}");
        assert!(text.contains("industry"), "{text}");
        assert!(text.contains("hold #"), "{text}");
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
        assert!(text.contains("output"), "{text}");
        app.handle_key(KeyCode::Enter, 1_000);
        assert!(
            app.status.contains("events") || app.status.contains("no source"),
            "{}",
            app.status
        );
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
        assert!(text.contains("victory"), "{text}");
        assert!(text.contains("wonder"), "{text}");
        assert!(!text.contains("Colony / Research"), "{text}");
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
        assert!(text.contains("limit"), "{text}");
        assert!(text.contains("energy"), "{text}");
        assert!(text.contains("heat"), "{text}");
        assert!(text.contains("REE"), "{text}");
        assert!(text.contains("H2O"), "{text}");
        assert!(text.contains("He3"), "{text}");
        let wide = render_text(&app, 120, 32);
        assert!(wide.contains("limit"), "{wide}");
        assert!(
            text.chars().any(|c| "▁▂▃▄▅▆▇█".contains(c)) || text.contains("rate"),
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
            !text.contains("Colony / Research"),
            "overlay left the screen chrome visible:\n{text}"
        );
        assert!(
            !text.contains("Armour Technology"),
            "overlay left the research list visible:\n{text}"
        );
        assert!(
            !text.contains("REE"),
            "overlay left resource lines visible:\n{text}"
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
        assert!(text.contains("Prices"), "{text}");
        assert!(text.contains("preview"), "{text}");
        assert!(
            text.chars().any(|c| "▁▂▃▄▅▆▇█".contains(c)),
            "sparkline missing: {text}"
        );
        app.handle_key(KeyCode::Char('+'), 1_000);
        let text = render_text(&app, 100, 32);
        assert!(text.contains("amount 20"), "{text}");
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
            text.contains("Neutral") || text.contains("Dormant"),
            "{text}"
        );
        assert!(text.contains("next"), "{text}");
        assert!(text.contains("Recent: none"), "{text}");
        app.handle_key(KeyCode::Char('j'), 1_000);
        app.handle_key(KeyCode::Enter, 1_000);
        close_standing(&app, "pirates", 12.0);
        let text = render_text(&app, 100, 32);
        assert!(text.contains("tribute"), "{text}");
        assert!(
            text.contains("truce") || text.contains("Truce") || text.contains("Neutral"),
            "{text}"
        );
        assert!(text.contains("Recent:"), "{text}");
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
        assert!(text.contains("Metal levy"), "{text}");
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
        });
        let mut app = App::new(state);
        app.open(Screen::Reports);
        assert_eq!(app.screen, Screen::Reports);
        let text = render_text(&app, 100, 32);
        assert!(text.contains("Fleet / Intel"), "{text}");
        assert!(text.contains(&dest.to_string()), "{text}");
        assert!(text.contains("age 100s"), "{text}");
        assert!(text.contains("garrison"), "{text}");
        app.handle_key(KeyCode::Enter, 1_000);
        let text = render_text(&app, 100, 32);
        assert!(text.contains(&format!("spy {dest}")), "{text}");
        assert!(
            !text.contains("Fleet / Intel"),
            "spy overlay left the reports chrome visible:\n{text}"
        );
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
        assert!(text.contains("governor"), "{text}");
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
        assert!(small.contains("Map"), "{small}");
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
        let overview = render_text(&app, 100, 28);
        assert!(overview.contains("moon"), "{overview}");
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
        assert!(text.contains("[H]f"), "{text}");
        assert!(
            text.contains(&format!("fleet #{id} outbound {eta}s")),
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
        assert!(overlay.contains("small_cargo"), "{overlay}");
        assert!(
            !overlay.contains("Fleet / Star map"),
            "dispatch overlay left the map chrome visible:\n{overlay}"
        );
        app.handle_key(KeyCode::Char('+'), 1_000);
        let preview = render_text(&app, 100, 32);
        assert!(preview.contains("ETA"), "{preview}");
        app.handle_key(KeyCode::Enter, 1_000);
        assert!(app.dispatch.is_none());
        assert_eq!(app.state.missions.len(), 1);
        assert_eq!(app.state.missions[0].dest.to_string(), "1:1:2");
        let text = render_text(&app, 100, 32);
        assert!(text.contains("Fleet / Star map"), "{text}");
        assert!(text.contains("fleet #"), "{text}");
        assert!(text.contains("outbound"), "{text}");
        assert!(text.contains("launched to 1:1:2"), "{text}");
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
        state.outposts.push(Outpost {
            coord: dest,
            kind: PlanetType::Rocky,
            size: 8,
            defences: Fleet::default(),
            garrison: 2,
            rare: None,
            last_rebuild_at: 0,
            homeworld: false,
        });
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
        assert!(overlay.contains("invasion approval"), "{overlay}");
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
        app.handle_key(KeyCode::Char('g'), 1_000);
        assert_eq!(app.screen, Screen::Overview);
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
            !panel.contains("Bridge / Daily"),
            "advisor overlay left the chrome visible:\n{panel}"
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
            !panel.contains("Colony / Research"),
            "activity overlay left the chrome visible:\n{panel}"
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
        assert!(full.contains("alerts"), "{full}");
        for tile in ["MAP", "GAUGES", "QUEUES", "FLEETS", "TIMELINE"] {
            assert!(full.contains(tile), "missing {tile}: {full}");
        }
        let compact = render_text(&app, 120, 40);
        assert!(compact.contains("alerts"), "{compact}");
        assert!(
            compact.contains("GAUGES") || compact.contains("QUEUES"),
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
        assert!(after.contains("alerts"), "{after}");
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

    #[test]
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
        let started = std::time::Instant::now();
        let _ = render_text(&app, 160, 48);
        let budget = if cfg!(debug_assertions) { 0.050 } else { 0.016 };
        assert!(
            started.elapsed().as_secs_f64() < budget,
            "cockpit render took {:?}",
            started.elapsed()
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
            &["DETAIL", "metal mine", "COST", "iron", "energy draw"],
        );
        assert_detail_on(&app, 120, 40, &["DETAIL", "metal mine", "affordable"]);
        app.screen = Screen::Overview;
        app.selected = app
            .overview_techs()
            .iter()
            .position(|spec| spec.id == "energy")
            .unwrap();
        assert_detail_on(&app, 160, 48, &["DETAIL", "Energy Technology", "REQUIRES"]);
        assert_detail_on(&app, 120, 40, &["Energy Technology"]);
        app.screen = Screen::Shipyard;
        app.selected = 0;
        assert_detail_on(&app, 160, 48, &["DETAIL", "speed", "fuel"]);
        assert_detail_on(&app, 120, 40, &["speed"]);
        app.screen = Screen::Laws;
        app.selected = 0;
        let laws = render_text(&app, 160, 48);
        assert!(laws.contains("DETAIL"), "{laws}");
        app.screen = Screen::Map;
        let map = render_text(&app, 160, 48);
        assert!(map.contains("DETAIL"), "{map}");
        assert!(map.contains("1:1:1") || map.contains("size"), "{map}");
        let map_mid = render_text(&app, 120, 40);
        assert!(map_mid.contains("DETAIL"), "{map_mid}");
    }

    #[test]
    fn detail_overlay_opens_at_80x24() {
        let mut app = detail_app();
        app.screen = Screen::Planet;
        let hidden = render_text(&app, 80, 24);
        assert!(!hidden.contains("energy draw"), "{hidden}");
        app.handle_key(KeyCode::Char('d'), 1_000);
        let text = render_text(&app, 80, 24);
        assert!(text.contains("detail"), "{text}");
        assert!(text.contains("metal mine"), "{text}");
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
        assert!(text.contains("never at current income"), "{text}");
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
        app.handle_key(KeyCode::Char('1'), 1_000);
        assert_eq!(app.screen, Screen::Cockpit);
        let text = render_text(&app, 80, 24);
        assert!(text.contains("BRIDGE"), "{text}");
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
        assert!(small.contains("esc bridge"), "{small}");
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
}
