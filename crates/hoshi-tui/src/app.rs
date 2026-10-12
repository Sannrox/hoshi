//! Overview, planet, shipyard, wonder, prestige, market, diplomacy and reports screens.
//! Core owns the rules; this module only renders and turns keys into core commands.

use crate::title::AwaySummary;
use hoshi_core::{
    BuildingKind, Coord, DefenceKind, Fleet, GameState, ResourceKind, Resources, ShipKind,
    SpyReport, StoryEvent, WonderError, encode_galaxy_seed,
};
use ratatui::Frame;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use std::io::{IsTerminal, Write};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Overview,
    Planet,
    Shipyard,
    Wonder,
    Prestige,
    Market,
    Diplomacy,
    Reports,
}

const TRADE_PAIRS: [(ResourceKind, ResourceKind); 6] = [
    (ResourceKind::Metal, ResourceKind::Crystal),
    (ResourceKind::Metal, ResourceKind::Deuterium),
    (ResourceKind::Crystal, ResourceKind::Metal),
    (ResourceKind::Crystal, ResourceKind::Deuterium),
    (ResourceKind::Deuterium, ResourceKind::Metal),
    (ResourceKind::Deuterium, ResourceKind::Crystal),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    None,
    Quit,
}

pub struct App {
    pub state: GameState,
    screen: Screen,
    selected: usize,
    planet_index: usize,
    status: String,
    confirming_prestige: bool,
    trade_amount: f64,
    away: Option<AwaySummary>,
    away_hidden: bool,
    opened_report: bool,
}

impl App {
    pub fn new(state: GameState) -> Self {
        Self {
            state,
            screen: Screen::Overview,
            selected: 0,
            planet_index: 0,
            status: String::new(),
            confirming_prestige: false,
            trade_amount: 10.0,
            away: None,
            away_hidden: false,
            opened_report: false,
        }
    }

    pub fn set_away(&mut self, summary: AwaySummary) {
        self.away = Some(summary);
        self.away_hidden = false;
    }

    pub fn handle_key(&mut self, code: KeyCode, now_unix: u64) -> Action {
        self.state.catch_up(now_unix);
        self.clamp_planet();
        if self.away.is_some() && !self.away_hidden {
            match code {
                KeyCode::Enter | KeyCode::Char('x') | KeyCode::Esc => {
                    self.away_hidden = true;
                }
                KeyCode::Char('q') => return Action::Quit,
                _ => {}
            }
            return Action::None;
        }
        if self.story_event().is_some() {
            match code {
                KeyCode::Char('q') => return Action::Quit,
                KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => {
                    let idx = (c as u8 - b'1') as usize;
                    self.choose_story_at(idx, now_unix);
                }
                _ => {}
            }
            return Action::None;
        }
        if self.opened_report {
            match code {
                KeyCode::Enter | KeyCode::Char('x') | KeyCode::Esc => {
                    self.opened_report = false;
                }
                KeyCode::Char('q') => return Action::Quit,
                _ => {}
            }
            return Action::None;
        }
        match code {
            KeyCode::Char('q') => return Action::Quit,
            KeyCode::Char('w') if self.screen == Screen::Overview && self.away.is_some() => {
                self.away_hidden = false;
            }
            KeyCode::Tab => {
                self.screen = match self.screen {
                    Screen::Overview => Screen::Planet,
                    Screen::Planet => Screen::Shipyard,
                    Screen::Shipyard => Screen::Wonder,
                    Screen::Wonder => Screen::Prestige,
                    Screen::Prestige => Screen::Market,
                    Screen::Market => Screen::Diplomacy,
                    Screen::Diplomacy => Screen::Reports,
                    Screen::Reports => Screen::Overview,
                };
                self.selected = 0;
                self.status.clear();
                self.confirming_prestige = false;
                self.opened_report = false;
            }
            KeyCode::Char(']') => self.cycle_planet(1),
            KeyCode::Char('[') => self.cycle_planet(-1),
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
            KeyCode::Char('+') if self.screen == Screen::Market => {
                self.trade_amount = (self.trade_amount + 10.0).min(1_000_000.0);
            }
            KeyCode::Char('-') if self.screen == Screen::Market => {
                self.trade_amount = (self.trade_amount - 10.0).max(10.0);
            }
            KeyCode::Char('s') if self.screen == Screen::Reports && !self.opened_report => {
                self.send_probe(now_unix);
            }
            KeyCode::Enter => self.activate(now_unix),
            KeyCode::Char('x') => self.cancel(now_unix),
            KeyCode::Char('y') => {
                let code = encode_galaxy_seed(self.state.galaxy_seed);
                copy_to_clipboard(&code);
                self.status = format!("copied {code}");
            }
            _ => {}
        }
        Action::None
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
        let title = match self.screen {
            Screen::Overview => {
                " hoshi  [overview] planet shipyard wonder prestige market diplomacy reports "
            }
            Screen::Planet => {
                " hoshi  overview [planet] shipyard wonder prestige market diplomacy reports "
            }
            Screen::Shipyard => {
                " hoshi  overview planet [shipyard] wonder prestige market diplomacy reports "
            }
            Screen::Wonder => {
                " hoshi  overview planet shipyard [wonder] prestige market diplomacy reports "
            }
            Screen::Prestige => {
                " hoshi  overview planet shipyard wonder [prestige] market diplomacy reports "
            }
            Screen::Market => {
                " hoshi  overview planet shipyard wonder prestige [market] diplomacy reports "
            }
            Screen::Diplomacy => {
                " hoshi  overview planet shipyard wonder prestige market [diplomacy] reports "
            }
            Screen::Reports => {
                " hoshi  overview planet shipyard wonder prestige market diplomacy [reports] "
            }
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
        if let Some(away) = &self.away
            && !self.away_hidden
        {
            self.draw_away(frame, away);
        } else if let Some(event) = self.story_event() {
            self.draw_story(frame, event);
        } else if self.opened_report {
            self.draw_report(frame);
        }
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

    fn draw_away(&self, frame: &mut Frame<'_>, away: &AwaySummary) {
        let area = frame.area();
        let block = Block::bordered().title(" while you were away ");
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let mut lines = vec![
            Line::from(format!("elapsed {}s", away.elapsed_secs)),
            Line::from(format!(
                "resources +{:.0} metal +{:.0} crystal +{:.0} deuterium",
                away.metal, away.crystal, away.deuterium
            )),
            Line::from(format!(
                "builds {}  research {}  fleets returned {}  expeditions {}  raids {}",
                away.builds, away.research, away.fleets_returned, away.expeditions, away.raids
            )),
        ];
        for event in away.events.iter().take(8) {
            lines.push(Line::from(event.clone()));
        }
        lines.push(Line::from("Enter dismisses. w on overview reopens."));
        frame.render_widget(Paragraph::new(lines), inner);
    }

    fn draw_story(&self, frame: &mut Frame<'_>, event: &StoryEvent) {
        let area = frame.area();
        let block = Block::bordered().title(format!(" {} ", event.title));
        let inner = block.inner(area);
        frame.render_widget(block, area);
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
        frame.render_widget(Paragraph::new(lines), inner);
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
            Screen::Market => self.activate_market(now_unix),
            Screen::Diplomacy => self.activate_diplomacy(now_unix),
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
            Screen::Market | Screen::Diplomacy | Screen::Reports => {}
        }
    }

    fn list_len(&self) -> usize {
        match self.screen {
            Screen::Planet => BuildingKind::ALL.len(),
            Screen::Shipyard => ShipKind::ALL.len() + DefenceKind::ALL.len(),
            Screen::Overview => self.state.config.techs.iter().count(),
            Screen::Wonder => self.state.config.wonders.len(),
            Screen::Prestige => 1 + self.state.config.prestige.bonuses.len(),
            Screen::Market => TRADE_PAIRS.len(),
            Screen::Diplomacy => self.state.config.diplomacy.len(),
            Screen::Reports => self.state.outposts.len(),
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
            Line::from(self.wonder_status()),
            Line::from(self.prestige_status()),
            Line::from(format!(
                "Seed {}  gen {}",
                encode_galaxy_seed(s.galaxy_seed),
                s.generator_version
            )),
        ];
        lines.extend(self.planet_glance_lines());
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
            Screen::Overview => {
                "tab screens  [/] planet  j/k select  enter research  x cancel  y copy seed  q quit"
            }
            Screen::Planet => {
                "tab screens  [/] planet  j/k select  enter build  x cancel queue  q quit"
            }
            Screen::Shipyard => {
                "tab screens  [/] planet  j/k select  enter queue  x cancel queue  q quit"
            }
            Screen::Wonder => "tab screens  [/] planet  j/k select  enter start/contribute  q quit",
            Screen::Prestige => "tab screens  j/k select  enter buy/reset  x back  q quit",
            Screen::Market => "tab screens  j/k pair  +/- amount  enter trade  q quit",
            Screen::Diplomacy => "tab screens  j/k faction  enter tribute  q quit",
            Screen::Reports => "tab screens  j/k outpost  enter report  s send probe  q quit",
        })]
    }

    fn list_lines(&self, height: usize) -> Vec<Line<'static>> {
        match self.screen {
            Screen::Planet => self.planet_lines(height),
            Screen::Shipyard => self.shipyard_lines(height),
            Screen::Overview => self.research_lines(height),
            Screen::Wonder => self.wonder_lines(height),
            Screen::Prestige => self.prestige_lines(height),
            Screen::Market => self.market_lines(height),
            Screen::Diplomacy => self.diplomacy_lines(height),
            Screen::Reports => self.report_lines(height),
        }
    }

    fn planet_lines(&self, height: usize) -> Vec<Line<'static>> {
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
        let mut rows = vec![Line::from("Research")];
        let specs: Vec<_> = self.state.config.techs.iter().collect();
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
                    "Active {name}  stage {}/{total}  need M{:.0} C{:.0} D{:.0}  eta {eta}",
                    job.stage + 1,
                    remaining.metal,
                    remaining.crystal,
                    remaining.deuterium
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

    fn building_row(&self, index: usize, kind: BuildingKind) -> Line<'static> {
        let coord = self.selected_coord();
        let level = self
            .state
            .buildings_at(coord)
            .unwrap_or(&self.state.buildings)
            .get(kind);
        let cost = self.state.build_cost_on(coord, kind);
        let secs = self.state.build_time_secs_on(coord, kind);
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
        let text = format!(
            "{mark} {}  lv{level}  {}  {secs}s",
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

    fn market_lines(&self, height: usize) -> Vec<Line<'static>> {
        let market = &self.state.market;
        let fee_pct = self.state.effective_market_fee() * 100.0;
        let mut rows = vec![
            Line::from(format!(
                "Prices  metal {:.3}  crystal {:.3}  deuterium {:.3}",
                market.metal, market.crystal, market.deuterium
            )),
            Line::from(format!(
                "metal     {}",
                sparkline(market.history(ResourceKind::Metal), 24)
            )),
            Line::from(format!(
                "crystal   {}",
                sparkline(market.history(ResourceKind::Crystal), 24)
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
        append_windowed(&mut rows, &entries, self.selected, height)
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

    fn draw_report(&self, frame: &mut Frame<'_>) {
        let area = frame.area();
        let dest = self.selected_outpost();
        let title = dest
            .map(|c| format!(" spy {c} "))
            .unwrap_or_else(|| " spy ".into());
        let block = Block::bordered().title(title);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let lines = match dest.and_then(|c| self.state.latest_report(c)) {
            Some(report) => report_overlay_lines(report, self.state.last_tick_unix),
            None => vec![Line::from("no spy report for this outpost")],
        };
        frame.render_widget(Paragraph::new(lines), inner);
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
                Line::from(format!(
                    "{mark} {coord}  M{:.0} C{:.0} D{:.0}  {active}",
                    res.metal, res.crystal, res.deuterium
                ))
            })
            .collect()
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
        lines.push(Line::from(format!(
            "resources M{:.0} C{:.0} D{:.0}",
            res.metal, res.crystal, res.deuterium
        )));
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

fn copy_to_clipboard(text: &str) {
    let mut out = std::io::stdout();
    if !out.is_terminal() {
        return;
    }
    let _ = write!(out, "\x1b]52;c;{}\x07", base64_encode(text.as_bytes()));
    let _ = out.flush();
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        let b0 = bytes[i];
        let b1 = bytes.get(i + 1).copied().unwrap_or(0);
        let b2 = bytes.get(i + 2).copied().unwrap_or(0);
        let n = ((b0 as u32) << 16) | ((b1 as u32) << 8) | b2 as u32;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        if i + 1 < bytes.len() {
            out.push(TABLE[((n >> 6) & 63) as usize] as char);
        } else {
            out.push('=');
        }
        if i + 2 < bytes.len() {
            out.push(TABLE[(n & 63) as usize] as char);
        } else {
            out.push('=');
        }
        i += 3;
    }
    out
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

fn sparkline(values: &[f64], width: usize) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;
    use hoshi_core::{
        BalanceConfig, Buildings, Clock, Colony, Coord, FakeClock, Fleet, GameState, PlanetType,
        Resources, load_state, open_save, save_state,
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
            metal: 10.0,
            crystal: 0.0,
            deuterium: 0.0,
            builds: 1,
            research: 0,
            fleets_returned: 0,
            expeditions: 0,
            raids: 0,
            events: vec!["raid: lost".into()],
        });
        let text = render_text(&app, 80, 24);
        assert!(text.contains("while you were away"), "{text}");
        assert!(text.contains("elapsed 120s"), "{text}");
        app.handle_key(KeyCode::Enter, 1_000);
        let text = render_text(&app, 80, 24);
        assert!(!text.contains("while you were away"), "{text}");
        app.handle_key(KeyCode::Char('w'), 1_000);
        let text = render_text(&app, 80, 24);
        assert!(text.contains("while you were away"), "{text}");
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
            assert!(text.contains("Wonder:"), "{w}x{h}: {text}");
            assert!(text.contains("Prestige:"), "{w}x{h}: {text}");
        }
    }

    #[test]
    fn overview_shows_raid_countdown() {
        let app = App::new(GameState::new(1_000));
        let text = render_text(&app, 80, 24);
        assert!(text.contains("Raid: 3600s"), "{text}");
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
        app.handle_key(KeyCode::Tab, clock.now_unix());
        app.handle_key(KeyCode::Tab, clock.now_unix());
        app.handle_key(KeyCode::Tab, clock.now_unix());
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
        assert_eq!(app.state.ship_queue[0].ship, Some(ShipKind::LightFighter));
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
        app.handle_key(KeyCode::Tab, 1_000);
        app.handle_key(KeyCode::Tab, 1_000);
        app.handle_key(KeyCode::Tab, 1_000);
        app.handle_key(KeyCode::Tab, 1_000);
        app.handle_key(KeyCode::Tab, 1_000);
        assert_eq!(app.screen, Screen::Planet);
        app.handle_key(KeyCode::Char('x'), 1_000);
        assert!(app.state.build_queue.is_empty());
    }

    #[test]
    fn shipyard_lists_defences_and_queues_a_cannon() {
        let mut state = GameState::new(1_000);
        state.buildings.shipyard = 1;
        state.resources.metal = 100_000.0;
        state.resources.crystal = 100_000.0;
        state.resources.deuterium = 100_000.0;
        let mut app = App::new(state);
        open_shipyard(&mut app, 1_000);
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

    #[test]
    fn planet_switcher_shows_glance_and_builds_on_the_selected_world() {
        let dest = Coord {
            galaxy: 1,
            system: 1,
            position: 2,
        };
        let mut state = GameState::new(1_000);
        state.resources.metal = 10_000.0;
        state.resources.crystal = 10_000.0;
        state.resources.deuterium = 10_000.0;
        state.colonies.push(Colony {
            coord: dest,
            kind: PlanetType::Rocky,
            size: 120,
            resources: Resources {
                metal: 10_000.0,
                crystal: 10_000.0,
                deuterium: 10_000.0,
            },
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
        assert!(text.contains("M10000"), "{text}");

        app.handle_key(KeyCode::Char(']'), 1_000);
        assert_eq!(app.selected_coord(), dest);
        app.handle_key(KeyCode::Tab, 1_000);
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
        app.handle_key(KeyCode::Tab, 1_000);
        assert_eq!(app.screen, Screen::Planet);
        let text = render_text(&app, 100, 32);
        assert!(text.contains("Planet 1:1:2"), "{text}");
        assert!(text.contains("Rare: Rich vein"), "{text}");
        assert!(text.contains("metal +25%"), "{text}");
    }

    #[test]
    fn planet_screen_shows_deposits_from_type_and_seed() {
        let state = GameState::new(1_000);
        let mut app = App::new(state);
        app.handle_key(KeyCode::Tab, 1_000);
        assert_eq!(app.screen, Screen::Planet);
        let text = render_text(&app, 100, 32);
        assert!(text.contains("Deposits"), "{text}");
        assert!(text.contains("iron_ore"), "{text}");
    }

    #[test]
    fn wonder_screen_shows_remaining_and_eta_on_small_terminals() {
        let mut state = GameState::new(1_000);
        state.tech_levels.insert("energy".into(), 8);
        state.resources.metal = 1_000.0;
        state.resources.crystal = 500.0;
        state.resources.deuterium = 2_000.0;
        let mut app = App::new(state);
        app.handle_key(KeyCode::Tab, 1_000);
        app.handle_key(KeyCode::Tab, 1_000);
        app.handle_key(KeyCode::Tab, 1_000);
        assert_eq!(app.screen, Screen::Wonder);
        let _ = render_text(&app, 40, 12);
        app.selected = 0;
        app.handle_key(KeyCode::Enter, 1_000);
        let remaining = app.state.wonder_remaining().expect("stage remaining");
        let eta = app.state.wonder_eta_secs().expect("eta");
        let text = render_text(&app, 100, 32);
        assert!(text.contains("[wonder]"), "{text}");
        assert!(text.contains("Dyson ring"), "{text}");
        assert!(
            text.contains(&format!("need M{:.0}", remaining.metal)),
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
        app.handle_key(KeyCode::Tab, 1_000);
        app.handle_key(KeyCode::Tab, 1_000);
        app.handle_key(KeyCode::Tab, 1_000);
        app.handle_key(KeyCode::Tab, 1_000);
        assert_eq!(app.screen, Screen::Prestige);
        let text = render_text(&app, 100, 32);
        assert!(text.contains("[prestige]"), "{text}");
        assert!(text.contains("Reset"), "{text}");
        app.handle_key(KeyCode::Enter, 1_000);
        assert!(app.confirming_prestige);
        let text = render_text(&app, 100, 32);
        assert!(text.contains("Reset keeps:"), "{text}");
        assert!(text.contains("prestige currency"), "{text}");
        assert!(text.contains("Reset loses:"), "{text}");
        assert!(text.contains("resources"), "{text}");
        app.handle_key(KeyCode::Enter, 1_000);
        assert_eq!(app.state.prestige.current.index, 2);
        assert!(app.state.completed_wonders.is_empty());
    }

    #[test]
    fn story_overlay_shows_text_and_number_keys_choose() {
        let mut app = App::new(GameState::new(1_000));
        app.state.story_pending = Some(hoshi_core::PendingStory {
            event_id: "derelict".into(),
            offered_at: 1_000,
            timeout_at: 10_000,
        });
        let text = render_text(&app, 80, 24);
        assert!(text.contains("Derelict hull"), "{text}");
        assert!(text.contains("Cut it open"), "{text}");
        assert!(text.contains("Tow it home"), "{text}");
        app.handle_key(KeyCode::Char('2'), 1_000);
        assert!(app.state.story_pending.is_none());
        assert!(
            app.state
                .events
                .iter()
                .any(|e| e.text.contains("Derelict hull")),
            "{:?}",
            app.state.events
        );
        let text = render_text(&app, 80, 24);
        assert!(!text.contains("Cut it open"), "{text}");
    }

    #[test]
    fn market_screen_shows_prices_sparkline_and_trade_preview() {
        let mut state = GameState::new(1_000);
        state.resources.metal = 1_000.0;
        let mut app = App::new(state);
        for _ in 0..5 {
            app.handle_key(KeyCode::Tab, 1_000);
        }
        assert_eq!(app.screen, Screen::Market);
        let text = render_text(&app, 100, 32);
        assert!(text.contains("[market]"), "{text}");
        assert!(text.contains("Prices"), "{text}");
        assert!(text.contains("preview"), "{text}");
        assert!(
            text.chars().any(|c| "▁▂▃▄▅▆▇█".contains(c)),
            "sparkline missing: {text}"
        );
        let before = app.state.resources.metal;
        app.handle_key(KeyCode::Enter, 1_000);
        assert!(
            app.state.resources.metal < before,
            "trade should spend metal"
        );
        app.handle_key(KeyCode::Char('+'), 1_000);
        let text = render_text(&app, 100, 32);
        assert!(text.contains("amount 20"), "{text}");
    }

    #[test]
    fn diplomacy_screen_shows_standing_next_tier_and_pays_tribute() {
        let mut state = GameState::new(1_000);
        state.resources.metal = 1_000.0;
        let mut app = App::new(state);
        for _ in 0..6 {
            app.handle_key(KeyCode::Tab, 1_000);
        }
        assert_eq!(app.screen, Screen::Diplomacy);
        let text = render_text(&app, 100, 32);
        assert!(text.contains("[diplomacy]"), "{text}");
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
        });
        let mut app = App::new(state);
        for _ in 0..7 {
            app.handle_key(KeyCode::Tab, 1_000);
        }
        assert_eq!(app.screen, Screen::Reports);
        let text = render_text(&app, 100, 32);
        assert!(text.contains("[reports]"), "{text}");
        assert!(text.contains(&dest.to_string()), "{text}");
        assert!(text.contains("age 100s"), "{text}");
        assert!(text.contains("garrison"), "{text}");
        app.handle_key(KeyCode::Enter, 1_000);
        let text = render_text(&app, 100, 32);
        assert!(text.contains(&format!("spy {dest}")), "{text}");
        assert!(text.contains("detected yes"), "{text}");
        assert!(text.contains("lost 1/3"), "{text}");
        assert!(text.contains(&format!("garrison {garrison}")), "{text}");
        app.handle_key(KeyCode::Enter, 1_000);
        let text = render_text(&app, 100, 32);
        assert!(text.contains("[reports]"), "{text}");
        assert!(!text.contains("detected yes"), "{text}");
    }

    fn close_standing(app: &App, id: &str, expected: f64) {
        let got = app.state.diplomacy.standing(id);
        assert!(
            (got - expected).abs() < 1e-6,
            "{id} standing {got} != {expected}"
        );
    }
}
