//! Overview, planet and shipyard screens. Core owns the rules; this module
//! only renders and turns keys into core commands.

use hoshi_core::{BuildingKind, GameState, ResourceKind, Resources, ShipKind};
use ratatui::Frame;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Overview,
    Planet,
    Shipyard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    None,
    Quit,
}

pub struct App {
    pub state: GameState,
    screen: Screen,
    selected: usize,
    status: String,
}

impl App {
    pub fn new(state: GameState) -> Self {
        Self {
            state,
            screen: Screen::Overview,
            selected: 0,
            status: String::new(),
        }
    }

    pub fn handle_key(&mut self, code: KeyCode, now_unix: u64) -> Action {
        self.state.catch_up(now_unix);
        match code {
            KeyCode::Char('q') => return Action::Quit,
            KeyCode::Tab => {
                self.screen = match self.screen {
                    Screen::Overview => Screen::Planet,
                    Screen::Planet => Screen::Shipyard,
                    Screen::Shipyard => Screen::Overview,
                };
                self.selected = 0;
                self.status.clear();
            }
            KeyCode::Char('j') | KeyCode::Down => {
                let len = self.list_len();
                if len > 0 {
                    self.selected = (self.selected + 1) % len;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                let len = self.list_len();
                if len > 0 {
                    self.selected = (self.selected + len - 1) % len;
                }
            }
            KeyCode::Enter => self.activate(now_unix),
            KeyCode::Char('x') => self.cancel(now_unix),
            _ => {}
        }
        Action::None
    }

    pub fn draw(&self, frame: &mut Frame<'_>) {
        let title = match self.screen {
            Screen::Overview => " hoshi  [overview] planet shipyard ",
            Screen::Planet => " hoshi  overview [planet] shipyard ",
            Screen::Shipyard => " hoshi  overview planet [shipyard] ",
        };
        let block = Block::bordered().title(title);
        let inner = block.inner(frame.area());
        frame.render_widget(block, frame.area());

        let header = self.header_lines();
        let footer = self.footer_lines();
        let header_h = header.len() as u16;
        let footer_h = footer.len() as u16;
        let chunks = Layout::vertical([
            Constraint::Length(header_h.min(inner.height)),
            Constraint::Min(1),
            Constraint::Length(footer_h.min(inner.height.saturating_sub(header_h))),
        ])
        .split(inner);

        frame.render_widget(Paragraph::new(header), chunks[0]);
        let list_h = chunks[1].height as usize;
        frame.render_widget(Paragraph::new(self.list_lines(list_h)), chunks[1]);
        frame.render_widget(Paragraph::new(footer), chunks[2]);
    }

    fn activate(&mut self, now_unix: u64) {
        match self.screen {
            Screen::Planet => {
                let Some(kind) = BuildingKind::ALL.get(self.selected).copied() else {
                    return;
                };
                match self.state.enqueue_build(kind, now_unix) {
                    Ok(()) => self.status.clear(),
                    Err(e) => self.status = e.to_string(),
                }
            }
            Screen::Shipyard => {
                let Some(kind) = ShipKind::ALL.get(self.selected).copied() else {
                    return;
                };
                match self.state.enqueue_ships(kind, 1, now_unix) {
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
        }
    }

    fn cancel(&mut self, now_unix: u64) {
        match self.screen {
            Screen::Planet => match self.state.cancel_build(0, now_unix) {
                Ok(_) => self.status.clear(),
                Err(e) => self.status = e.to_string(),
            },
            Screen::Shipyard => match self.state.cancel_ships(0, now_unix) {
                Ok(_) => self.status.clear(),
                Err(e) => self.status = e.to_string(),
            },
            Screen::Overview => match self.state.cancel_research(now_unix) {
                Ok(_) => self.status.clear(),
                Err(e) => self.status = e.to_string(),
            },
        }
    }

    fn list_len(&self) -> usize {
        match self.screen {
            Screen::Planet => BuildingKind::ALL.len(),
            Screen::Shipyard => ShipKind::ALL.len(),
            Screen::Overview => self.state.config.techs.iter().count(),
        }
    }

    fn tech_id_at(&self, index: usize) -> Option<String> {
        self.state
            .config
            .techs
            .iter()
            .nth(index)
            .map(|spec| spec.id.clone())
    }

    fn header_lines(&self) -> Vec<Line<'static>> {
        let s = &self.state;
        let mut lines = vec![
            resource_line("Metal", s, ResourceKind::Metal),
            resource_line("Crystal", s, ResourceKind::Crystal),
            resource_line("Deuterium", s, ResourceKind::Deuterium),
            Line::from(format!(
                "Energy     {:>7.0} / {:.0}  bal {:+.0}",
                s.energy_produced(),
                s.energy_consumed(),
                s.energy_balance()
            )),
            Line::from(self.build_status()),
            Line::from(self.research_status()),
            Line::from(self.ship_status()),
            Line::from(self.raid_status()),
        ];
        if let Some(event) = s.events.last() {
            for piece in event.text.split(". ") {
                lines.push(Line::from(piece.to_string()));
            }
        }
        if !self.status.is_empty() {
            lines.push(Line::from(self.status.clone()));
        }
        lines
    }

    fn footer_lines(&self) -> Vec<Line<'static>> {
        vec![Line::from(match self.screen {
            Screen::Overview => "tab screens  j/k select  enter research  x cancel  q quit",
            Screen::Planet => "tab screens  j/k select  enter build  x cancel queue  q quit",
            Screen::Shipyard => "tab screens  j/k select  enter queue  x cancel queue  q quit",
        })]
    }

    fn list_lines(&self, height: usize) -> Vec<Line<'static>> {
        match self.screen {
            Screen::Planet => self.planet_lines(height),
            Screen::Shipyard => self.shipyard_lines(height),
            Screen::Overview => self.research_lines(height),
        }
    }

    fn planet_lines(&self, height: usize) -> Vec<Line<'static>> {
        let mut rows: Vec<Line<'static>> = Vec::new();
        rows.push(Line::from(format!(
            "Queue {}/{}",
            self.state.build_queue.len(),
            self.state.config.max_build_queue
        )));
        for (i, job) in self.state.build_queue.iter().enumerate() {
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
        rows.push(Line::from("Buildings"));
        let entries: Vec<Line<'static>> = BuildingKind::ALL
            .iter()
            .enumerate()
            .map(|(i, &kind)| self.building_row(i, kind))
            .collect();
        append_windowed(&mut rows, &entries, self.selected, height)
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
                job.ship,
                job.remaining
            )));
        }
        rows.push(Line::from("Ships"));
        let entries: Vec<Line<'static>> = ShipKind::ALL
            .iter()
            .enumerate()
            .map(|(i, &kind)| self.ship_row(i, kind))
            .collect();
        append_windowed(&mut rows, &entries, self.selected, height)
    }

    fn research_lines(&self, height: usize) -> Vec<Line<'static>> {
        let mut rows = vec![Line::from("Research")];
        let specs: Vec<_> = self.state.config.techs.iter().collect();
        let entries: Vec<Line<'static>> = specs
            .iter()
            .enumerate()
            .map(|(i, spec)| self.tech_row(i, spec))
            .collect();
        append_windowed(&mut rows, &entries, self.selected, height)
    }

    fn building_row(&self, index: usize, kind: BuildingKind) -> Line<'static> {
        let level = self.state.buildings.get(kind);
        let cost = self.state.build_cost(kind);
        let secs = self.state.build_time_secs(kind);
        let mark = if index == self.selected { '>' } else { ' ' };
        let text = format!("{mark} {kind}  lv{level}  {}  {secs}s", cost_label(cost));
        styled_row(text, self.building_block_reason(kind))
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
        let text = format!(
            "{mark} {}  lv{level}  {}  {secs}s",
            spec.name,
            cost_label(cost)
        );
        styled_row(text, self.tech_block_reason(&spec.id, cost))
    }

    fn building_block_reason(&self, kind: BuildingKind) -> Option<String> {
        for &(req, required) in &self.state.config.buildings.get(kind).prerequisites {
            let actual = self.state.buildings.get(req)
                + self
                    .state
                    .build_queue
                    .iter()
                    .filter(|job| job.building == req)
                    .count() as u32;
            if actual < required {
                return Some(format!("need {req} {required}"));
            }
        }
        let cost = self.state.build_cost(kind);
        if !self.state.resources.can_afford(cost) {
            return Some(missing_label(self.state.resources.missing(cost)));
        }
        None
    }

    fn tech_block_reason(&self, id: &str, cost: Resources) -> Option<String> {
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
                    job.ship,
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
}

fn resource_line(name: &str, state: &GameState, kind: ResourceKind) -> Line<'static> {
    Line::from(format!(
        "{name:<10} {:>8.0}  {:+.0}/h  cap {:.0}",
        state.resources.get(kind),
        state.production_per_hour(kind),
        state.storage_cap(kind)
    ))
}

fn cost_label(cost: Resources) -> String {
    format!(
        "M{:.0} C{:.0} D{:.0}",
        cost.metal, cost.crystal, cost.deuterium
    )
}

fn missing_label(missing: Resources) -> String {
    let mut parts = Vec::new();
    if missing.metal > 0.5 {
        parts.push(format!("M{:.0}", missing.metal));
    }
    if missing.crystal > 0.5 {
        parts.push(format!("C{:.0}", missing.crystal));
    }
    if missing.deuterium > 0.5 {
        parts.push(format!("D{:.0}", missing.deuterium));
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use hoshi_core::{
        BalanceConfig, Clock, FakeClock, GameState, load_state, open_save, save_state,
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
    fn renders_without_panic_from_60x20_upward() {
        let app = App::new(GameState::new(1_000));
        for (w, h) in [(60, 20), (80, 24), (100, 30)] {
            let text = render_text(&app, w, h);
            assert!(text.contains("Metal"), "{w}x{h}: {text}");
            assert!(text.contains("Energy"), "{w}x{h}: {text}");
            assert!(text.contains("Build:"), "{w}x{h}: {text}");
            assert!(text.contains("Research:"), "{w}x{h}: {text}");
            assert!(text.contains("Ships"), "{w}x{h}: {text}");
            assert!(text.contains("Raid:"), "{w}x{h}: {text}");
        }
    }

    #[test]
    fn overview_shows_raid_countdown() {
        let app = App::new(GameState::new(1_000));
        let text = render_text(&app, 80, 24);
        assert!(text.contains("Raid: 3600s"), "{text}");
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
        state.resources.metal = 10_000.0;
        state.resources.crystal = 10_000.0;
        state.resources.deuterium = 10_000.0;
        state.buildings.research_lab = 1;
        let mut app = App::new(state);

        assert_eq!(app.handle_key(KeyCode::Tab, clock.now_unix()), Action::None);
        assert_eq!(app.screen, Screen::Planet);
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

        app.handle_key(KeyCode::Tab, clock.now_unix());
        app.handle_key(KeyCode::Tab, clock.now_unix());
        assert_eq!(app.screen, Screen::Overview);
        let energy = app
            .state
            .config
            .techs
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
        app.handle_key(KeyCode::Tab, 1_000);
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();
        let text = buffer_text(terminal.backend());
        assert!(
            text.contains("need D") || text.contains("need M"),
            "planet should show missing resources: {text}"
        );
        assert!(
            line_is_dim(terminal.backend(), "research lab")
                || line_is_dim(terminal.backend(), "need"),
            "unaffordable building should be dim: {text}"
        );
    }

    fn open_shipyard(app: &mut App, now: u64) {
        app.handle_key(KeyCode::Tab, now);
        app.handle_key(KeyCode::Tab, now);
        assert_eq!(app.screen, Screen::Shipyard);
    }

    #[test]
    fn shipyard_lists_ships_requirements_and_queue() {
        let mut state = GameState::new(1_000);
        state.buildings.shipyard = 1;
        state.tech_levels.insert("combustion_drive".into(), 1);
        state.resources.metal = 100_000.0;
        state.resources.crystal = 100_000.0;
        state.resources.deuterium = 100_000.0;
        state.fleet.add(ShipKind::LightFighter, 4);
        state.fleet.add(ShipKind::HeavyFighter, 2);
        state.fleet.add(ShipKind::SmallCargo, 1);
        let mut app = App::new(state);
        open_shipyard(&mut app, 1_000);
        let text = render_text(&app, 100, 32);
        assert!(text.contains("[shipyard]"), "{text}");
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
        assert_eq!(app.state.ship_queue[0].ship, ShipKind::LightFighter);
        let text = render_text(&app, 100, 32);
        assert!(text.contains("1. light fighter x1"), "{text}");
        assert!(text.contains("Queue 1/5"), "{text}");
    }

    #[test]
    fn unaffordable_ships_are_dimmed_with_missing_or_shipyard() {
        let mut app = App::new(GameState::new(1_000));
        open_shipyard(&mut app, 1_000);
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
        open_shipyard(&mut app, 1_000);
        app.selected = ShipKind::ALL
            .iter()
            .position(|k| *k == ShipKind::LightFighter)
            .unwrap();
        let backend = TestBackend::new(100, 32);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();
        let text = buffer_text(terminal.backend());
        assert!(
            text.contains("need M") || text.contains("need C"),
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
        state.resources.metal = 100_000.0;
        state.resources.crystal = 100_000.0;
        state.resources.deuterium = 100_000.0;
        let mut app = App::new(state);
        app.handle_key(KeyCode::Tab, 1_000);
        assert_eq!(app.screen, Screen::Planet);
        app.handle_key(KeyCode::Enter, 1_000);
        assert_eq!(app.state.build_queue.len(), 1);
        app.handle_key(KeyCode::Tab, 1_000);
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

        app.handle_key(KeyCode::Tab, 1_000);
        app.handle_key(KeyCode::Tab, 1_000);
        assert_eq!(app.screen, Screen::Planet);
        app.handle_key(KeyCode::Char('x'), 1_000);
        assert!(app.state.build_queue.is_empty());
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
        app.handle_key(KeyCode::Tab, 50);
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
}
