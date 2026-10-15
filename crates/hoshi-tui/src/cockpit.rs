//! Dense command-center screen. Core still owns the rules; this only renders.

use hoshi_core::{
    Coord, GameState, HistoryKind, MissionPhase, ResourceKind, Resources, RoleKind, ShipKind,
};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout as TuiLayout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

pub const OVERVIEW_PANELS: &str = "map,gauges,queues,fleets,timeline";
pub const ECONOMY_PANELS: &str = "gauges,queues,market";
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
    Raid,
    Defence,
    Hostile,
    IdleQueue,
    Storage,
}

impl AlertKind {
    fn severity(self) -> u8 {
        match self {
            Self::Raid => 0,
            Self::Defence => 1,
            Self::Hostile => 2,
            Self::IdleQueue => 3,
            Self::Storage => 4,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alert {
    kind: AlertKind,
    pub text: String,
    pub jump: Target,
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
    pub high_contrast: bool,
    focus: usize,
    pub(crate) selected: usize,
    stack: Vec<Target>,
    layout_dirty: bool,
}

impl Default for Cockpit {
    fn default() -> Self {
        Self {
            layout: CockpitLayout::Overview,
            presets: Presets::default(),
            high_contrast: false,
            focus: 0,
            selected: 0,
            stack: Vec::new(),
            layout_dirty: false,
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

pub fn alerts(state: &GameState) -> Vec<Alert> {
    let mut rows = Vec::new();
    let home = state.home;
    if let Some(secs) = state.raid_countdown_secs() {
        rows.push(Alert {
            kind: AlertKind::Raid,
            text: format!("incoming raid {secs}s"),
            jump: Target::Planet(home),
        });
        if state.defences.is_empty() {
            rows.push(Alert {
                kind: AlertKind::Defence,
                text: format!("undefended home {home}"),
                jump: Target::Planet(home),
            });
        }
    }
    if state.pending_tactical.is_some() {
        rows.push(Alert {
            kind: AlertKind::Raid,
            text: "tactical combat pending".into(),
            jump: Target::Planet(home),
        });
    }
    for spec in state.config.diplomacy.iter() {
        let standing = state.diplomacy.standing(&spec.id);
        if standing < -20.0 {
            rows.push(Alert {
                kind: AlertKind::Hostile,
                text: format!("hostile {} {standing:.0}", spec.name),
                jump: Target::Faction(spec.id.clone()),
            });
        }
    }
    if state.build_queue.is_empty() && state.research.is_none() {
        rows.push(Alert {
            kind: AlertKind::IdleQueue,
            text: format!("idle queues {home}"),
            jump: Target::Planet(home),
        });
    }
    for colony in &state.colonies {
        if colony.build_queue.is_empty() {
            rows.push(Alert {
                kind: AlertKind::IdleQueue,
                text: format!("idle queue {}", colony.coord),
                jump: Target::Planet(colony.coord),
            });
        }
    }
    for kind in [
        ResourceKind::Metal,
        ResourceKind::Crystal,
        ResourceKind::Deuterium,
    ] {
        let cap = state.storage_cap(kind);
        let stock = state.resources.get(kind);
        if cap > 0.0 && stock / cap >= 0.95 {
            rows.push(Alert {
                kind: AlertKind::Storage,
                text: format!("{} storage {:.0}%", kind.as_key(), 100.0 * stock / cap),
                jump: Target::Planet(home),
            });
        }
    }
    rows.sort_by_key(|row| row.kind.severity());
    rows
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
    let mode = size_mode(area.width, area.height);
    let high = view.cockpit.high_contrast;
    let alert_rows = alerts(view.state);
    let panels = view.cockpit.panels();
    let chunks = TuiLayout::vertical([
        Constraint::Length(if mode == SizeMode::Tabbed { 2 } else { 3 }),
        Constraint::Min(3),
        Constraint::Length(footer.len() as u16),
    ])
    .split(area);
    frame.render_widget(
        Paragraph::new(alert_lines(&alert_rows, alert_index, high, mode)),
        chunks[0],
    );
    frame.render_widget(Paragraph::new(footer), chunks[2]);
    if let Some(target) = view.cockpit.stack.last() {
        frame.render_widget(
            panel_block("detail", true, high).title(" detail "),
            chunks[1],
        );
        let inner = inset(chunks[1]);
        frame.render_widget(
            Paragraph::new(detail_lines(view.state, target, inner.height as usize, high)),
            inner,
        );
        return;
    }
    match mode {
        SizeMode::Tabbed => {
            let panel = view.cockpit.focused_panel().unwrap_or(Panel::Gauges);
            let inner_chunks =
                TuiLayout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(chunks[1]);
            frame.render_widget(
                Paragraph::new(tab_line(panels, view.cockpit.focus, high)),
                inner_chunks[0],
            );
            render_panel(frame, inner_chunks[1], panel, true, &view);
        }
        SizeMode::Compact | SizeMode::Full => {
            render_spread(frame, chunks[1], &view, mode == SizeMode::Full);
        }
    }
}

fn render_spread(frame: &mut Frame<'_>, area: Rect, view: &View<'_>, full: bool) {
    let panels = view.cockpit.panels();
    if panels.is_empty() {
        return;
    }
    let has_map = panels.contains(&Panel::Map);
    let side: Vec<Panel> = panels
        .iter()
        .copied()
        .filter(|p| *p != Panel::Map)
        .collect();
    if has_map && !side.is_empty() {
        let cols = TuiLayout::horizontal([
            Constraint::Percentage(if full { 58 } else { 50 }),
            Constraint::Percentage(if full { 42 } else { 50 }),
        ])
        .split(area);
        render_panel(
            frame,
            cols[0],
            Panel::Map,
            view.cockpit.focused_panel() == Some(Panel::Map),
            view,
        );
        render_stack(frame, cols[1], &side, view);
    } else if has_map {
        render_panel(frame, area, Panel::Map, true, view);
    } else {
        render_stack(frame, area, panels, view);
    }
}

fn render_stack(frame: &mut Frame<'_>, area: Rect, panels: &[Panel], view: &View<'_>) {
    if panels.is_empty() {
        return;
    }
    let constraints: Vec<Constraint> = panels.iter().map(|_| Constraint::Min(3)).collect();
    let rows = TuiLayout::vertical(constraints).split(area);
    for (i, panel) in panels.iter().enumerate() {
        if let Some(rect) = rows.get(i) {
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
    let high = view.cockpit.high_contrast;
    let title = if focused {
        format!("[{}]", panel.title())
    } else {
        format!(" {} ", panel.title())
    };
    frame.render_widget(panel_block(&title, focused, high), area);
    let inner = inset(area);
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
            high,
        ),
        Panel::Gauges => gauge_lines(
            view.state,
            view.gauges,
            height,
            inner.width as usize,
            high,
        ),
        Panel::Queues => queue_lines(view.state, height, selected, high),
        Panel::Fleets => fleet_lines(view.state, height, selected, high),
        Panel::Timeline => timeline_lines(view.state, height, high),
        Panel::Market => market_lines(view.state, height, high),
        Panel::Raids => raid_lines(view.state, height, high),
        Panel::Tactics => tactics_lines(view.state, height, high),
    };
    frame.render_widget(Paragraph::new(lines), inner);
}

fn panel_block(title: &str, focused: bool, high: bool) -> Block<'_> {
    let mut style = Style::default().fg(if high { Color::White } else { Color::Gray });
    if focused {
        style = style
            .fg(if high { Color::White } else { Color::Cyan })
            .add_modifier(Modifier::BOLD);
        if high {
            style = style.add_modifier(Modifier::REVERSED);
        }
    }
    Block::default()
        .borders(Borders::ALL)
        .border_style(style)
        .title(title.to_string())
}

fn inset(area: Rect) -> Rect {
    Rect {
        x: area.x.saturating_add(1),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    }
}

fn alert_lines(rows: &[Alert], selected: usize, high: bool, mode: SizeMode) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(Span::styled(
        "alerts",
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    if rows.is_empty() {
        lines.push(Line::from("none"));
        return lines;
    }
    let take = if mode == SizeMode::Tabbed { 1 } else { 2 };
    let mut parts = Vec::new();
    for (i, row) in rows.iter().take(take + 2).enumerate() {
        let mark = if i == selected { '>' } else { ' ' };
        let style = alert_style(row.kind, high);
        parts.push(Span::styled(format!("{mark}{}", row.text), style));
    }
    lines.push(Line::from(parts.into_iter().fold(
        Vec::new(),
        |mut acc, span| {
            if !acc.is_empty() {
                acc.push(Span::raw("  "));
            }
            acc.push(span);
            acc
        },
    )));
    lines
}

fn alert_style(kind: AlertKind, high: bool) -> Style {
    let color = if high {
        Color::White
    } else {
        match kind {
            AlertKind::Raid | AlertKind::Defence => Color::Red,
            AlertKind::Hostile => Color::Yellow,
            AlertKind::IdleQueue => Color::Cyan,
            AlertKind::Storage => Color::Yellow,
        }
    };
    let mut style = Style::default().fg(color);
    if matches!(kind, AlertKind::Raid | AlertKind::Defence) {
        style = style.add_modifier(Modifier::BOLD);
    }
    style
}

fn tab_line(panels: &[Panel], focus: usize, high: bool) -> Line<'static> {
    let mut spans = Vec::new();
    for (i, panel) in panels.iter().enumerate() {
        let label = panel.title().to_uppercase();
        let mut style = Style::default().fg(if high { Color::White } else { Color::Gray });
        if i == focus {
            style = style
                .fg(if high { Color::White } else { Color::Cyan })
                .add_modifier(Modifier::BOLD | Modifier::REVERSED);
        }
        spans.push(Span::styled(format!(" {label} "), style));
    }
    Line::from(spans)
}

fn map_lines(
    state: &GameState,
    map_system: u16,
    map_position: u16,
    height: usize,
    high: bool,
) -> Vec<Line<'static>> {
    let owned = state.owned_coords();
    let mut entries = Vec::new();
    for system in 1..=MAP_SYSTEM_MAX {
        let mark = if system == map_system { '>' } else { ' ' };
        let mut glyphs = String::new();
        let mut labels = Vec::new();
        for pos in 1..=MAP_SLOT_COUNT {
            let coord = Coord {
                galaxy: state.home.galaxy,
                system,
                position: pos,
            };
            let ch = if coord == state.home {
                'H'
            } else if owned.contains(&coord) {
                'C'
            } else if state.outposts.iter().any(|o| o.coord == coord) {
                'P'
            } else if state.missions.iter().any(|m| m.dest == coord) {
                'f'
            } else {
                '.'
            };
            if pos == map_position && system == map_system {
                glyphs.push('[');
                glyphs.push(ch);
                glyphs.push(']');
            } else {
                glyphs.push(ch);
            }
            if coord == state.home {
                labels.push("home".into());
            }
        }
        for mission in &state.missions {
            if mission.dest.galaxy == state.home.galaxy && mission.dest.system == system {
                let eta = mission.phase_ends_at.saturating_sub(state.last_tick_unix);
                labels.push(format!("fleet #{} {eta}s", mission.id));
            }
        }
        if let Some(secs) = state.raid_countdown_secs()
            && system == state.home.system
        {
            labels.push(format!("raid {secs}s"));
        }
        let extra = if labels.is_empty() {
            String::new()
        } else {
            format!("  {}", labels.join("  "))
        };
        let style = if system == map_system {
            Style::default().add_modifier(Modifier::BOLD)
        } else if high {
            Style::default().fg(Color::White)
        } else {
            Style::default()
        };
        entries.push(Line::from(Span::styled(
            format!("{mark} {system:>3} {glyphs}{extra}"),
            style,
        )));
    }
    window(&entries, map_system.saturating_sub(1) as usize, height)
}

fn gauge_lines(
    state: &GameState,
    gauges: &[Resources],
    height: usize,
    width: usize,
    high: bool,
) -> Vec<Line<'static>> {
    let spark_w = width.saturating_sub(28).min(24);
    let use_spark = spark_w >= 8;
    let mut lines = Vec::new();
    for kind in [
        ResourceKind::Metal,
        ResourceKind::Crystal,
        ResourceKind::Deuterium,
    ] {
        let stock = state.resources.get(kind);
        let cap = state.storage_cap(kind);
        let rate = state.production_per_hour(kind);
        let hist: Vec<f64> = gauges.iter().map(|row| row.get(kind)).collect();
        let spark = if use_spark {
            sparkline(&hist, spark_w)
        } else {
            String::new()
        };
        let style = if high {
            Style::default().fg(Color::White)
        } else if cap > 0.0 && stock / cap >= 0.95 {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::Green)
        };
        lines.push(Line::from(Span::styled(
            format!(
                "{:<10} {:>8.0}/{:<8.0} {:+.0}/h {spark}",
                kind.as_key(),
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

fn queue_lines(
    state: &GameState,
    height: usize,
    selected: usize,
    high: bool,
) -> Vec<Line<'static>> {
    let now = state.last_tick_unix;
    let mut entries = Vec::new();
    push_planet_queue(
        &mut entries,
        state.home,
        &state.build_queue,
        now,
        selected,
        0,
        high,
    );
    for (idx, colony) in (1..).zip(state.colonies.iter()) {
        push_planet_queue(
            &mut entries,
            colony.coord,
            &colony.build_queue,
            now,
            selected,
            idx,
            high,
        );
    }
    if let Some(job) = &state.research {
        let eta = job.completes_at.saturating_sub(now);
        entries.push(Line::from(format!("research {} {eta}s", job.tech)));
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
    high: bool,
) {
    let mark = if index == selected { '>' } else { ' ' };
    let style = if index == selected {
        Style::default().add_modifier(Modifier::BOLD)
    } else if high {
        Style::default().fg(Color::White)
    } else {
        Style::default()
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

fn fleet_lines(
    state: &GameState,
    height: usize,
    selected: usize,
    high: bool,
) -> Vec<Line<'static>> {
    let mut entries = Vec::new();
    let parked = state.fleet.total();
    let mark = if selected == 0 { '>' } else { ' ' };
    entries.push(styled_row(
        format!("{mark} docked {parked} at {}", state.home),
        selected == 0,
        high,
        Color::Green,
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
            high,
            if combat { Color::Red } else { Color::Cyan },
        ));
    }
    window(
        &entries,
        selected.min(entries.len().saturating_sub(1)),
        height,
    )
}

fn timeline_lines(state: &GameState, height: usize, _high: bool) -> Vec<Line<'static>> {
    let now = state.last_tick_unix;
    let mut entries = Vec::new();
    if !state.history.events.is_empty() {
        for event in state.history.events.iter().rev().take(height.max(1)) {
            let ago = now.saturating_sub(event.at);
            let style = timeline_style(event.kind);
            entries.push(Line::from(Span::styled(
                format!(
                    "{ago}s {} {} {}",
                    event.kind.as_key(),
                    event.actor,
                    event.action
                ),
                style,
            )));
        }
    } else {
        for event in state.events.iter().rev().take(height.max(1)) {
            let ago = now.saturating_sub(event.at);
            entries.push(Line::from(format!("{ago}s {}", event.text)));
        }
    }
    if let Some(line) = bridge_line(state) {
        entries.insert(0, line);
    }
    entries
}

fn bridge_line(state: &GameState) -> Option<Line<'static>> {
    let now = state.last_tick_unix;
    if state.activity.prod_boost_until.is_some_and(|t| t > now)
        || state.activity.research_boost_until.is_some_and(|t| t > now)
    {
        Some(Line::from(Span::styled(
            "bridge boost active",
            Style::default().fg(Color::Cyan),
        )))
    } else {
        None
    }
}

fn timeline_style(kind: HistoryKind) -> Style {
    match kind {
        HistoryKind::Combat => Style::default().fg(Color::Red),
        HistoryKind::Trade => Style::default().fg(Color::Yellow),
        HistoryKind::Activity => Style::default().fg(Color::Cyan),
        HistoryKind::Build | HistoryKind::Research | HistoryKind::Ship => {
            Style::default().fg(Color::Green)
        }
        HistoryKind::Fleet | HistoryKind::Prestige | HistoryKind::Other => Style::default(),
    }
}

fn market_lines(state: &GameState, height: usize, _high: bool) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let market = &state.market;
    lines.push(Line::from(format!(
        "M {:.1}  C {:.1}  D {:.1}",
        market.price(ResourceKind::Metal),
        market.price(ResourceKind::Crystal),
        market.price(ResourceKind::Deuterium)
    )));
    lines.push(Line::from(format!(
        "metal {}",
        sparkline(market.history(ResourceKind::Metal), 16)
    )));
    lines.truncate(height.max(1));
    lines
}

fn raid_lines(state: &GameState, height: usize, high: bool) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    match state.raid_countdown_secs() {
        Some(secs) => lines.push(styled_row(
            format!("next raid {secs}s"),
            true,
            high,
            Color::Red,
        )),
        None => lines.push(Line::from("no raid timer")),
    }
    if state.pending_tactical.is_some() {
        lines.push(styled_row(
            "tactical overlay pending".into(),
            false,
            high,
            Color::Yellow,
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

fn tactics_lines(state: &GameState, height: usize, _high: bool) -> Vec<Line<'static>> {
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
    high: bool,
) -> Vec<Line<'static>> {
    match target {
        Target::Planet(coord) => planet_detail(state, *coord, height, high),
        Target::Fleet { mission } => fleet_detail(state, *mission, height, high),
        Target::Faction(id) => faction_detail(state, id, height, high),
    }
}

fn planet_detail(state: &GameState, coord: Coord, height: usize, high: bool) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(Span::styled(
        format!("planet {coord}"),
        Style::default().add_modifier(Modifier::BOLD),
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
            lines.push(Line::from(format!("spy {}", report.tier)));
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
    if high {
        for line in &mut lines {
            *line = Line::from(Span::styled(
                line.to_string(),
                Style::default().fg(Color::White),
            ));
        }
    }
    lines.truncate(height.max(1));
    lines
}

fn fleet_detail(
    state: &GameState,
    mission_id: Option<u64>,
    height: usize,
    _high: bool,
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
            "ships {}  cargo M{:.0} C{:.0} D{:.0}",
            mission.ships.total(),
            mission.cargo.metal,
            mission.cargo.crystal,
            mission.cargo.deuterium
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

fn faction_detail(state: &GameState, id: &str, height: usize, _high: bool) -> Vec<Line<'static>> {
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
            Some(r) => format!("  {:.0}M", r.metal),
            None => String::new(),
        };
        lines.push(Line::from(format!(
            "rival {} {} last {}{stock}",
            row.name, row.home, row.last_action
        )));
    }
    if lines.is_empty() {
        lines.push(Line::from(format!("faction {id}")));
    }
    lines.truncate(height.max(1));
    lines
}

fn styled_row(text: String, selected: bool, high: bool, color: Color) -> Line<'static> {
    let mut style = Style::default().fg(if high { Color::White } else { color });
    if selected {
        style = style.add_modifier(Modifier::BOLD);
        if high {
            style = style.add_modifier(Modifier::REVERSED);
        }
    }
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
        _ => alerts(state).len().max(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoshi_core::GameState;

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
        state.resources.metal = state.storage_cap(ResourceKind::Metal);
        let rows = alerts(&state);
        let kinds: Vec<_> = rows.iter().map(|row| row.kind.severity()).collect();
        let mut sorted = kinds.clone();
        sorted.sort_unstable();
        assert_eq!(kinds, sorted);
        assert!(rows.iter().any(|row| row.text.contains("incoming raid")));
        assert!(rows.iter().any(|row| row.text.contains("idle")));
    }

    #[test]
    fn resolved_raid_drops_the_raid_alert() {
        let mut state = GameState::new(1_000);
        state.next_raid_at = None;
        state.config.raid.enabled = false;
        let rows = alerts(&state);
        assert!(!rows.iter().any(|row| row.text.contains("incoming raid")));
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
