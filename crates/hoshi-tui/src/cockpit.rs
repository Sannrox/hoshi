//! Dense command-center screen. Core still owns the rules; this only renders.

use crate::ui::chrome::{Tile, fill};
use crate::ui::panels::Callout;
use crate::ui::spans::{countdown, duration};
use crate::ui::{Intent, Surface, Theme};
use hoshi_core::{
    Actor, Coord, GameState, HistoryKind, MissionPhase, ResourceKind, Resources, RoleKind, ShipKind,
};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout as TuiLayout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

pub const OVERVIEW_PANELS: &str = "map,queues,fleets,timeline";
pub const ECONOMY_PANELS: &str = "gauges,kpis,queues,market";
pub const WAR_PANELS: &str = "map,fleets,raids,tactics";

const MAP_SYSTEM_MAX: u16 = 81;
const MAP_SLOT_COUNT: u16 = 16;
const GAUGE_CAP: usize = 48;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CockpitLayout {
    Overview,
    Economy,
    War,
}

impl CockpitLayout {
    pub fn as_key(self) -> &'static str {
        match self {
            Self::Overview => "overview",
            Self::Economy => "economy",
            Self::War => "war",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "overview" => Self::Overview,
            "economy" => Self::Economy,
            "war" => Self::War,
            _ => return None,
        })
    }

    pub fn next(self) -> Self {
        match self {
            Self::Overview => Self::Economy,
            Self::Economy => Self::War,
            Self::War => Self::Overview,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Map,
    Gauges,
    Queues,
    Fleets,
    Timeline,
    Market,
    Kpis,
    Raids,
    Tactics,
}

impl Panel {
    pub fn title(self) -> &'static str {
        match self {
            Self::Map => "map",
            Self::Gauges => "gauges",
            Self::Queues => "queues",
            Self::Fleets => "fleets",
            Self::Timeline => "timeline",
            Self::Market => "market",
            Self::Kpis => "kpis",
            Self::Raids => "raids",
            Self::Tactics => "tactics",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        Some(match key.trim() {
            "map" => Self::Map,
            "gauges" => Self::Gauges,
            "queues" => Self::Queues,
            "fleets" => Self::Fleets,
            "timeline" => Self::Timeline,
            "market" => Self::Market,
            "kpis" => Self::Kpis,
            "raids" => Self::Raids,
            "tactics" => Self::Tactics,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Planet(Coord),
    Fleet { mission: Option<u64> },
    Faction(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AlertKind {
    Decision,
    Story,
    Raid,
    Defence,
    Hostile,
    Power,
    IdleQueue,
    Storage,
}

impl AlertKind {
    fn intent(self) -> Intent {
        match self {
            Self::Decision | Self::Raid | Self::Defence => Intent::Danger,
            Self::Story | Self::Hostile | Self::Power | Self::Storage => Intent::Warning,
            Self::IdleQueue => Intent::Primary,
        }
    }

    fn severity(self) -> u8 {
        match self {
            Self::Decision | Self::Raid => 0,
            Self::Story | Self::Defence => 1,
            Self::Hostile => 2,
            Self::Power => 3,
            Self::IdleQueue => 4,
            Self::Storage => 5,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alert {
    kind: AlertKind,
    pub text: String,
    pub detail: String,
    pub jump: Target,
    /// Set on the expiring-decision row. The same value `hoshi state` prints.
    pub decision: Option<hoshi_core::ExpiringDecision>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Presets {
    pub overview: Vec<Panel>,
    pub economy: Vec<Panel>,
    pub war: Vec<Panel>,
}

impl Default for Presets {
    fn default() -> Self {
        Self {
            overview: parse_panels(OVERVIEW_PANELS),
            economy: parse_panels(ECONOMY_PANELS),
            war: parse_panels(WAR_PANELS),
        }
    }
}

impl Presets {
    pub fn parse(overview: &str, economy: &str, war: &str) -> Self {
        let mut presets = Self::default();
        let overview_panels = parse_panels(overview);
        if !overview_panels.is_empty() {
            presets.overview = overview_panels;
        }
        let economy_panels = parse_panels(economy);
        if !economy_panels.is_empty() {
            presets.economy = economy_panels;
        }
        let war_panels = parse_panels(war);
        if !war_panels.is_empty() {
            presets.war = war_panels;
        }
        presets
    }

    fn for_layout(&self, layout: CockpitLayout) -> &[Panel] {
        match layout {
            CockpitLayout::Overview => &self.overview,
            CockpitLayout::Economy => &self.economy,
            CockpitLayout::War => &self.war,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cockpit {
    pub layout: CockpitLayout,
    pub presets: Presets,
    focus: usize,
    pub(crate) selected: usize,
    stack: Vec<Target>,
    layout_dirty: bool,
    /// Victory focus from `config.toml`. Set drops idle-queue rows for non-home colonies.
    pub victory_focus: Option<hoshi_core::VictoryPath>,
}

impl Default for Cockpit {
    fn default() -> Self {
        Self {
            layout: CockpitLayout::Overview,
            presets: Presets::default(),
            focus: 0,
            selected: 0,
            stack: Vec::new(),
            layout_dirty: false,
            victory_focus: None,
        }
    }
}

impl Cockpit {
    pub fn take_layout_dirty(&mut self) -> bool {
        let dirty = self.layout_dirty;
        self.layout_dirty = false;
        dirty
    }

    pub fn cycle_layout(&mut self) {
        self.layout = self.layout.next();
        self.focus = 0;
        self.selected = 0;
        self.layout_dirty = true;
    }

    pub fn panels(&self) -> &[Panel] {
        self.presets.for_layout(self.layout)
    }

    pub(crate) fn focused_panel(&self) -> Option<Panel> {
        let panels = self.panels();
        if panels.is_empty() {
            return None;
        }
        Some(panels[self.focus.min(panels.len() - 1)])
    }

    pub fn nudge_focus(&mut self, delta: i32) {
        let n = self.panels().len();
        if n == 0 {
            return;
        }
        let i = self.focus.min(n - 1) as i32;
        self.focus = (i + delta).rem_euclid(n as i32) as usize;
        self.selected = 0;
    }

    pub fn nudge_select(&mut self, delta: i32, len: usize) {
        if len == 0 {
            self.selected = 0;
            return;
        }
        let i = self.selected.min(len - 1) as i32;
        self.selected = (i + delta).rem_euclid(len as i32) as usize;
    }

    pub fn confirm(
        &mut self,
        alerts: &[Alert],
        fleets: &[Target],
        planets: &[Coord],
        cursor: Coord,
    ) {
        if let Some(target) = self.stack.last().cloned() {
            if let Some(next) = self.link_from(&target, fleets, planets) {
                self.stack.push(next);
                self.selected = 0;
            }
            return;
        }
        match self.focused_panel() {
            Some(Panel::Map) => {
                self.stack.push(Target::Planet(cursor));
                self.selected = 0;
            }
            Some(Panel::Fleets) => {
                if let Some(target) = fleets.get(self.selected).cloned() {
                    self.stack.push(target);
                    self.selected = 0;
                }
            }
            Some(Panel::Queues) => {
                if let Some(&coord) = planets.get(self.selected) {
                    self.stack.push(Target::Planet(coord));
                    self.selected = 0;
                }
            }
            _ => {
                if let Some(alert) = alerts.get(self.selected) {
                    self.stack.push(alert.jump.clone());
                    self.selected = 0;
                }
            }
        }
    }

    pub fn back(&mut self) -> bool {
        self.stack.pop().is_some()
    }

    pub fn top(&self) -> Option<&Target> {
        self.stack.last()
    }

    fn link_from(&self, target: &Target, fleets: &[Target], planets: &[Coord]) -> Option<Target> {
        match target {
            Target::Planet(coord) => fleets
                .iter()
                .filter(|row| matches!(row, Target::Fleet { .. }))
                .nth(self.selected)
                .cloned()
                .or_else(|| {
                    planets
                        .iter()
                        .filter(|c| *c != coord)
                        .nth(self.selected)
                        .copied()
                        .map(Target::Planet)
                }),
            Target::Fleet { .. } => planets.first().copied().map(Target::Planet),
            Target::Faction(_) => planets.first().copied().map(Target::Planet),
        }
    }
}

pub fn parse_panels(spec: &str) -> Vec<Panel> {
    let mut out = Vec::new();
    for part in spec.split(',') {
        if let Some(panel) = Panel::from_key(part)
            && !out.contains(&panel)
        {
            out.push(panel);
        }
    }
    out
}

/// Bridge rows, most urgent first. A `focus` drops idle-queue rows for
/// colonies other than home; the expiring decision still leads.
pub fn alerts(state: &GameState, focus: Option<hoshi_core::VictoryPath>) -> Vec<Alert> {
    let mut rows = Vec::new();
    let home = state.home;
    if let Some(secs) = state.raid_countdown_secs() {
        let ships = state.scheduled_raid_ships();
        rows.push(Alert {
            kind: AlertKind::Raid,
            text: format!("Incoming raid  T-{}", countdown(secs)),
            detail: format!(
                "{ships} attackers · formation {} · stance {} · pirates → {home}",
                state.fleet_tactics.formation.as_key(),
                state.fleet_tactics.stance.as_key()
            ),
            jump: Target::Planet(home),
            decision: None,
        });
    }
    // The latest raid stays on the bridge until the next one replaces it.
    if let Some(log) = state
        .combat_logs
        .iter()
        .rev()
        .find(|log| log.raid_plunder.is_some())
        && log.report.winner == hoshi_core::CombatWinner::Attacker
    {
        let taken = log.raid_plunder.unwrap_or(hoshi_core::Resources::ZERO);
        rows.push(Alert {
            kind: AlertKind::Raid,
            text: format!(
                "Raid lost  {} ago",
                duration(state.last_tick_unix.saturating_sub(log.at))
            ),
            detail: format!("plunder {} · r report", taken.brief()),
            jump: Target::Planet(home),
            decision: None,
        });
        if let Some(losses) = log.raid_losses() {
            for line in raid_loss_lines(losses) {
                let (text, detail) = line.split_once(": ").unwrap_or((&line, ""));
                rows.push(Alert {
                    kind: AlertKind::Raid,
                    text: text.to_string(),
                    detail: detail.to_string(),
                    jump: Target::Planet(home),
                    decision: None,
                });
            }
        }
    }
    // Undefended stays true between raids, not only inside the warning window.
    // Docked ships with weapons fight a raid too, so they count as defence.
    let armed = hoshi_core::ShipKind::ALL
        .iter()
        .any(|&kind| state.fleet.get(kind) > 0 && state.config.ships.get(kind).attack > 0.0);
    if state.next_raid_in_secs().is_some() && state.defences.is_empty() && !armed {
        let ships = state.fleet.total();
        let detail = match ships {
            0 => format!("no ships · 0 defences at {home}"),
            1 => format!("1 unarmed ship · 0 defences at {home}"),
            n => format!("{n} unarmed ships · 0 defences at {home}"),
        };
        rows.push(Alert {
            kind: AlertKind::Defence,
            text: "Home undefended".into(),
            detail,
            jump: Target::Planet(home),
            decision: None,
        });
    }
    if state.pending_tactical.is_some() {
        rows.push(Alert {
            kind: AlertKind::Raid,
            text: "Tactical combat pending".into(),
            detail: "orders wait for you".into(),
            jump: Target::Planet(home),
            decision: None,
        });
    }
    for spec in state.config.diplomacy.iter() {
        let standing = state.diplomacy.standing(&spec.id);
        if standing < -20.0 {
            rows.push(Alert {
                kind: AlertKind::Hostile,
                text: format!("Hostile {}", spec.name),
                detail: format!("standing {standing:.0}"),
                jump: Target::Faction(spec.id.clone()),
                decision: None,
            });
        }
    }
    if state.build_queue.is_empty() && state.research.is_none() {
        rows.push(Alert {
            kind: AlertKind::IdleQueue,
            text: "Queues idle".into(),
            detail: format!("{home} · build and research"),
            jump: Target::Planet(home),
            decision: None,
        });
    }
    for colony in state.colonies.iter().filter(|_| focus.is_none()) {
        if colony.build_queue.is_empty() {
            rows.push(Alert {
                kind: AlertKind::IdleQueue,
                text: "Queue idle".into(),
                detail: colony.coord.to_string(),
                jump: Target::Planet(colony.coord),
                decision: None,
            });
        }
    }
    // Shortfalls slow every chain at home, so they belong on the bridge,
    // not only in a tutorial hint.
    let energy = state.energy_balance();
    if energy < 0.0 {
        rows.push(Alert {
            kind: AlertKind::Power,
            text: format!("Energy short {energy:.0}"),
            detail: format!("production slows at {home} · build solar"),
            jump: Target::Planet(home),
            decision: None,
        });
    }
    let waste = state.waste_home();
    if waste.heat_sink > 0.0 && waste.heat_load > waste.heat_sink {
        rows.push(Alert {
            kind: AlertKind::Power,
            text: format!(
                "Heat {:.0}/{:.0} ({:.0}%)",
                waste.heat_load,
                waste.heat_sink,
                100.0 * waste.heat_load / waste.heat_sink
            ),
            detail: format!("work slows at {home} · build radiators"),
            jump: Target::Planet(home),
            decision: None,
        });
    }
    for kind in ResourceKind::ALL {
        let cap = state.storage_cap(kind);
        let stock = state.resources.get(kind);
        if cap > 0.0 && stock / cap >= 0.95 {
            rows.push(Alert {
                kind: AlertKind::Storage,
                text: format!("{} storage full", kind.abbr()),
                detail: format!("{:.0}% of cap at {home}", 100.0 * stock / cap),
                jump: Target::Planet(home),
                decision: None,
            });
        }
    }
    rows.sort_by_key(|row| row.kind.severity());
    if let Some(decision) = hoshi_core::expiring_decision(state) {
        let left = decision.deadline.saturating_sub(state.last_tick_unix);
        let also = match decision.action {
            hoshi_core::DecisionAction::SetStance => " · set_formation",
            hoshi_core::DecisionAction::ArmHome => " · arm the home",
            _ => "",
        };
        rows.insert(
            0,
            Alert {
                kind: AlertKind::Decision,
                text: format!("{}  T-{}", decision.action.as_key(), countdown(left)),
                detail: format!(
                    "{}{also} · deadline {}",
                    decision.id.as_key(),
                    decision.deadline
                ),
                jump: Target::Planet(home),
                decision: Some(decision),
            },
        );
    }
    if let Some(note) = state.story_bridge_note() {
        let at = usize::from(rows.first().is_some_and(|row| row.decision.is_some()));
        rows.insert(
            at,
            Alert {
                kind: AlertKind::Story,
                text: note.to_string(),
                detail: "story".into(),
                jump: Target::Planet(home),
                decision: None,
            },
        );
    }
    rows
}

/// The two lines a raid on home leads with: hulls destroyed (never rebuilt),
/// then defences the repair crews returned and batteries still missing.
pub fn raid_loss_lines(losses: hoshi_core::RaidLosses) -> [String; 2] {
    let ships = if losses.ships_lost.is_empty() {
        "Ships lost: none".to_string()
    } else {
        format!("Ships lost: {} (gone for good)", losses.ships_lost.label())
    };
    let missing = losses
        .batteries_missing
        .map(|n| format!(" · batteries still missing {n}"))
        .unwrap_or_default();
    [
        ships,
        format!("Defences returned: {}{missing}", losses.defences_returned),
    ]
}

/// Battle wreckage a recycler can still lift at `coord`.
fn debris_at(state: &GameState, coord: Coord) -> f64 {
    state.debris.get(&coord).copied().unwrap_or(0.0)
}

pub fn fleet_targets(state: &GameState) -> Vec<Target> {
    let mut rows = vec![Target::Fleet { mission: None }];
    for mission in &state.missions {
        rows.push(Target::Fleet {
            mission: Some(mission.id),
        });
    }
    rows
}

pub fn planet_targets(state: &GameState) -> Vec<Coord> {
    state.owned_coords()
}

pub fn size_mode(width: u16, height: u16) -> SizeMode {
    if width >= 160 && height >= 48 {
        SizeMode::Full
    } else if width >= 120 && height >= 40 {
        SizeMode::Compact
    } else {
        SizeMode::Tabbed
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizeMode {
    Full,
    Compact,
    Tabbed,
}

pub struct View<'a> {
    pub theme: &'a Theme,
    pub state: &'a GameState,
    pub cockpit: &'a Cockpit,
    pub map_system: u16,
    pub map_position: u16,
    pub gauges: &'a [Resources],
}

pub fn draw(
    frame: &mut Frame<'_>,
    area: Rect,
    view: View<'_>,
    footer: Vec<Line<'static>>,
    alert_index: usize,
) {
    let screen = frame.area();
    let mode = size_mode(screen.width, screen.height);
    let t = view.theme;
    let alert_rows = alerts(view.state, view.cockpit.victory_focus);
    let panels = view.cockpit.panels();
    fill(frame.buffer_mut(), area, t.surface(Surface::Base));
    let top = if mode == SizeMode::Tabbed { 2 } else { 0 };
    let [strip, body, foot] = TuiLayout::vertical([
        Constraint::Length(top),
        Constraint::Min(3),
        Constraint::Length(footer.len() as u16),
    ])
    .areas(area);
    frame.render_widget(Paragraph::new(footer).style(t.muted()), foot);
    if mode == SizeMode::Tabbed {
        frame.render_widget(
            Paragraph::new(alert_lines(&alert_rows, alert_index, t, mode)),
            strip,
        );
    }
    if let Some(target) = view.cockpit.stack.last() {
        frame.render_widget(
            Tile {
                theme: t,
                title: "detail".into(),
                right: None,
                focused: true,
            },
            body,
        );
        let inner = Tile::inner(body);
        frame.render_widget(
            Paragraph::new(detail_lines(view.state, target, inner.height as usize, t)),
            inner,
        );
        return;
    }
    match mode {
        SizeMode::Tabbed => {
            let panel = view.cockpit.focused_panel().unwrap_or(Panel::Queues);
            let [tabs, rest] =
                TuiLayout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(body);
            frame.render_widget(
                Paragraph::new(tab_line(panels, view.cockpit.focus, t)),
                tabs,
            );
            render_panel(frame, rest, panel, true, &view);
        }
        SizeMode::Compact | SizeMode::Full => {
            render_bridge(
                frame,
                body,
                &view,
                &alert_rows,
                alert_index,
                mode == SizeMode::Full,
            );
        }
    }
}

fn render_bridge(
    frame: &mut Frame<'_>,
    area: Rect,
    view: &View<'_>,
    alert_rows: &[Alert],
    alert_index: usize,
    full: bool,
) {
    let t = view.theme;
    let panels = view.cockpit.panels();
    let has_map = panels.contains(&Panel::Map);
    let side: Vec<Panel> = panels
        .iter()
        .copied()
        .filter(|p| *p != Panel::Map)
        .collect();
    let alert_w = if full { 42 } else { 38 };
    let map_w = if has_map { 40 } else { 0 };
    let [left, _, mid, _, right] = TuiLayout::horizontal([
        Constraint::Length(alert_w),
        Constraint::Length(1),
        Constraint::Length(map_w),
        Constraint::Length(u16::from(has_map)),
        Constraint::Min(20),
    ])
    .areas(area);
    frame.render_widget(
        Tile {
            theme: t,
            title: "alerts".into(),
            right: Some(Line::styled(alert_rows.len().to_string(), t.muted())),
            focused: false,
        },
        left,
    );
    let alerts_area = pad(Tile::inner(left));
    frame.render_widget(
        Paragraph::new(callout_lines(
            alert_rows,
            alert_index,
            usize::from(alerts_area.width),
            t,
        )),
        alerts_area,
    );
    if has_map {
        render_panel(
            frame,
            mid,
            Panel::Map,
            view.cockpit.focused_panel() == Some(Panel::Map),
            view,
        );
    }
    let upcoming = upcoming_lines(
        view.state,
        Tile::inner(right).width.saturating_sub(2) as usize,
        t,
    );
    let up_h = (upcoming.len() as u16 + 3).min(right.height / 2);
    let [up, rest] =
        TuiLayout::vertical([Constraint::Length(up_h), Constraint::Min(0)]).areas(right);
    frame.render_widget(
        Tile {
            theme: t,
            title: "upcoming".into(),
            right: Some(Line::styled("next 60 min", t.label())),
            focused: false,
        },
        up,
    );
    frame.render_widget(Paragraph::new(upcoming), pad(Tile::inner(up)));
    let rest = Rect {
        y: rest.y + u16::from(rest.height > 0),
        height: rest.height.saturating_sub(1),
        ..rest
    };
    render_stack(frame, rest, &side, view);
}

fn pad(area: Rect) -> Rect {
    Rect {
        x: area.x + 1.min(area.width),
        width: area.width.saturating_sub(2),
        ..area
    }
}

/// Alert callouts for a panel `width` cells wide. A detail that does not fit
/// wraps onto more indented lines instead of being cut.
fn callout_lines(rows: &[Alert], selected: usize, width: usize, t: &Theme) -> Vec<Line<'static>> {
    if rows.is_empty() {
        return vec![Line::styled("all quiet", t.muted())];
    }
    let mut out = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        let title = if i == selected {
            Line::styled(
                row.text.clone(),
                t.intent(row.kind.intent()).add_modifier(Modifier::BOLD),
            )
        } else {
            Line::from(row.text.clone())
        };
        // The callout indents its detail by two cells.
        let mut detail = wrap_words(&row.detail, width.saturating_sub(2).max(8)).into_iter();
        out.extend(
            Callout {
                theme: t,
                intent: row.kind.intent(),
                title,
                detail: detail.next().map(Line::from),
            }
            .lines(),
        );
        out.extend(detail.map(|rest| Line::styled(format!("  {rest}"), t.muted())));
        out.push(Line::default());
    }
    out
}

/// Greedy word wrap to `width` display cells (as the table measures them);
/// a word longer than a line stays whole.
fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let cells = |s: &str| Span::raw(s.to_string()).width();
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let fits = cells(&line) + 1 + cells(word) <= width;
        if !line.is_empty() && !fits {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

fn upcoming_lines(state: &GameState, width: usize, t: &Theme) -> Vec<Line<'static>> {
    let now = state.last_tick_unix;
    let mut items: Vec<(u64, String, Intent)> = Vec::new();
    if let Some(secs) = state.raid_countdown_secs() {
        items.push((secs, format!("raid → {}", state.home), Intent::Danger));
    }
    let queues = std::iter::once((state.home, &state.build_queue))
        .chain(state.colonies.iter().map(|c| (c.coord, &c.build_queue)));
    for (coord, queue) in queues {
        if let Some(job) = queue.first()
            && let Some(at) = job.completes_at
        {
            items.push((
                at.saturating_sub(now),
                format!(
                    "[{}] {} lv{} {coord}",
                    job.kind.as_key(),
                    job.building,
                    if job.kind == hoshi_core::BuildJobKind::Dismantle {
                        job.from_level.saturating_sub(1)
                    } else {
                        job.from_level.saturating_add(1)
                    }
                ),
                Intent::Success,
            ));
        }
    }
    if let Some(job) = &state.research {
        items.push((
            job.completes_at.saturating_sub(now),
            format!("research {}", job.tech),
            Intent::Primary,
        ));
    }
    for mission in &state.missions {
        items.push((
            mission.phase_ends_at.saturating_sub(now),
            if mission.deploy {
                format!(
                    "fleet #{} {} → {} · cargo {}",
                    mission.id,
                    mission.phase.as_key(),
                    mission.dest,
                    mission.cargo.brief()
                )
            } else {
                format!(
                    "fleet #{} {} → {}",
                    mission.id,
                    mission.phase.as_key(),
                    mission.dest
                )
            },
            Intent::Primary,
        ));
    }
    items.sort_by_key(|row| row.0);
    let axis_w = width.saturating_sub(10).max(8);
    let mut cells: Vec<Span<'static>> = (0..axis_w).map(|_| Span::styled("─", t.line())).collect();
    for (secs, _, intent) in items.iter().filter(|row| row.0 <= 3600) {
        let x = (*secs as usize * (axis_w - 1)) / 3600;
        cells[x] = Span::styled("●", t.intent(*intent));
    }
    let mut axis = vec![Span::styled("now ", t.label())];
    axis.extend(cells);
    axis.push(Span::styled(" +60m", t.label()));
    let mut out = vec![Line::from(axis), Line::default()];
    if items.is_empty() {
        out.push(Line::styled("nothing scheduled", t.muted()));
    }
    for (secs, text, intent) in items {
        out.push(Line::from(vec![
            Span::styled(format!("T-{:<8}", countdown(secs)), t.intent(intent)),
            Span::styled(text, t.text()),
        ]));
    }
    out
}

fn render_stack(frame: &mut Frame<'_>, area: Rect, panels: &[Panel], view: &View<'_>) {
    if panels.is_empty() {
        return;
    }
    let mut constraints = Vec::new();
    for i in 0..panels.len() {
        if i > 0 {
            constraints.push(Constraint::Length(1));
        }
        constraints.push(Constraint::Min(3));
    }
    let rows = TuiLayout::vertical(constraints).split(area);
    for (i, panel) in panels.iter().enumerate() {
        if let Some(rect) = rows.get(i * 2) {
            render_panel(
                frame,
                *rect,
                *panel,
                view.cockpit.focused_panel() == Some(*panel),
                view,
            );
        }
    }
}

fn render_panel(frame: &mut Frame<'_>, area: Rect, panel: Panel, focused: bool, view: &View<'_>) {
    let t = view.theme;
    frame.render_widget(
        Tile {
            theme: t,
            title: if panel == Panel::Map {
                "star map".into()
            } else {
                panel.title().into()
            },
            right: (panel == Panel::Map)
                .then(|| Line::styled(format!("galaxy {}", view.state.home.galaxy), t.label())),
            focused,
        },
        area,
    );
    let inner = pad(Tile::inner(area));
    let height = inner.height as usize;
    let selected = if focused {
        view.cockpit.selected
    } else {
        usize::MAX
    };
    let lines = match panel {
        Panel::Map => map_lines(
            view.state,
            view.map_system,
            view.map_position,
            height,
            inner.width as usize,
            t,
        ),
        Panel::Gauges => gauge_lines(view.state, view.gauges, height, inner.width as usize, t),
        Panel::Queues => queue_lines(view.state, height, selected, t),
        Panel::Fleets => fleet_lines(view.state, height, selected, t),
        Panel::Timeline => timeline_lines(view.state, height, t),
        Panel::Market => market_lines(view.state, height, t),
        Panel::Kpis => kpi_lines(view.state, height, t),
        Panel::Raids => raid_lines(view.state, height, t),
        Panel::Tactics => tactics_lines(view.state, height, t),
    };
    frame.render_widget(Paragraph::new(lines).style(t.text()), inner);
}

fn alert_lines(rows: &[Alert], selected: usize, t: &Theme, mode: SizeMode) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(vec![
        Span::styled(" alerts", t.label()),
        Span::styled(format!("  {}", rows.len()), t.muted()),
    ])];
    if rows.is_empty() {
        lines.push(Line::from(Span::styled(" none", t.muted())));
        return lines;
    }
    let take = if mode == SizeMode::Tabbed { 1 } else { 2 };
    let mut parts = vec![Span::raw(" ")];
    for (i, row) in rows.iter().take(take + 2).enumerate() {
        if i > 0 {
            parts.push(Span::raw("   "));
        }
        let intent = row.kind.intent();
        let mut style = t.intent(intent);
        if i == selected {
            style = style.add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
        }
        parts.push(Span::styled("▌", t.intent(intent)));
        parts.push(Span::styled(row.text.clone(), style));
    }
    lines.push(Line::from(parts));
    lines
}

fn tab_line(panels: &[Panel], focus: usize, t: &Theme) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    for (i, panel) in panels.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("   "));
        }
        let label = panel.title().to_uppercase();
        let style = if i == focus {
            t.intent(Intent::Primary)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            t.muted()
        };
        spans.push(Span::styled(label, style));
    }
    Line::from(spans)
}

fn map_lines(
    state: &GameState,
    map_system: u16,
    map_position: u16,
    height: usize,
    width: usize,
    t: &Theme,
) -> Vec<Line<'static>> {
    let owned = state.owned_coords();
    let mut entries = Vec::new();
    for system in 1..=MAP_SYSTEM_MAX {
        let current = system == map_system;
        let mut spans = vec![Span::styled(
            format!("{} {system:>2}  ", if current { '▸' } else { ' ' }),
            if current {
                t.intent(Intent::Primary)
            } else {
                t.label()
            },
        )];
        let mut labels = Vec::new();
        for pos in 1..=MAP_SLOT_COUNT {
            let coord = Coord {
                galaxy: state.home.galaxy,
                system,
                position: pos,
            };
            let (ch, style) = if coord == state.home {
                ('H', t.intent(Intent::Primary).add_modifier(Modifier::BOLD))
            } else if owned.contains(&coord) {
                ('C', t.intent(Intent::Success))
            } else if state.outposts.iter().any(|o| o.coord == coord) {
                ('P', t.intent(Intent::Danger))
            } else if state.missions.iter().any(|m| m.dest == coord) {
                ('f', t.intent(Intent::Warning))
            } else if let Some(site) = state.site_at(coord) {
                (site.map_mark(), t.intent(Intent::Warning))
            } else if debris_at(state, coord) > 0.0 {
                ('*', t.intent(Intent::Warning))
            } else {
                ('·', t.faint())
            };
            let style = if current && pos == map_position {
                t.selected().patch(style).add_modifier(Modifier::REVERSED)
            } else {
                style
            };
            if pos > 1 {
                spans.push(Span::raw(" "));
            }
            spans.push(Span::styled(ch.to_string(), style));
            if coord == state.home {
                labels.push("home".to_string());
            } else if let Some(site) = state.site_at(coord) {
                let label = match site.outcome {
                    Some(outcome) => format!("{} {}", site.kind.as_key(), outcome.as_key()),
                    None => site.kind.as_key().to_string(),
                };
                labels.push(label);
            }
            let debris = debris_at(state, coord);
            if debris > 0.0 {
                labels.push(format!("debris {}", crate::ui::spans::compact(debris)));
            }
        }
        for mission in &state.missions {
            if mission.dest.galaxy == state.home.galaxy && mission.dest.system == system {
                let eta = mission.phase_ends_at.saturating_sub(state.last_tick_unix);
                labels.push(format!("fleet #{} {}", mission.id, countdown(eta)));
            }
        }
        if let Some(secs) = state.raid_countdown_secs()
            && system == state.home.system
        {
            labels.push(format!("raid {}", countdown(secs)));
        }
        let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        let extra = format!("  {}", labels.join("  "));
        if !labels.is_empty() && used + extra.chars().count() <= width {
            spans.push(Span::styled(extra, t.muted()));
        }
        entries.push(Line::from(spans));
    }
    window(&entries, map_system.saturating_sub(1) as usize, height)
}

fn gauge_lines(
    state: &GameState,
    gauges: &[Resources],
    height: usize,
    width: usize,
    t: &Theme,
) -> Vec<Line<'static>> {
    let spark_w = width.saturating_sub(28).min(24);
    let use_spark = spark_w >= 8;
    let mut lines = Vec::new();
    for kind in ResourceKind::ALL {
        let stock = state.resources.get(kind);
        let cap = state.storage_cap(kind);
        let rate = state.production_per_hour(kind);
        let hist: Vec<f64> = gauges.iter().map(|row| row.get(kind)).collect();
        let spark = if use_spark {
            sparkline(&hist, spark_w)
        } else {
            String::new()
        };
        let style = if cap > 0.0 && stock / cap >= 0.95 {
            t.intent(Intent::Warning)
        } else {
            t.intent(Intent::Success)
        };
        lines.push(Line::from(Span::styled(
            format!(
                "{} {:>8.0}/{:<8.0} {:+.0}/h {spark}",
                kind.abbr(),
                stock,
                cap,
                rate
            ),
            style,
        )));
    }
    lines.truncate(height.max(1));
    lines
}

fn queue_lines(state: &GameState, height: usize, selected: usize, t: &Theme) -> Vec<Line<'static>> {
    let now = state.last_tick_unix;
    let mut entries = Vec::new();
    push_planet_queue(
        &mut entries,
        state.home,
        &state.build_queue,
        now,
        selected,
        0,
        t,
    );
    for (idx, colony) in (1..).zip(state.colonies.iter()) {
        push_planet_queue(
            &mut entries,
            colony.coord,
            &colony.build_queue,
            now,
            selected,
            idx,
            t,
        );
    }
    if let Some(job) = &state.research {
        let eta = job.completes_at.saturating_sub(now);
        entries.push(Line::from(format!(
            "research {} {}",
            state.display_name(&job.tech),
            duration(eta)
        )));
    }
    window(
        &entries,
        selected.min(entries.len().saturating_sub(1)),
        height,
    )
}

fn push_planet_queue(
    entries: &mut Vec<Line<'static>>,
    coord: Coord,
    queue: &[hoshi_core::BuildJob],
    now: u64,
    selected: usize,
    index: usize,
    t: &Theme,
) {
    let mark = if index == selected { '>' } else { ' ' };
    let style = if index == selected {
        t.selected()
    } else {
        t.text()
    };
    if let Some(job) = queue.first() {
        let eta = job.completes_at.unwrap_or(now).saturating_sub(now);
        entries.push(Line::from(Span::styled(
            format!(
                "{mark} {coord} {} {eta}s  +{}",
                job.building,
                queue.len() - 1
            ),
            style,
        )));
    } else {
        entries.push(Line::from(Span::styled(
            format!("{mark} {coord} idle"),
            style,
        )));
    }
}

fn fleet_lines(state: &GameState, height: usize, selected: usize, t: &Theme) -> Vec<Line<'static>> {
    let mut entries = Vec::new();
    let parked = state.fleet.total();
    let mark = if selected == 0 { '>' } else { ' ' };
    entries.push(styled_row(
        format!("{mark} docked {parked} at {}", state.home),
        selected == 0,
        t,
        Intent::Success,
    ));
    for (i, mission) in state.missions.iter().enumerate() {
        let eta = mission.phase_ends_at.saturating_sub(state.last_tick_unix);
        let combat = mission.attack || mission.invade || state.pending_tactical.is_some();
        let phase = if combat && mission.phase == MissionPhase::Action {
            "combat"
        } else {
            mission.phase.as_key()
        };
        let mark = if selected == i + 1 { '>' } else { ' ' };
        entries.push(styled_row(
            format!(
                "{mark} #{} {} {} -> {} {eta}s",
                mission.id, phase, mission.origin, mission.dest
            ),
            selected == i + 1,
            t,
            if combat {
                Intent::Danger
            } else {
                Intent::Primary
            },
        ));
    }
    window(
        &entries,
        selected.min(entries.len().saturating_sub(1)),
        height,
    )
}

fn timeline_lines(state: &GameState, height: usize, t: &Theme) -> Vec<Line<'static>> {
    let now = state.last_tick_unix;
    let mut entries = Vec::new();
    if !state.history.events.is_empty() {
        for event in state.history.events.iter().rev().take(height.max(1)) {
            let ago = now.saturating_sub(event.at);
            let style = timeline_style(event.kind, t);
            entries.push(Line::from(Span::styled(
                format!("{} ago {}", duration(ago), timeline_text(state, event)),
                style,
            )));
        }
    } else {
        for event in state
            .visible_events(Actor::Player)
            .into_iter()
            .rev()
            .take(height.max(1))
        {
            let ago = now.saturating_sub(event.at);
            entries.push(Line::from(format!("{} ago {}", duration(ago), event.text)));
        }
    }
    if let Some(line) = bridge_line(state, t) {
        entries.insert(0, line);
    }
    entries
}

/// `leader:3`, `contract:7`: a save id, not something to read. Coordinates
/// (`1:2:3`) are all numbers and stay.
fn internal_id(text: &str) -> bool {
    text.contains(':') && !text.split(':').all(|part| part.parse::<u32>().is_ok())
}

/// One history row as a short sentence about what happened, without the
/// internal kind and actor tokens the History table shows as columns.
pub(crate) fn timeline_text(state: &GameState, event: &hoshi_core::HistoryEvent) -> String {
    // Ids read as player names (`metal_mine` → `iron mine`); prefixed and
    // coordinate refs pass through unchanged for the filters below.
    let obj = |i: usize| {
        event
            .objects
            .get(i)
            .map(|o| state.display_name(o))
            .unwrap_or_default()
    };
    // The building, spend centre or other subject; producers put it last.
    let last = event
        .objects
        .last()
        .map(|o| state.display_name(o))
        .unwrap_or_default();
    let outcome = event.outcome.replace('_', " ");
    match (event.kind, event.action.as_str()) {
        (HistoryKind::Build, "queue") => format!("queued {} at {}", obj(1), obj(0)),
        (HistoryKind::Build, "complete") => format!("{} reached {outcome}", obj(1)),
        (HistoryKind::Build, "dismantle") => format!("queued dismantling {last}"),
        (HistoryKind::Build, "dismantle_start") => format!("dismantling {last}"),
        (HistoryKind::Build, "dismantle_complete") => format!("{last} dismantled"),
        (HistoryKind::Research, "start") => format!("started {} research", obj(0)),
        (HistoryKind::Research, "complete") => format!("{} research reached {outcome}", obj(0)),
        (HistoryKind::Ship, "queue") => format!("queued {}", obj(0)),
        (HistoryKind::Ship, "complete") => format!("{} built", obj(0)),
        (HistoryKind::Combat, "fight") => match event.objects.first().map(String::as_str) {
            Some("attacker") => format!("fight: attackers won ({outcome})"),
            Some("defender") => format!("fight: defenders held ({outcome})"),
            _ => format!("fight: draw ({outcome})"),
        },
        (_, "story_choice") => format!("story: {} → {outcome}", obj(0)),
        (_, "request_spend") => format!("{last} spending {outcome}"),
        (_, "open_incident") => format!("incident opened: {} ({outcome})", obj(1)),
        _ => {
            // Internal ids (`leader:3`, `spend:12`) stay off the bridge;
            // coordinates (`1:2:3`) and plain names stay.
            let objects: Vec<String> = event
                .objects
                .iter()
                .filter(|o| !internal_id(o))
                .map(|o| o.replace('_', " "))
                .collect();
            let mut text = event.action.replace('_', " ");
            if !objects.is_empty() {
                text = format!("{text}: {}", objects.join(", "));
            }
            if !outcome.is_empty() && !internal_id(&event.outcome) {
                text = format!("{text} ({outcome})");
            }
            text
        }
    }
}

fn bridge_line(state: &GameState, t: &Theme) -> Option<Line<'static>> {
    let now = state.last_tick_unix;
    if state.activity.prod_boost_until.is_some_and(|t| t > now)
        || state.activity.research_boost_until.is_some_and(|t| t > now)
    {
        Some(Line::from(Span::styled(
            "bridge boost active",
            t.intent(Intent::Primary),
        )))
    } else {
        None
    }
}

fn timeline_style(kind: HistoryKind, t: &Theme) -> Style {
    match kind {
        HistoryKind::Combat => t.intent(Intent::Danger),
        HistoryKind::Trade => t.intent(Intent::Warning),
        HistoryKind::Activity => t.intent(Intent::Primary),
        HistoryKind::Build | HistoryKind::Research | HistoryKind::Ship => t.intent(Intent::Success),
        HistoryKind::Fleet | HistoryKind::Prestige | HistoryKind::Other => t.text(),
    }
}

fn market_lines(state: &GameState, height: usize, _t: &Theme) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let market = &state.market;
    lines.push(Line::from(format!(
        "Fe {:.1}  Si {:.1}  D {:.1}",
        market.price(ResourceKind::Iron),
        market.price(ResourceKind::Silicon),
        market.price(ResourceKind::Deuterium)
    )));
    lines.push(Line::from(format!(
        "Fe {}",
        sparkline(market.history(ResourceKind::Iron), 16)
    )));
    lines.truncate(height.max(1));
    lines
}

fn kpi_lines(state: &GameState, height: usize, _t: &Theme) -> Vec<Line<'static>> {
    let snap = state.kpi_snapshot();
    let mut lines = Vec::new();
    for row in snap.measurements.iter().take(height.max(1)) {
        let target = row
            .target
            .map(|t| format!("{t:.0}"))
            .unwrap_or_else(|| "-".into());
        let var = row
            .variance()
            .map(|v| format!("{v:+.0}"))
            .unwrap_or_else(|| "-".into());
        lines.push(Line::from(format!(
            "{} {} {:.0}/{target} {var}",
            row.kpi.as_key(),
            row.owner.as_key(),
            row.value
        )));
    }
    if lines.is_empty() {
        lines.push(Line::from("no kpis"));
    }
    lines.truncate(height.max(1));
    lines
}

fn raid_lines(state: &GameState, height: usize, t: &Theme) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    match state.raid_countdown_secs() {
        Some(secs) => lines.push(styled_row(
            format!("next raid {secs}s"),
            true,
            t,
            Intent::Danger,
        )),
        None => lines.push(Line::from("no raid timer")),
    }
    if state.pending_tactical.is_some() {
        lines.push(styled_row(
            "tactical overlay pending".into(),
            false,
            t,
            Intent::Warning,
        ));
    }
    for mission in state.missions.iter().filter(|m| m.attack || m.invade) {
        let eta = mission.phase_ends_at.saturating_sub(state.last_tick_unix);
        lines.push(Line::from(format!(
            "strike {} {} {eta}s",
            mission.dest,
            mission.phase.as_key()
        )));
    }
    lines.truncate(height.max(1));
    lines
}

fn tactics_lines(state: &GameState, height: usize, _t: &Theme) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(format!(
        "formation {}  stance {}",
        state.fleet_tactics.formation.as_key(),
        state.fleet_tactics.stance.as_key()
    ))];
    if let Some(pending) = &state.pending_tactical {
        lines.push(Line::from(format!(
            "pending {} / {}",
            pending.atk_plan.formation.as_key(),
            pending.atk_plan.stance.as_key()
        )));
    }
    lines.truncate(height.max(1));
    lines
}

fn detail_lines(
    state: &GameState,
    target: &Target,
    height: usize,
    t: &Theme,
) -> Vec<Line<'static>> {
    match target {
        Target::Planet(coord) => planet_detail(state, *coord, height, t),
        Target::Fleet { mission } => fleet_detail(state, *mission, height, t),
        Target::Faction(id) => faction_detail(state, id, height, t),
    }
}

fn planet_detail(state: &GameState, coord: Coord, height: usize, t: &Theme) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(Span::styled(
        format!("planet {coord}"),
        t.strong(),
    ))];
    let owned = state.owned_coords().contains(&coord);
    if coord == state.home {
        lines.push(Line::from(format!(
            "home  queue {}  parked {}",
            state.build_queue.len(),
            state.fleet.total()
        )));
        if !state.defences.is_empty() {
            lines.push(Line::from("defences manned"));
        }
    } else if let Some(colony) = state.colonies.iter().find(|c| c.coord == coord) {
        lines.push(Line::from(format!(
            "colony {}  queue {}  parked {}",
            colony.kind.as_key(),
            colony.build_queue.len(),
            colony.fleet.total()
        )));
    } else if let Some(outpost) = state.outposts.iter().find(|o| o.coord == coord) {
        lines.push(Line::from(format!(
            "pirate {}  faction pirates",
            outpost.kind.as_key()
        )));
        if let Some(report) = state.latest_report(coord) {
            lines.push(Line::from(format!(
                "spy {}  conf {:.2}  age {}",
                report.tier,
                state.report_confidence(report),
                report.age_secs(state.last_tick_unix)
            )));
        }
    } else if owned {
        lines.push(Line::from("owned"));
    } else {
        lines.push(Line::from("unscouted"));
    }
    for mission in state
        .missions
        .iter()
        .filter(|m| m.dest == coord || m.origin == coord)
    {
        let eta = mission.phase_ends_at.saturating_sub(state.last_tick_unix);
        lines.push(Line::from(format!(
            "fleet #{} {} {eta}s",
            mission.id,
            mission.phase.as_key()
        )));
    }
    lines.truncate(height.max(1));
    lines
}

fn fleet_detail(
    state: &GameState,
    mission_id: Option<u64>,
    height: usize,
    _t: &Theme,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let admiral_id = state
        .assignments
        .iter()
        .find(|row| {
            row.role == RoleKind::Admiral && row.target == hoshi_core::AssignmentTarget::HomeFleet
        })
        .map(|row| row.leader_id);
    let admiral = admiral_id.and_then(|id| state.leaders.iter().find(|leader| leader.id == id));
    if let Some(id) = mission_id
        && let Some(mission) = state.missions.iter().find(|m| m.id == id)
    {
        lines.push(Line::from(format!("fleet #{id}")));
        lines.push(Line::from(format!(
            "{} -> {}  {}",
            mission.origin,
            mission.dest,
            mission.phase.as_key()
        )));
        lines.push(Line::from(format!(
            "ships {}  cargo {}",
            mission.ships.total(),
            cargo_label(mission.cargo)
        )));
        lines.push(Line::from(format!(
            "admiral {}",
            admiral.map(|a| a.name.as_str()).unwrap_or("-")
        )));
    } else {
        lines.push(Line::from(format!("parked at {}", state.home)));
        lines.push(Line::from(format!("ships {}", state.fleet.total())));
        lines.push(Line::from(format!(
            "fighters {}",
            state.fleet.get(ShipKind::LightFighter)
        )));
        lines.push(Line::from(format!(
            "admiral {}",
            admiral.map(|a| a.name.as_str()).unwrap_or("-")
        )));
    }
    lines.truncate(height.max(1));
    lines
}

fn faction_detail(state: &GameState, id: &str, height: usize, _t: &Theme) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if let Some(spec) = state.config.diplomacy.iter().find(|row| row.id == id) {
        let standing = state.diplomacy.standing(id);
        let tier = spec.tier(standing).map(|t| t.name.as_str()).unwrap_or("-");
        lines.push(Line::from(format!("{}  {standing:.0}  {tier}", spec.name)));
        if id == "pirates" {
            for outpost in state.outposts.iter().take(6) {
                lines.push(Line::from(format!("world {}", outpost.coord)));
            }
        }
        for change in state
            .diplomacy
            .log
            .iter()
            .rev()
            .filter(|c| c.faction == id)
            .take(4)
        {
            lines.push(Line::from(format!(
                "{:+.0} {} t{}",
                change.delta, change.reason, change.at
            )));
        }
    }
    for row in state
        .rival_intel()
        .into_iter()
        .filter(|row| row.personality == id || row.name.contains(id))
    {
        let stock = match row.resources {
            Some(r) => format!("  {:.0}M", r.iron),
            None => String::new(),
        };
        lines.push(Line::from(format!(
            "rival {} {} last {}  {}  conf {:.2}  age {}{stock}",
            row.name, row.home, row.last_action, row.source, row.confidence, row.age_secs
        )));
    }
    if lines.is_empty() {
        lines.push(Line::from(format!("faction {id}")));
    }
    lines.truncate(height.max(1));
    lines
}

fn styled_row(text: String, selected: bool, t: &Theme, intent: Intent) -> Line<'static> {
    let style = if selected {
        t.selected().patch(t.intent(intent))
    } else {
        t.intent(intent)
    };
    Line::from(Span::styled(text, style))
}

pub(crate) fn sparkline(values: &[f64], width: usize) -> String {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    if values.is_empty() || width == 0 {
        return String::new();
    }
    let slice = if values.len() > width {
        &values[values.len() - width..]
    } else {
        values
    };
    let min = slice.iter().copied().fold(f64::INFINITY, f64::min);
    let max = slice.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let span = (max - min).max(1e-9);
    slice
        .iter()
        .map(|&v| {
            let t = ((v - min) / span * (BARS.len() - 1) as f64).round() as usize;
            BARS[t.min(BARS.len() - 1)]
        })
        .collect()
}

fn window(entries: &[Line<'static>], selected: usize, height: usize) -> Vec<Line<'static>> {
    if entries.is_empty() || height == 0 {
        return Vec::new();
    }
    let len = entries.len();
    let height = height.min(len);
    let mut start = selected.saturating_sub(height / 2);
    if start + height > len {
        start = len - height;
    }
    entries[start..start + height].to_vec()
}

pub fn record_gauge(history: &mut Vec<Resources>, sample: Resources) {
    history.push(sample);
    if history.len() > GAUGE_CAP {
        let drop = history.len() - GAUGE_CAP;
        history.drain(..drop);
    }
}

pub fn select_len(state: &GameState, cockpit: &Cockpit) -> usize {
    if cockpit.stack.last().is_some() {
        return fleet_targets(state).len().max(1);
    }
    match cockpit.focused_panel() {
        Some(Panel::Fleets) => fleet_targets(state).len(),
        Some(Panel::Queues) => planet_targets(state).len().max(1),
        Some(Panel::Map) => MAP_SYSTEM_MAX as usize,
        _ => alerts(state, cockpit.victory_focus).len().max(1),
    }
}

fn cargo_label(stock: Resources) -> String {
    ResourceKind::ALL
        .iter()
        .map(|&kind| format!("{} {:.0}", kind.abbr(), stock.get(kind)))
        .collect::<Vec<_>>()
        .join("  ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoshi_core::GameState;

    #[test]
    fn alert_details_wrap_at_word_boundaries_instead_of_cutting() {
        let lines = wrap_words("production slows at 1:1:1 · build a solar plant", 26);
        assert_eq!(
            lines,
            vec!["production slows at 1:1:1", "· build a solar plant"]
        );
        assert_eq!(wrap_words("short", 26), vec!["short"]);
    }

    #[test]
    fn presets_parse_and_skip_unknown_tokens() {
        let presets = Presets::parse("map,nope,gauges", "market", "raids,tactics,raids");
        assert_eq!(presets.overview, vec![Panel::Map, Panel::Gauges]);
        assert_eq!(presets.economy, vec![Panel::Market]);
        assert_eq!(presets.war, vec![Panel::Raids, Panel::Tactics]);
    }

    #[test]
    fn layout_cycles_through_the_three_presets() {
        assert_eq!(CockpitLayout::Overview.next(), CockpitLayout::Economy);
        assert_eq!(CockpitLayout::Economy.next(), CockpitLayout::War);
        assert_eq!(CockpitLayout::War.next(), CockpitLayout::Overview);
    }

    #[test]
    fn alerts_order_threats_before_idle_queues() {
        let mut state = GameState::new(1_000);
        state.next_raid_at = Some(1_120);
        state.resources.iron = state.storage_cap(ResourceKind::Iron);
        let rows = alerts(&state, None);
        let kinds: Vec<_> = rows.iter().map(|row| row.kind.severity()).collect();
        let mut sorted = kinds.clone();
        sorted.sort_unstable();
        assert_eq!(kinds, sorted);
        assert!(rows.iter().any(|row| row.text.contains("Incoming raid")));
        assert!(rows.iter().any(|row| row.text.contains("idle")));
    }

    #[test]
    fn the_expiring_decision_leads_and_keeps_the_idle_and_storage_rows() {
        let mut state = GameState::new(1_000);
        state.story_pending = Some(hoshi_core::PendingStory {
            event_id: "derelict".into(),
            offered_at: 1_000,
            timeout_at: 1_200,
        });
        state.fleet.add(hoshi_core::ShipKind::LightFighter, 2);
        state.resources.iron = state.storage_cap(ResourceKind::Iron);
        let decision = hoshi_core::expiring_decision(&state).expect("story");
        let rows = alerts(&state, None);
        assert_eq!(rows[0].decision, Some(decision));
        assert!(rows[0].text.contains(decision.action.as_key()));
        assert!(rows[0].detail.contains(&decision.deadline.to_string()));
        assert!(rows[0].detail.contains(decision.id.as_key()));
        assert!(rows.iter().skip(1).any(|row| row.text.contains("idle")));
        assert!(rows.iter().skip(1).any(|row| row.text.contains("storage")));
        assert!(rows.iter().skip(1).all(|row| row.decision.is_none()));
    }

    #[test]
    fn a_focus_drops_idle_colonies_but_keeps_power_and_the_decision() {
        let mut state = GameState::new(1_000);
        state.story_pending = Some(hoshi_core::PendingStory {
            event_id: "derelict".into(),
            offered_at: 1_000,
            timeout_at: 1_200,
        });
        state.fleet.add(hoshi_core::ShipKind::LightFighter, 2);
        state.buildings.metal_mine = 30;
        state.build_queue.clear();
        state.research = None;
        state.colonies.push(hoshi_core::Colony {
            coord: hoshi_core::Coord {
                galaxy: 1,
                system: 1,
                position: 9,
            },
            kind: hoshi_core::PlanetType::Rocky,
            size: 3,
            resources: Resources::ZERO,
            buildings: hoshi_core::Buildings::default(),
            fleet: hoshi_core::Fleet::default(),
            defences: hoshi_core::DefenceGrid::default(),
            build_queue: Vec::new(),
            rare: None,
            goods: Default::default(),
        });
        let plain = alerts(&state, None);
        assert!(plain.iter().any(|row| row.text == "Queue idle"));
        let rows = alerts(&state, Some(hoshi_core::VictoryPath::Conquest));
        assert!(rows[0].decision.is_some());
        assert!(!rows.iter().any(|row| row.text == "Queue idle"));
        assert!(
            rows.iter().any(|row| row.text == "Queues idle"),
            "home stays"
        );
        assert!(rows.iter().any(|row| row.text.starts_with("Energy short")));
    }

    #[test]
    fn a_lost_raid_alert_names_lost_ships_then_returned_defences() {
        let mut state = GameState::new(1_000);
        state.config.defences.rebuild_chance = 0.0;
        state.config.raid.min_ships = 60;
        state.config.raid.strength_factor = 20.0;
        state.fleet.add(hoshi_core::ShipKind::LightFighter, 3);
        state.defences.add(hoshi_core::DefenceKind::LightCannon, 6);
        state.trigger_raid(1_000);
        let rows = alerts(&state, None);
        let lost = rows
            .iter()
            .position(|r| r.text.starts_with("Raid lost"))
            .unwrap();
        assert_eq!(rows[lost + 1].text, "Ships lost");
        assert!(
            rows[lost + 1].detail.contains("gone for good"),
            "{:?}",
            rows[lost + 1]
        );
        assert_eq!(rows[lost + 2].text, "Defences returned");
        assert!(
            rows[lost + 2]
                .detail
                .starts_with("0 · batteries still missing"),
            "{:?}",
            rows[lost + 2]
        );
    }

    #[test]
    fn raids_saved_before_the_missing_count_show_only_the_returned_units() {
        let losses = hoshi_core::RaidLosses {
            ships_lost: hoshi_core::Fleet::default(),
            defences_returned: 2,
            batteries_missing: None,
        };
        assert_eq!(
            raid_loss_lines(losses),
            [
                "Ships lost: none".to_string(),
                "Defences returned: 2".to_string()
            ]
        );
    }

    #[test]
    fn a_story_note_sits_behind_the_decision_until_the_next_arc_is_pending() {
        let mut state = GameState::new(0);
        state.config.raid.enabled = false;
        state.config.market.enabled = false;
        state.config.diplomacy.enabled = false;
        state.next_raid_at = None;
        state.catch_up(1_800);
        state.choose_story("answer", 1_800).unwrap();
        let rows = alerts(&state, None);
        assert!(rows[0].decision.is_some() || rows[0].text.contains("hail"));
        assert!(
            rows.iter().any(|row| row.text == "The hail is still open."),
            "{:?}",
            rows.iter().map(|row| row.text.as_str()).collect::<Vec<_>>()
        );
        state.catch_up(1_800 + 3_600);
        let rows = alerts(&state, None);
        assert!(rows.iter().all(|row| row.text != "The hail is still open."));
    }

    #[test]
    fn raid_warning_names_the_scheduled_fleet_and_the_stance() {
        let mut state = GameState::new(1_000);
        state.next_raid_at = Some(1_120);
        state.set_formation("wedge").unwrap();
        state.set_stance("aggressive").unwrap();
        let ships = state.scheduled_raid_ships();
        assert!(ships > 0);
        let rows = alerts(&state, None);
        let raid = rows
            .iter()
            .find(|row| row.text.contains("Incoming raid"))
            .expect("warning");
        assert!(
            raid.detail.contains(&format!("{ships} attackers")),
            "{}",
            raid.detail
        );
        assert!(raid.detail.contains("stance aggressive"), "{}", raid.detail);
        assert!(raid.detail.contains("formation wedge"), "{}", raid.detail);
    }

    #[test]
    fn resolved_raid_drops_the_raid_alert() {
        let mut state = GameState::new(1_000);
        state.next_raid_at = None;
        state.config.raid.enabled = false;
        let rows = alerts(&state, None);
        assert!(!rows.iter().any(|row| row.text.contains("Incoming raid")));
    }

    #[test]
    fn the_upcoming_row_names_a_deploys_cargo() {
        let mut state = GameState::new(1_000);
        state.resources = Resources::from_parts(5_000.0, 5_000.0, 10_000.0);
        state.fleet.add(hoshi_core::ShipKind::LargeCargo, 1);
        let mut ships = hoshi_core::Fleet::default();
        ships.add(hoshi_core::ShipKind::LargeCargo, 1);
        let dest = Coord {
            galaxy: 1,
            system: 1,
            position: 9,
        };
        let cargo = Resources::from_parts(1_000.0, 0.0, 0.0);
        state
            .launch_deploy(state.home, dest, ships, cargo, 600, 100, 1_000)
            .unwrap();
        let text: String = upcoming_lines(&state, 120, &Theme::default())
            .iter()
            .flat_map(|line| line.spans.iter().map(|span| span.content.to_string()))
            .collect();
        assert!(text.contains(&format!("cargo {}", cargo.brief())), "{text}");
    }

    #[test]
    fn map_marks_debris_with_its_amount() {
        let mut state = GameState::new(1_000);
        let coord = (2..16)
            .map(|position| Coord {
                galaxy: 1,
                system: state.home.system,
                position,
            })
            .find(|c| state.site_at(*c).is_none() && state.outpost(*c).is_none())
            .unwrap();
        state.debris.insert(coord, 1_500.0);
        let lines = map_lines(&state, state.home.system, 1, 20, 160, &Theme::default());
        let text: String = lines
            .iter()
            .flat_map(|line| line.spans.iter().map(|span| span.content.to_string()))
            .collect();
        assert!(text.contains('*'), "{text}");
        assert!(text.contains("debris 1.5k"), "{text}");
    }

    #[test]
    fn map_marks_open_sites_by_kind_and_resolved_sites_by_outcome() {
        let mut state = GameState::new(1_000);
        assert!(state.sites.len() >= 2);
        state.sites[1].outcome = Some(hoshi_core::ExpeditionKind::Resources);
        let open = state.sites[0].map_mark();
        let resolved = state.sites[1].map_mark();
        let lines = map_lines(&state, state.home.system, 1, 20, 160, &Theme::default());
        let text: String = lines
            .iter()
            .flat_map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref().to_string())
            })
            .collect();
        assert!(text.contains(open), "{text}");
        assert!(text.contains(resolved), "{text}");
        assert!(text.contains(state.sites[0].kind.as_key()), "{text}");
        assert!(text.contains("resources"), "{text}");
    }

    #[test]
    fn map_confirm_opens_the_cursor_slot() {
        let mut cockpit = Cockpit::default();
        let cursor = Coord {
            galaxy: 1,
            system: 4,
            position: 7,
        };
        cockpit.selected = 0;
        cockpit.confirm(&[], &[], &[Coord::HOME], cursor);
        assert_eq!(cockpit.top(), Some(&Target::Planet(cursor)));
    }

    #[test]
    fn queue_confirm_opens_the_selected_planet() {
        let mut cockpit = Cockpit::default();
        for _ in 0..cockpit.panels().len() {
            if cockpit.focused_panel() == Some(Panel::Queues) {
                break;
            }
            cockpit.nudge_focus(1);
        }
        assert_eq!(cockpit.focused_panel(), Some(Panel::Queues));
        let colony = Coord {
            galaxy: 1,
            system: 2,
            position: 3,
        };
        cockpit.selected = 1;
        cockpit.confirm(&[], &[], &[Coord::HOME, colony], Coord::HOME);
        assert_eq!(cockpit.top(), Some(&Target::Planet(colony)));
    }

    #[test]
    fn size_mode_matches_the_three_accepted_terminals() {
        assert_eq!(size_mode(160, 48), SizeMode::Full);
        assert_eq!(size_mode(120, 40), SizeMode::Compact);
        assert_eq!(size_mode(80, 24), SizeMode::Tabbed);
    }
}
